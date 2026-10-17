use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub text: String,
    pub code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Character(char),
    Backspace,
    Escape,
    Up,
    Down,
    Space,
    Enter,
    Digit(u8),
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome {
    PassThrough,
    Update(String),
    Commit(String),
    Cancel,
    Consumed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub composition: String,
    pub candidates: Vec<Candidate>,
    pub selected: usize,
    pub visible: bool,
}

/// Per-service composition state; COM pointers are deliberately kept outside it.
#[derive(Debug, Clone, Default)]
pub struct Engine {
    composition: String,
    candidates: Vec<Candidate>,
    selected: usize,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            composition: self.composition.clone(),
            candidates: self.candidates.clone(),
            selected: self.selected,
            visible: !self.composition.is_empty() && !self.candidates.is_empty(),
        }
    }

    /// Predict whether a key will be consumed, without changing any state.
    ///
    /// Keep this decision in sync with `handle_key`: TSF calls the test phase
    /// before the handling phase and relies on the test being side-effect-free.
    pub fn test_key(&self, key: Key) -> bool {
        if self.composition.is_empty() {
            return matches!(key, Key::Character(c) if c.is_ascii_alphabetic());
        }

        match key {
            Key::Character(c) => c.is_ascii_alphabetic(),
            Key::Backspace
            | Key::Escape
            | Key::Space
            | Key::Enter => true,
            Key::Up | Key::Down => !self.candidates.is_empty(),
            Key::Digit(n @ 1..=9) => usize::from(n - 1) < self.candidates.len(),
            Key::Digit(_) | Key::Other => false,
        }
    }

    pub fn handle_key(&mut self, key: Key) -> KeyOutcome {
        if self.composition.is_empty() {
            if let Key::Character(c) = key {
                if c.is_ascii_alphabetic() {
                    self.composition.push(c.to_ascii_lowercase());
                    self.refresh();
                    return KeyOutcome::Update(self.composition.clone());
                }
            }
            return KeyOutcome::PassThrough;
        }

        match key {
            Key::Character(c) if c.is_ascii_alphabetic() => {
                self.composition.push(c.to_ascii_lowercase());
                self.refresh();
                KeyOutcome::Update(self.composition.clone())
            }
            Key::Backspace => {
                self.composition.pop();
                if self.composition.is_empty() {
                    self.clear();
                    KeyOutcome::Cancel
                } else {
                    self.refresh();
                    KeyOutcome::Update(self.composition.clone())
                }
            }
            Key::Escape => {
                self.clear();
                KeyOutcome::Cancel
            }
            Key::Up if !self.candidates.is_empty() => {
                self.selected = self.selected.saturating_sub(1);
                KeyOutcome::Consumed
            }
            Key::Down if !self.candidates.is_empty() => {
                self.selected = (self.selected + 1).min(self.candidates.len() - 1);
                KeyOutcome::Consumed
            }
            Key::Digit(n @ 1..=9) => {
                let index = usize::from(n - 1);
                if index < self.candidates.len() {
                    self.commit(index)
                } else {
                    KeyOutcome::PassThrough
                }
            }
            Key::Space | Key::Enter => {
                if self.candidates.is_empty() {
                    let raw = self.composition.clone();
                    self.clear();
                    KeyOutcome::Commit(raw)
                } else {
                    self.commit(self.selected)
                }
            }
            _ => KeyOutcome::PassThrough,
        }
    }

    fn commit(&mut self, index: usize) -> KeyOutcome {
        if let Some(candidate) = self.candidates.get(index) {
            let text = candidate.text.clone();
            self.clear();
            KeyOutcome::Commit(text)
        } else {
            KeyOutcome::PassThrough
        }
    }

    fn clear(&mut self) {
        self.composition.clear();
        self.candidates.clear();
        self.selected = 0;
    }

    fn refresh(&mut self) {
        self.candidates = lookup(&self.composition);
        self.selected = 0;
    }
}

/// Bootstrap lexicon only; full source dictionary and ranking still need porting.
fn lookup(code: &str) -> Vec<Candidate> {
    const WORDS: &[(&str, &str)] = &[
        ("ni", "你"),
        ("ni", "呢"),
        ("ni", "泥"),
        ("hao", "好"),
        ("hao", "号"),
        ("nihao", "你好"),
        ("zhong", "中"),
        ("guo", "国"),
        ("zhongguo", "中国"),
        ("shi", "是"),
        ("jie", "界"),
        ("shijie", "世界"),
        ("wo", "我"),
        ("men", "们"),
        ("women", "我们"),
        ("de", "的"),
        ("ren", "人"),
        ("zai", "在"),
        ("ma", "吗"),
    ];

    let mut candidates: Vec<Candidate> = Vec::new();
    for (prefix, word) in WORDS {
        if (*prefix == code || prefix.starts_with(code))
            && !candidates.iter().any(|candidate| candidate.text == *word)
        {
            candidates.push(Candidate {
                text: (*word).to_owned(),
                code: (*prefix).to_owned(),
            });
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_text(engine: &mut Engine, text: &str) {
        for c in text.chars() {
            assert!(engine.test_key(Key::Character(c)));
            assert!(matches!(
                engine.handle_key(Key::Character(c)),
                KeyOutcome::Update(_)
            ));
        }
    }

    #[test]
    fn prediction_is_pure() {
        let mut engine = Engine::new();
        assert!(engine.test_key(Key::Character('n')));
        assert_eq!(engine.snapshot().composition, "");

        engine.handle_key(Key::Character('n'));
        let before = engine.snapshot();
        assert!(engine.test_key(Key::Character('i')));
        assert_eq!(engine.snapshot(), before);
    }

    #[test]
    fn test_key_matches_handling_for_invalid_keys() {
        let mut engine = Engine::new();
        type_text(&mut engine, "ni");

        for key in [
            Key::Character('!'),
            Key::Character('中'),
            Key::Digit(0),
            Key::Digit(10),
            Key::Other,
        ] {
            let before = engine.snapshot();
            assert!(!engine.test_key(key), "unexpected prediction for {key:?}");
            assert_eq!(engine.handle_key(key), KeyOutcome::PassThrough);
            assert_eq!(engine.snapshot(), before, "{key:?} changed engine state");
        }
    }

    #[test]
    fn test_key_matches_handling_for_supported_keys() {
        let mut engine = Engine::new();
        type_text(&mut engine, "ni");

        for key in [
            Key::Character('a'),
            Key::Backspace,
            Key::Escape,
            Key::Up,
            Key::Down,
            Key::Space,
            Key::Enter,
            Key::Digit(1),
            Key::Digit(3),
        ] {
            assert!(engine.test_key(key), "did not predict {key:?}");
        }

        let mut no_candidates = Engine::new();
        type_text(&mut no_candidates, "a");
        assert!(!no_candidates.test_key(Key::Up));
        assert!(!no_candidates.test_key(Key::Digit(1)));
        assert_eq!(no_candidates.handle_key(Key::Up), KeyOutcome::PassThrough);
        assert_eq!(no_candidates.handle_key(Key::Digit(1)), KeyOutcome::PassThrough);
        assert_eq!(
            no_candidates.handle_key(Key::Enter),
            KeyOutcome::Commit("a".into())
        );
    }

    #[test]
    fn compose_and_commit_selected_candidate() {
        let mut engine = Engine::new();
        type_text(&mut engine, "ni");
        assert_eq!(engine.snapshot().composition, "ni");
        assert_eq!(
            engine.handle_key(Key::Space),
            KeyOutcome::Commit("你".into())
        );
        assert_eq!(engine.snapshot().composition, "");
        assert!(engine.snapshot().candidates.is_empty());
    }

    #[test]
    fn digit_selects_candidate_by_one_based_index() {
        let mut engine = Engine::new();
        type_text(&mut engine, "ni");
        assert_eq!(
            engine.handle_key(Key::Digit(2)),
            KeyOutcome::Commit("呢".into())
        );
    }

    #[test]
    fn backspace_clears_last_character_and_escape_cancels() {
        let mut engine = Engine::new();
        type_text(&mut engine, "ni");
        assert_eq!(
            engine.handle_key(Key::Backspace),
            KeyOutcome::Update("n".into())
        );
        assert_eq!(engine.handle_key(Key::Backspace), KeyOutcome::Cancel);
        assert_eq!(engine.snapshot().composition, "");

        type_text(&mut engine, "a");
        assert_eq!(engine.handle_key(Key::Escape), KeyOutcome::Cancel);
        assert_eq!(engine.snapshot().composition, "");
    }

    #[test]
    fn each_engine_has_independent_composition_state() {
        let mut first = Engine::new();
        let second = Engine::new();
        first.handle_key(Key::Character('n'));

        assert_eq!(first.snapshot().composition, "n");
        assert_eq!(second.snapshot().composition, "");
    }
}
