//! The pinyin engine: embedded dictionary, candidate generation and the key
//! reducer. Everything here is pure and platform independent.
//!
//! Port of `tip/strategy.go`. The dictionary ships inside the DLL, so no host
//! process or pipe is involved: a key goes in, a [`Reduction`] comes out.

use std::collections::{BTreeMap, HashSet};
use std::ops::Bound;
use std::sync::OnceLock;

/// Rime-format dictionary embedded into the binary.
const DICTIONARY_SOURCE: &str = include_str!("../data/pinyin_simp.dict.yaml");

/// The candidate window shows at most nine entries, one per digit key.
pub const MAX_CANDIDATES: usize = 9;

/// When segmenting a multi-syllable composition, only the best few words of
/// each syllable take part in the search.
const SEGMENT_WORDS_PER_CODE: usize = 4;
/// Partial segmentations kept per position (a small beam).
const SEGMENT_BEAM_WIDTH: usize = 24;
/// Each extra word costs this much, so fewer longer words outrank many short ones.
const SEGMENT_WORD_PENALTY: f64 = 100.0;

/// One entry of the candidate list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub text: String,
    pub index: usize,
    /// The pinyin code the entry came from; empty for synthesized phrases.
    pub annotation: String,
}

#[derive(Debug)]
struct Entry {
    text: String,
    weight: f64,
}

/// Pinyin code -> words, best weight first.
#[derive(Debug, Default)]
pub struct Dictionary {
    entries: BTreeMap<String, Vec<Entry>>,
    max_code_len: usize,
}

/// Lowercases and strips the separators a user may type between syllables.
pub fn normalize_pinyin(text: &str) -> String {
    text.to_lowercase().chars().filter(|c| !matches!(c, ' ' | '\t' | '\'' | '\u{2019}' | '`' | '-')).collect()
}

impl Dictionary {
    /// Parses a Rime `*.dict.yaml` file. Header lines and anything before the
    /// `...` marker are skipped; malformed rows are dropped silently, like the
    /// Go loader did.
    pub fn parse(source: &str) -> Dictionary {
        let mut entries: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
        let mut in_table = false;
        for line in source.lines() {
            if line.trim() == "..." {
                in_table = true;
                continue;
            }
            if !in_table || line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut columns = line.split('\t');
            let (Some(text), Some(code)) = (columns.next(), columns.next()) else {
                continue;
            };
            let text = text.trim();
            let code = normalize_pinyin(code.trim());
            if text.is_empty() || code.is_empty() || !code.bytes().all(|b| b.is_ascii_lowercase()) {
                continue;
            }
            // A trailing `%` marks a percentage weight, which this engine ignores.
            let weight = match columns.next().map(str::trim) {
                Some(w) if !w.ends_with('%') => w.parse::<f64>().unwrap_or(0.0),
                _ => 0.0,
            };
            entries.entry(code).or_default().push(Entry { text: text.to_owned(), weight });
        }
        let mut max_code_len = 0;
        for (code, words) in entries.iter_mut() {
            max_code_len = max_code_len.max(code.len());
            // Stable: equal weights keep file order.
            words.sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal));
        }
        Dictionary { entries, max_code_len }
    }

    /// Number of distinct pinyin codes.
    pub fn code_count(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Candidates for a (possibly partial) pinyin composition.
    ///
    /// Three stages, first one that yields anything wins:
    /// 1. exact dictionary entries for the whole composition;
    /// 2. the best segmentation into dictionary words;
    /// 3. entries whose code merely starts with the composition.
    pub fn candidates_for(&self, composition: &str) -> Vec<Candidate> {
        let query = normalize_pinyin(composition);
        if query.is_empty() {
            return Vec::new();
        }
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<Candidate> = Vec::new();

        // Stage 1: exact.
        if let Some(words) = self.entries.get(&query) {
            for word in words {
                if push_unique(&mut out, &mut seen, &word.text, &query) {
                    return out;
                }
            }
        }
        if !out.is_empty() {
            return out;
        }

        // Stage 2: segment a multi-syllable composition into words, ranking the
        // combinations by summed word weight.
        for phrase in self.segment(&query) {
            if push_unique(&mut out, &mut seen, &phrase, "") {
                return out;
            }
        }
        if !out.is_empty() {
            return out;
        }

        // Stage 3: prefix fallback gives useful candidates while the user is
        // still typing. Codes are ordered by the weight of their best word.
        // (The Go version iterated a map and used an unstable sort, so ties came
        // out in a random order; here they fall back to alphabetical order.)
        let mut codes: Vec<(&String, f64)> = self
            .entries
            .range::<str, _>((Bound::Included(query.as_str()), Bound::Unbounded))
            .take_while(|(code, _)| code.starts_with(&query))
            .map(|(code, words)| (code, words.first().map_or(f64::NEG_INFINITY, |w| w.weight)))
            .collect();
        codes.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for (code, _) in codes {
            for word in &self.entries[code] {
                if push_unique(&mut out, &mut seen, &word.text, code) {
                    return out;
                }
            }
        }
        out
    }

    /// Best-first segmentations of `query` into dictionary words.
    fn segment(&self, query: &str) -> Vec<String> {
        struct Partial {
            text: String,
            weight: f64,
        }
        let mut paths: Vec<Vec<Partial>> = (0..=query.len()).map(|_| Vec::new()).collect();
        paths[0].push(Partial { text: String::new(), weight: 0.0 });

        for offset in 0..query.len() {
            if paths[offset].is_empty() {
                continue;
            }
            let prefixes: Vec<(String, f64)> = paths[offset].iter().map(|p| (p.text.clone(), p.weight)).collect();
            // Every code that matches here is a prefix of query[offset..], so
            // walking the possible lengths upwards visits them in exactly the
            // order a sorted scan over all codes would.
            let longest = self.max_code_len.min(query.len() - offset);
            for len in 1..=longest {
                // `get` is None on a non-char boundary, which can't be a code.
                let Some(code) = query.get(offset..offset + len) else { continue };
                let Some(words) = self.entries.get(code) else { continue };
                let next = offset + len;
                let top = words.len().min(SEGMENT_WORDS_PER_CODE);
                for (prefix_text, prefix_weight) in &prefixes {
                    for word in &words[..top] {
                        paths[next].push(Partial {
                            text: format!("{prefix_text}{}", word.text),
                            weight: prefix_weight + word.weight - SEGMENT_WORD_PENALTY,
                        });
                    }
                }
                paths[next].sort_by(|a, b| b.weight.partial_cmp(&a.weight).unwrap_or(std::cmp::Ordering::Equal));
                paths[next].truncate(SEGMENT_BEAM_WIDTH);
            }
        }
        paths.pop().unwrap_or_default().into_iter().map(|p| p.text).collect()
    }
}

/// Appends `text` unless already present. Returns true once the list is full.
fn push_unique(out: &mut Vec<Candidate>, seen: &mut HashSet<String>, text: &str, annotation: &str) -> bool {
    if seen.insert(text.to_owned()) {
        out.push(Candidate { text: text.to_owned(), index: out.len(), annotation: annotation.to_owned() });
    }
    out.len() >= MAX_CANDIDATES
}

/// The embedded dictionary, parsed on first use. Parsing lazily keeps the cost
/// out of every process that merely loads the DLL.
pub fn dictionary() -> &'static Dictionary {
    static DICTIONARY: OnceLock<Dictionary> = OnceLock::new();
    DICTIONARY.get_or_init(|| Dictionary::parse(DICTIONARY_SOURCE))
}

/// Candidates for `composition` from the embedded dictionary.
pub fn candidates_for(composition: &str) -> Vec<Candidate> {
    dictionary().candidates_for(composition)
}

/// What the reducer needs to remember between keys.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImeState {
    pub composition: String,
    pub selected: usize,
}

/// A key the engine understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A lowercase ASCII letter.
    Letter(char),
    /// An ASCII digit.
    Digit(char),
    Backspace,
    Delete,
    Escape,
    ArrowUp,
    ArrowLeft,
    ArrowDown,
    ArrowRight,
    Space,
    Enter,
}

impl Key {
    /// Maps a Windows virtual-key code. Keys the engine has no use for map to
    /// `None`, and the caller must leave them to the application: claiming one
    /// in the test phase and declining it in `OnKeyDown` would leave the two
    /// phases disagreeing about every unmapped key.
    pub fn from_vk(vk: usize) -> Option<Key> {
        Some(match vk {
            0x41..=0x5A => Key::Letter((vk as u8).to_ascii_lowercase() as char),
            0x30..=0x39 => Key::Digit(vk as u8 as char),
            0x08 => Key::Backspace,
            0x2E => Key::Delete,
            0x1B => Key::Escape,
            0x26 => Key::ArrowUp,
            0x25 => Key::ArrowLeft,
            0x28 => Key::ArrowDown,
            0x27 => Key::ArrowRight,
            0x20 => Key::Space,
            0x0D => Key::Enter,
            _ => return None,
        })
    }
}

/// Human-readable name of a virtual key, for the diagnostic log only.
pub fn key_name(vk: usize) -> String {
    match vk {
        0x08 => "Backspace".into(),
        0x2E => "Delete".into(),
        0x1B => "Escape".into(),
        0x0D => "Enter".into(),
        0x20 => " ".into(),
        0x25 => "ArrowLeft".into(),
        0x26 => "ArrowUp".into(),
        0x27 => "ArrowRight".into(),
        0x28 => "ArrowDown".into(),
        0x31..=0x39 | 0x41..=0x5A => (vk as u8 as char).to_string(),
        _ => String::new(),
    }
}

/// Result of feeding one key to the engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reduction {
    pub state: ImeState,
    /// Whether the engine keeps the key (true) or leaves it to the application.
    pub consume: bool,
    /// Text to insert into the document, ending the composition.
    pub commit: Option<String>,
}

impl Reduction {
    fn pass(state: &ImeState) -> Reduction {
        Reduction { state: state.clone(), consume: false, commit: None }
    }
    fn keep(state: ImeState) -> Reduction {
        Reduction { state, consume: true, commit: None }
    }
    fn commit(text: String) -> Reduction {
        Reduction { state: ImeState::default(), consume: true, commit: Some(text) }
    }
}

/// The input-method state machine. Pure: it neither mutates `state` nor touches
/// the document, which is what lets `OnTestKeyDown` predict a key safely.
pub fn reduce_ime(state: &ImeState, key: Key) -> Reduction {
    let composing = !state.composition.is_empty();
    match key {
        Key::Letter(letter) => {
            let mut composition = state.composition.clone();
            composition.push(letter);
            Reduction::keep(ImeState { composition, selected: 0 })
        }
        Key::Backspace if composing => {
            let mut composition = state.composition.clone();
            composition.pop();
            Reduction::keep(ImeState { composition, selected: 0 })
        }
        Key::Delete if composing => {
            let composition: String = state.composition.chars().skip(1).collect();
            Reduction::keep(ImeState { composition, selected: 0 })
        }
        Key::Escape if composing => Reduction::keep(ImeState::default()),
        Key::ArrowUp | Key::ArrowLeft | Key::ArrowDown | Key::ArrowRight if composing => {
            let candidates = candidates_for(&state.composition);
            if candidates.is_empty() {
                return Reduction::pass(state);
            }
            let delta: i64 = if matches!(key, Key::ArrowUp | Key::ArrowLeft) { -1 } else { 1 };
            let selected = (state.selected as i64 + delta).clamp(0, candidates.len() as i64 - 1);
            Reduction::keep(ImeState { composition: state.composition.clone(), selected: selected as usize })
        }
        Key::Space | Key::Enter if composing => {
            let candidates = candidates_for(&state.composition);
            // With nothing to pick, the raw pinyin is committed as typed.
            let text = match candidates.get(state.selected).or_else(|| candidates.first()) {
                Some(candidate) => candidate.text.clone(),
                None => state.composition.clone(),
            };
            Reduction::commit(text)
        }
        Key::Digit(digit) if composing && digit != '0' => {
            let candidates = candidates_for(&state.composition);
            match candidates.get(digit as usize - '1' as usize) {
                Some(candidate) => Reduction::commit(candidate.text.clone()),
                None => Reduction::pass(state),
            }
        }
        _ => Reduction::pass(state),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(state: &ImeState, keys: &[Key]) -> ImeState {
        let mut state = state.clone();
        for key in keys {
            let r = reduce_ime(&state, *key);
            assert!(r.consume, "{key:?} should be consumed");
            state = r.state;
        }
        state
    }

    fn letters(text: &str) -> Vec<Key> {
        text.chars().map(Key::Letter).collect()
    }

    // --- port of TestReduceIMEKeyLifecycle ---
    #[test]
    fn key_lifecycle_builds_a_composition_and_commits_it() {
        let s = press(&ImeState::default(), &letters("ni"));
        assert_eq!(s.composition, "ni");
        let got = candidates_for(&s.composition);
        assert!(!got.is_empty() && !got[0].text.is_empty(), "expected embedded dictionary candidates for ni");
        assert!(!candidates_for("nihao").is_empty(), "expected phrase candidates for nihao");
        let r = reduce_ime(&s, Key::Space);
        assert!(r.consume);
        assert!(r.commit.as_deref().is_some_and(|t| !t.is_empty()));
        assert_eq!(r.state.composition, "");
    }

    // --- port of TestReduceIMEEscapeAndEmptyEditing ---
    #[test]
    fn escape_cancels_and_editing_keys_pass_through_when_idle() {
        assert!(
            !reduce_ime(&ImeState::default(), Key::Backspace).consume,
            "Backspace with no composition must pass through"
        );
        assert!(!reduce_ime(&ImeState::default(), Key::Delete).consume);
        assert!(!reduce_ime(&ImeState::default(), Key::Escape).consume);
        let r = reduce_ime(&ImeState { composition: "ni".into(), selected: 0 }, Key::Escape);
        assert!(r.consume && r.state.composition.is_empty(), "Escape should cancel composition");
    }

    #[test]
    fn backspace_and_delete_edit_the_composition_and_reset_the_selection() {
        let s = ImeState { composition: "nihao".into(), selected: 3 };
        let r = reduce_ime(&s, Key::Backspace);
        assert_eq!((r.state.composition.as_str(), r.state.selected), ("niha", 0));
        let r = reduce_ime(&s, Key::Delete);
        assert_eq!((r.state.composition.as_str(), r.state.selected), ("ihao", 0));
    }

    #[test]
    fn digits_pick_candidates_only_while_composing() {
        let idle = ImeState::default();
        assert!(!reduce_ime(&idle, Key::Digit('1')).consume);
        let ni = ImeState { composition: "ni".into(), selected: 0 };
        let r = reduce_ime(&ni, Key::Digit('2'));
        assert!(r.consume);
        assert_eq!(r.commit.as_deref(), Some(candidates_for("ni")[1].text.as_str()));
        assert_eq!(r.state, ImeState::default());
        // zero is never a pick, and a digit past the end of the list is left alone
        assert!(!reduce_ime(&ni, Key::Digit('0')).consume);
        let tiny = ImeState { composition: "nihao".into(), selected: 0 };
        let count = candidates_for("nihao").len();
        if count < 9 {
            let beyond = (b'1' + count as u8) as char;
            assert!(!reduce_ime(&tiny, Key::Digit(beyond)).consume);
        }
    }

    #[test]
    fn arrows_move_the_selection_and_clamp_at_both_ends() {
        let mut s = ImeState { composition: "ni".into(), selected: 0 };
        let len = candidates_for("ni").len();
        assert!(len > 3);
        s = reduce_ime(&s, Key::ArrowUp).state;
        assert_eq!(s.selected, 0, "ArrowUp at the top stays put");
        s = reduce_ime(&s, Key::ArrowDown).state;
        s = reduce_ime(&s, Key::ArrowRight).state;
        assert_eq!(s.selected, 2);
        s = reduce_ime(&s, Key::ArrowLeft).state;
        assert_eq!(s.selected, 1);
        for _ in 0..50 {
            s = reduce_ime(&s, Key::ArrowDown).state;
        }
        assert_eq!(s.selected, len - 1, "selection clamps to the last candidate");
        assert_eq!(s.composition, "ni", "navigation never edits the composition");
    }

    #[test]
    fn space_and_enter_commit_the_selected_candidate() {
        let s = ImeState { composition: "ni".into(), selected: 2 };
        let want = candidates_for("ni")[2].text.clone();
        for key in [Key::Space, Key::Enter] {
            let r = reduce_ime(&s, key);
            assert_eq!(r.commit.as_deref(), Some(want.as_str()));
        }
        // an out-of-range selection falls back to the first candidate
        let s = ImeState { composition: "ni".into(), selected: 999 };
        let r = reduce_ime(&s, Key::Space);
        assert_eq!(r.commit.as_deref(), Some(candidates_for("ni")[0].text.as_str()));
    }

    #[test]
    fn a_composition_without_candidates_commits_as_typed() {
        // not a prefix of any code
        let s = ImeState { composition: "qqqq".into(), selected: 0 };
        assert!(candidates_for("qqqq").is_empty());
        let r = reduce_ime(&s, Key::Space);
        assert_eq!(r.commit.as_deref(), Some("qqqq"));
        // and the arrows have nothing to select, so they belong to the app
        assert!(!reduce_ime(&s, Key::ArrowDown).consume);
    }

    #[test]
    fn unmapped_keys_are_never_consumed() {
        assert_eq!(Key::from_vk(0x70), None, "F1 has no mapping");
        assert_eq!(Key::from_vk(0x09), None, "Tab has no mapping");
        assert_eq!(Key::from_vk('N' as usize), Some(Key::Letter('n')));
        assert_eq!(Key::from_vk('5' as usize), Some(Key::Digit('5')));
        assert_eq!(Key::from_vk(0x20), Some(Key::Space));
        assert_eq!(key_name(0x28), "ArrowDown");
        assert_eq!(key_name('N' as usize), "N");
        assert_eq!(key_name(0x30), "", "zero is not named, matching the Go table");
    }

    #[test]
    fn candidates_are_unique_numbered_and_annotated() {
        let got = candidates_for("ni");
        assert!(got.len() <= MAX_CANDIDATES);
        let mut texts: Vec<_> = got.iter().map(|c| c.text.clone()).collect();
        texts.sort();
        texts.dedup();
        assert_eq!(texts.len(), got.len(), "no duplicate texts");
        for (i, c) in got.iter().enumerate() {
            assert_eq!(c.index, i);
            assert_eq!(c.annotation, "ni");
        }
    }

    #[test]
    fn nihao_has_the_expected_top_candidate() {
        // pinned by the Go test suite (TestCandidateUIKeepsUnicodeCandidatesIntact)
        assert_eq!(candidates_for("nihao")[0].text, "你好");
    }

    #[test]
    fn composition_is_normalized_before_lookup() {
        assert_eq!(candidates_for("Ni Hao"), candidates_for("nihao"));
        assert_eq!(candidates_for("ni'hao"), candidates_for("nihao"));
        assert_eq!(normalize_pinyin("Xi\u{2019}An-X`y\tz"), "xianxyz");
        assert!(candidates_for("").is_empty());
        assert!(candidates_for(" '").is_empty());
    }

    #[test]
    fn segmentation_handles_phrases_that_are_not_in_the_dictionary() {
        let dict = Dictionary::parse("---\n...\n你\tni\t100\n好\thao\t90\n");
        let got = dict.candidates_for("nihao");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, "你好");
        assert_eq!(got[0].annotation, "", "synthesized phrases carry no code");
    }

    #[test]
    fn prefix_fallback_serves_a_composition_still_being_typed() {
        let dict = Dictionary::parse("...\n你\tni\t10\n泥\tnitu\t20\n");
        // "nit" is neither an entry nor segmentable, but starts the code "nitu"
        let got = dict.candidates_for("nit");
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].text.as_str(), got[0].annotation.as_str()), ("泥", "nitu"));
    }

    #[test]
    fn prefix_fallback_ties_are_deterministic() {
        let dict = Dictionary::parse("...\n乙\tbb\t5\n甲\tba\t5\n丙\tbc\t5\n");
        for _ in 0..20 {
            let texts: Vec<_> = dict.candidates_for("b").into_iter().map(|c| c.text).collect();
            assert_eq!(texts, ["甲", "乙", "丙"], "equal weights fall back to code order");
        }
    }

    #[test]
    fn parser_skips_junk_and_orders_by_weight() {
        let src = "name: x\n对\tdui\t1\n...\n# comment\n\n低\tdi\t1\n高\tdi\t9\n中\tdi\t5%\n坏\tD1\t5\nno-tab-line\n空\t\t3\n";
        let dict = Dictionary::parse(src);
        assert_eq!(dict.code_count(), 1, "rows before `...`, bad codes and short rows are dropped");
        let texts: Vec<_> = dict.candidates_for("di").into_iter().map(|c| c.text).collect();
        // 高 (9) first; 低 (1) before 中 (percent weight counts as 0)? no: 1 > 0
        assert_eq!(texts, ["高", "低", "中"]);
    }

    #[test]
    fn the_embedded_dictionary_loads() {
        let dict = dictionary();
        assert!(dict.code_count() > 30_000, "got {} codes", dict.code_count());
        assert!(!dict.is_empty());
    }

    #[test]
    fn non_ascii_composition_does_not_panic() {
        // byte offsets inside a multi-byte char must be skipped, not sliced
        let _ = candidates_for("ni好hao");
        let _ = candidates_for("好");
    }
}
