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

    /// Predict whether a key would be consumed, without changing any state.
    ///
    /// Keep this decision aligned with `handle_key`: TSF may call its test-key
    /// callback before the real key callback, so prediction must be side-effect
    /// free and must not claim keys that the reducer will pass through.
    pub fn test_key(&self, key: Key) -> bool {
        match (self.composition.is_empty(), key) {
            (true, Key::Character(c)) => c.is_ascii_alphabetic(),
            (false, Key::Character(c)) => c.is_ascii_alphabetic(),
            (false, Key::Backspace | Key::Escape | Key::Up | Key::Down | Key::Space | Key::Enter) => true,
            (false, Key::Digit(1..=9)) => true,
            _ => false,
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
            Key::Up => {
                self.selected = self.selected.saturating_sub(1);
                KeyOutcome::Consumed
            }
            Key::Down => {
                if !self.candidates.is_empty() {
                    self.selected = (self.selected + 1).min(self.candidates.len() - 1);
                }
                KeyOutcome::Consumed
            }
            Key::Digit(n @ 1..=9) => self.commit(usize::from(n - 1)),
            Key::Space | Key::Enter => self.commit(self.selected),
            _ => KeyOutcome::PassThrough,
        }
    }

    fn commit(&mut self, index: usize) -> KeyOutcome {
        if let Some(candidate) = self.candidates.get(index) {
            let text = candidate.text.clone();
            self.clear();
            KeyOutcome::Commit(text)
        } else {
            KeyOutcome::Consumed
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

    let mut candidates = Vec::new();
    for (pinyin, word) in WORDS {
        if (*pinyin == code || pinyin.starts_with(code))
            && !candidates.iter().any(|candidate: &Candidate| candidate.text == *word)
        {
            candidates.push(Candidate {
                text: (*word).to_owned(),
                code: (*pinyin).to_owned(),
            });
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prediction_is_pure() {
        let mut engine = Engine::new();
        assert!(engine.test_key(Key::Character('n')));
        assert_eq!(engine.snapshot().composition, "");

        engine.handle_key(Key::Character('n'));
        let before = engine.snapshot();
        for key in [Key::Character('i'), Key::Backspace, Key::Down, Key::Digit(1)] {
            let _ = engine.test_key(key);
            assert_eq!(engine.snapshot(), before, "prediction mutated state for {key:?}");
        }
    }

    #[test]
    fn prediction_matches_reducer_consumption() {
        let keys = [
            Key::Character('a'),
            Key::Character('中'),
            Key::Character('-'),
            Key::Backspace,
            Key::Escape,
            Key::Up,
            Key::Down,
            Key::Space,
            Key::Enter,
            Key::Digit(0),
            Key::Digit(1),
            Key::Digit(9),
            Key::Digit(10),
            Key::Other,
        ];

        // Check both the empty and composing states. Clone before reduction so
        // the prediction and reducer observe exactly the same starting state.
        for composing in [false, true] {
            let mut initial = Engine::new();
            if composing {
                initial.handle_key(Key::Character('n'));
                initial.handle_key(Key::Character('i'));
            }

            for key in keys {
                let mut reduced = initial.clone();
                let predicted = initial.test_key(key);
                let consumed = !matches!(reduced.handle_key(key), KeyOutcome::PassThrough);
                assert_eq!(predicted, consumed, "mismatch for {key:?}, composing={composing}");
            }
        }
    }

    #[test]
    fn compose_commit() {
        let mut engine = Engine::new();
        engine.handle_key(Key::Character('n'));
        engine.handle_key(Key::Character('i'));
        assert_eq!(engine.snapshot().composition, "ni");
        assert_eq!(
            engine.handle_key(Key::Space),
            KeyOutcome::Commit("你".into())
        );
        assert_eq!(engine.snapshot().composition, "");
    }

    #[test]
    fn escape_cancels() {
        let mut engine = Engine::new();
        engine.handle_key(Key::Character('a'));
        assert_eq!(engine.handle_key(Key::Escape), KeyOutcome::Cancel);
        assert_eq!(engine.snapshot().composition, "");
    }

    #[test]
    fn engine_instances_do_not_share_composition() {
        let mut first = Engine::new();
        let second = Engine::new();

        first.handle_key(Key::Character('n'));
        first.handle_key(Key::Character('i'));

        assert_eq!(first.snapshot().composition, "ni");
        assert_eq!(second.snapshot().composition, "");
        assert!(second.snapshot().candidates.is_empty());
    }
}
