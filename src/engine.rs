use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate { pub text: String, pub code: String }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key { Character(char), Backspace, Escape, Up, Down, Space, Enter, Digit(u8), Other }
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome { PassThrough, Update(String), Commit(String), Cancel, Consumed }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot { pub composition: String, pub candidates: Vec<Candidate>, pub selected: usize, pub visible: bool }

/// Per-service composition state; COM pointers are deliberately kept outside it.
#[derive(Debug, Clone, Default)]
pub struct Engine { composition: String, candidates: Vec<Candidate>, selected: usize }
impl Engine {
 pub fn new() -> Self { Self::default() }
 pub fn snapshot(&self) -> Snapshot { Snapshot { composition:self.composition.clone(), candidates:self.candidates.clone(), selected:self.selected, visible:!self.composition.is_empty() && !self.candidates.is_empty() } }
 /// Predict consumption without changing composition or selection state.
 pub fn test_key(&self, key: Key) -> bool { if self.composition.is_empty() { return matches!(key,Key::Character(c) if c.is_ascii_alphabetic()); } matches!(key,Key::Character(_) | Key::Backspace | Key::Escape | Key::Up | Key::Down | Key::Space | Key::Enter | Key::Digit(_)) }
 pub fn handle_key(&mut self, key: Key) -> KeyOutcome {
  if self.composition.is_empty() { if let Key::Character(c)=key { if c.is_ascii_alphabetic() { self.composition.push(c.to_ascii_lowercase()); self.refresh(); return KeyOutcome::Update(self.composition.clone()); } } return KeyOutcome::PassThrough; }
  match key {
   Key::Character(c) if c.is_ascii_alphabetic() => { self.composition.push(c.to_ascii_lowercase()); self.refresh(); KeyOutcome::Update(self.composition.clone()) },
   Key::Backspace => { self.composition.pop(); if self.composition.is_empty() { self.clear(); KeyOutcome::Cancel } else { self.refresh(); KeyOutcome::Update(self.composition.clone()) } },
   Key::Escape => { self.clear(); KeyOutcome::Cancel },
   Key::Up => { self.selected=self.selected.saturating_sub(1); KeyOutcome::Consumed },
   Key::Down => { if !self.candidates.is_empty() { self.selected=(self.selected+1).min(self.candidates.len()-1); } KeyOutcome::Consumed },
   Key::Digit(n @ 1..=9) => self.commit(usize::from(n-1)),
   Key::Space | Key::Enter => self.commit(self.selected),
   _ => KeyOutcome::PassThrough,
  }
 }
 fn commit(&mut self,index:usize)->KeyOutcome { if let Some(c)=self.candidates.get(index) { let s=c.text.clone(); self.clear(); KeyOutcome::Commit(s) } else { KeyOutcome::Consumed } }
 fn clear(&mut self) { self.composition.clear(); self.candidates.clear(); self.selected=0; }
 fn refresh(&mut self) { self.candidates=lookup(&self.composition); self.selected=0; }
}

/// Bootstrap lexicon only; full source dictionary and ranking still need porting.
fn lookup(code:&str)->Vec<Candidate> {
 const WORDS:&[(&str,&str)]=&[("ni","你"),("ni","呢"),("ni","泥"),("hao","好"),("hao","号"),("nihao","你好"),("zhong","中"),("guo","国"),("zhongguo","中国"),("shi","是"),("jie","界"),("shijie","世界"),("wo","我"),("men","们"),("women","我们"),("de","的"),("ren","人"),("zai","在"),("ma","吗")];
 let mut out:Vec<Candidate>=Vec::new(); for (prefix,word) in WORDS { if *prefix==code || prefix.starts_with(code) { if !out.iter().any(|c|c.text==*word) { out.push(Candidate{text:(*word).to_owned(),code:(*prefix).to_owned()}); } } } out
}

#[cfg(test)] mod tests { use super::*;
 #[test] fn prediction_is_pure(){let mut e=Engine::new();assert!(e.test_key(Key::Character('n')));assert_eq!(e.snapshot().composition,"");e.handle_key(Key::Character('n'));let before=e.snapshot();e.test_key(Key::Character('i'));assert_eq!(e.snapshot(),before);}
 #[test] fn compose_commit(){let mut e=Engine::new();e.handle_key(Key::Character('n'));e.handle_key(Key::Character('i'));assert_eq!(e.snapshot().composition,"ni");assert_eq!(e.handle_key(Key::Space),KeyOutcome::Commit("你".into()));assert_eq!(e.snapshot().composition,"");}
 #[test] fn escape_cancels(){let mut e=Engine::new();e.handle_key(Key::Character('a'));assert_eq!(e.handle_key(Key::Escape),KeyOutcome::Cancel);assert_eq!(e.snapshot().composition,"");}
}
