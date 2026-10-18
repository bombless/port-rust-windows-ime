//! Per-service state shared by the key path, without any COM in it.
//!
//! The Go `tipImpl` mixed COM plumbing with input-method state. The state half
//! lives here so that the whole key pipeline (key in, reply out, candidate model
//! updated) can be exercised on any host; the COM half is in `win`.
//!
//! A [`Session`] belongs to one text-service instance and is only touched from
//! the TSF thread (the service is apartment threaded), so interior mutability is
//! `Cell`/`RefCell`. Nothing here holds a borrow across a call into TSF.

use std::cell::{Cell, RefCell};

use crate::candidate_ui::{CandidateUiModel, Presenter};
use crate::diag::text_for_log;
use crate::engine::{candidates_for, dictionary, reduce_ime, Candidate, ImeState, Key};

/// What the engine decided for one key, in a form the edit session and the
/// candidate UI can consume. (The Go port passed this around as a JSON string
/// and re-parsed it by hand; a struct says the same thing without the parsing.)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    /// Whether the engine kept the key.
    pub consume: bool,
    pub composition: String,
    pub candidates: Vec<Candidate>,
    pub selected: usize,
    /// Text to commit into the document, which ends the composition.
    pub commit: Option<String>,
}

/// What a reply asks of the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditAction {
    /// Nothing to draw and nothing to commit: take down what is on screen.
    EndComposition,
    /// Write `text` into the composition; `commit` also ends it.
    Write { text: String, commit: bool },
}

impl Reply {
    pub fn edit_action(&self) -> EditAction {
        let commit = self.commit.is_some();
        let text = self.commit.as_deref().unwrap_or(&self.composition);
        if text.is_empty() && !commit {
            EditAction::EndComposition
        } else {
            EditAction::Write { text: text.to_owned(), commit }
        }
    }

    /// The fields that decide behaviour, for the log: whether the key was eaten,
    /// what is being composed, which candidate is selected. User text is
    /// redacted unless text logging is on (see [`text_for_log`]).
    pub fn describe(&self) -> String {
        let mut out = format!("consume={} composition={}", self.consume, text_for_log(&self.composition));
        if let Some(commit) = &self.commit {
            out.push_str(&format!(" commit={}", text_for_log(commit)));
        }
        out.push_str(&format!(" selected={} candidates={}", self.selected, self.candidates.len()));
        if std::env::var_os(crate::diag::LOG_TEXT_ENV).is_some_and(|v| v == "1") {
            out.push_str(&format!(" raw={self:?}"));
        }
        out
    }
}

/// Whether a key callback should report the key as eaten. A key whose reduction
/// produced no reply must stay with the application, the same rule the C++
/// service applied with `*eaten = ok && consume`.
pub fn key_decision(reply: Option<&Reply>) -> bool {
    reply.is_some_and(|r| r.consume)
}

/// Virtual-key codes of the modifiers that turn a key into a shortcut: Ctrl, Alt
/// and either Windows key.
pub const SHORTCUT_MODIFIERS: [usize; 4] = [0x11, 0x12, 0x5B, 0x5C];

/// Whether `vk` is part of a shortcut (Ctrl+C, Alt+Tab, Win+Space, ...), which
/// belongs to the application or to Windows and must reach it untouched.
///
/// The Go service only stepped aside for Win+Space, so with it active Ctrl+C,
/// Ctrl+V and every other Ctrl/Alt letter shortcut was eaten and typed as pinyin.
/// A previous Rust port of this project hit that on a real machine and fixed it
/// the same way. `key_down` asks whether a virtual key is currently held; it is
/// only consulted for keys the engine handles, so ordinary typing of keys it
/// ignores never pays for the lookup. Shift is deliberately not a shortcut
/// modifier: Shift+letter is how capitals are typed.
pub fn is_shortcut(vk: usize, key_down: impl Fn(usize) -> bool) -> bool {
    Key::from_vk(vk).is_some() && SHORTCUT_MODIFIERS.iter().any(|&modifier| key_down(modifier))
}

/// How many UTF-16 code units `text` occupies in the document, which is the
/// count `ITfRange::SetText` takes. It is neither the byte length nor the char
/// count: a Chinese character is one unit and takes three bytes, an astral
/// character two units. `None` means the text cannot be encoded for TSF (an
/// embedded NUL), so no range may be sized from it.
pub fn utf16_units(text: &str) -> Option<usize> {
    if text.contains('\0') {
        None
    } else {
        Some(text.encode_utf16().count())
    }
}

/// Number of dictionary codes, for the activation log line.
pub fn dictionary_summary() -> String {
    format!("embedded pinyin entries={}", dictionary().code_count())
}

pub struct Session {
    ime: RefCell<ImeState>,
    pub candidate_ui: CandidateUiModel,
    /// The text the composition range was last given to write. The range does
    /// not grow with it, so the next write has to replace it by hand, and a
    /// cancel has to clear it. Empty means a composition writing for the first
    /// time, which must still be allowed to overwrite a selection.
    written: RefCell<String>,
    /// Set when the application ended the composition itself.
    terminated: Cell<bool>,
}

impl Session {
    pub fn new(presenter: Box<dyn Presenter>) -> Session {
        Session {
            ime: RefCell::new(ImeState::default()),
            candidate_ui: CandidateUiModel::new(presenter),
            written: RefCell::new(String::new()),
            terminated: Cell::new(false),
        }
    }

    /// A session with no candidate window (tests).
    pub fn headless() -> Session {
        Session::new(Box::new(crate::candidate_ui::NullPresenter))
    }

    pub fn ime_state(&self) -> ImeState {
        self.ime.borrow().clone()
    }

    pub fn set_ime_state(&self, state: ImeState) {
        *self.ime.borrow_mut() = state;
    }

    /// `OnTestKeyDown`: would the engine keep this key? Never changes state:
    /// TSF asks ahead of `OnKeyDown` for the same key, and applying it twice
    /// would double the pinyin.
    pub fn predict_consume(&self, vk: usize) -> bool {
        match Key::from_vk(vk) {
            Some(key) => reduce_ime(&self.ime.borrow(), key).consume,
            None => false,
        }
    }

    /// `OnKeyDown`: runs the engine and keeps its new state. `None` for a key
    /// with no mapping, which belongs to the application.
    pub fn apply_key(&self, vk: usize) -> Option<Reply> {
        let key = Key::from_vk(vk)?;
        let before = self.ime.borrow().clone();
        let reduction = reduce_ime(&before, key);
        *self.ime.borrow_mut() = reduction.state.clone();
        let candidates = if !reduction.consume {
            Vec::new()
        } else if reduction.commit.is_some() {
            // A commit empties the state, but the list that was on screen is the
            // one the commit chose from.
            candidates_for(&before.composition)
        } else {
            candidates_for(&reduction.state.composition)
        };
        Some(Reply {
            consume: reduction.consume,
            composition: reduction.state.composition,
            candidates,
            selected: reduction.state.selected,
            commit: reduction.commit,
        })
    }

    /// Drives the candidate model from one reply. A commit, an empty composition
    /// or a candidate-less reply all hide the list.
    pub fn notify_candidates(&self, reply: &Reply, caret: [i32; 4], dpi: u32) {
        if reply.commit.is_some() || reply.composition.is_empty() || reply.candidates.is_empty() {
            self.candidate_ui.hide();
            return;
        }
        self.candidate_ui.update(&reply.composition, &reply.candidates, reply.selected, caret, dpi);
    }

    /// Puts the list back when focus returns to the application that still owns
    /// the composition. A composition that is no longer on screen owns no list.
    pub fn restore_candidates(&self, composition_alive: bool, has_context: bool) {
        if !composition_alive || !has_context {
            return;
        }
        if self.candidate_ui.snapshot().composition.is_empty() {
            return;
        }
        crate::tip_log!("StrategyTip restoring in-process candidate model after focus");
        self.candidate_ui.resume();
    }

    pub fn written_composition(&self) -> String {
        self.written.borrow().clone()
    }

    pub fn store_written_composition(&self, text: &str) {
        *self.written.borrow_mut() = text.to_owned();
    }

    /// The composition this text belonged to is gone, so the next one starts
    /// with nothing to replace.
    pub fn clear_written(&self) {
        self.written.borrow_mut().clear();
    }

    /// `OnCompositionTerminated` only records the fact (see the COM side).
    pub fn mark_terminated(&self) {
        self.terminated.set(true);
    }

    /// Reads and clears the "application ended the composition" flag for the
    /// reply about to be written. A reply that carries composition text starts
    /// or continues one, so it clears the flag instead (a dead composition is
    /// noticed by the edit session itself). Returns true if the stale
    /// composition handle must be dropped.
    pub fn take_terminated_for(&self, reply_composition: &str) -> bool {
        if !reply_composition.is_empty() {
            self.terminated.set(false);
        }
        self.terminated.replace(false)
    }

    /// `Deactivate`: forget the engine state and take the list down.
    pub fn deactivate(&self) {
        *self.ime.borrow_mut() = ImeState::default();
        self.candidate_ui.set_enabled(false);
        self.candidate_ui.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::test_support::*;

    const CARET: [i32; 4] = [10, 20, 30, 40];

    fn type_keys(session: &Session, keys: &[usize]) {
        for &vk in keys {
            let reply = session.apply_key(vk).unwrap_or_else(|| panic!("no reply for {vk:#x}"));
            session.notify_candidates(&reply, CARET, 96);
        }
    }

    const NIHAO: [usize; 5] = ['N' as usize, 'I' as usize, 'H' as usize, 'A' as usize, 'O' as usize];

    // port of TestKeyDecisionFollowsReplyConsume
    #[test]
    fn key_decision_follows_reply_consume() {
        let reply = |consume| Reply { consume, composition: "n".into(), candidates: vec![], selected: 0, commit: None };
        assert!(key_decision(Some(&reply(true))), "consume true is eaten");
        assert!(!key_decision(Some(&reply(false))), "consume false passes through");
        assert!(!key_decision(None), "a reduction that produced nothing must not eat the key");
    }

    // port of TestTestKeyPredictionDoesNotMutateState
    #[test]
    fn test_key_prediction_does_not_mutate_state() {
        let session = Session::headless();
        session.set_ime_state(ImeState { composition: "ni".into(), selected: 0 });
        assert!(session.predict_consume('1' as usize), "first candidate digit should be predicted as consumed");
        assert_eq!(session.ime_state(), ImeState { composition: "ni".into(), selected: 0 });
        assert!(session.predict_consume('N' as usize));
        assert_eq!(session.ime_state().composition, "ni", "predicting a letter must not append it");
        assert!(!session.predict_consume(0x70), "F1 has no mapping");
    }

    // port of TestKeySinkEatDecisionFollowsTheReducer (the COM-free half)
    #[test]
    fn eat_decisions_follow_the_reducer_over_a_whole_composition() {
        let session = Session::headless();
        for &letter in &NIHAO {
            assert!(session.predict_consume(letter));
            assert!(key_decision(session.apply_key(letter).as_ref()));
        }
        assert_eq!(session.ime_state().composition, "nihao");

        // Escape ends the composition; once there is none it belongs to the app.
        assert!(key_decision(session.apply_key(0x1B).as_ref()), "Escape is eaten while composing");
        assert_eq!(session.ime_state().composition, "");
        assert!(!key_decision(session.apply_key(0x1B).as_ref()), "Escape with no composition is the app's");

        // "ni" has several candidates, so the selection has room to move.
        type_keys(&session, &['N' as usize, 'I' as usize]);
        assert!(key_decision(session.apply_key(0x28).as_ref()));
        assert_eq!(session.ime_state().selected, 1);
        assert_eq!(session.ime_state().composition, "ni", "navigation must not change the composition");
        assert!(key_decision(session.apply_key(0x26).as_ref()));
        assert_eq!(session.ime_state().selected, 0);

        // A numeric pick commits the chosen candidate and clears everything.
        let reply = session.apply_key('2' as usize).unwrap();
        assert!(reply.consume && reply.commit.is_some());
        assert_eq!(session.ime_state(), ImeState::default());

        // A key with no mapping never reaches the reducer.
        assert!(!session.predict_consume(0x70));
        assert!(session.apply_key(0x70).is_none());
    }

    // port of TestOnTestKeyDownDoesNotMutateComposition
    #[test]
    fn prediction_then_application_adds_the_letter_once() {
        let session = Session::headless();
        assert!(session.predict_consume('N' as usize));
        assert_eq!(session.ime_state().composition, "");
        assert!(key_decision(session.apply_key('N' as usize).as_ref()));
        assert_eq!(session.ime_state().composition, "n");
    }

    #[test]
    fn a_commit_reply_carries_the_list_it_chose_from() {
        let session = Session::headless();
        type_keys(&session, &NIHAO);
        let reply = session.apply_key(0x20).unwrap();
        assert_eq!(reply.commit.as_deref(), Some("你好"));
        assert_eq!(reply.composition, "");
        assert!(!reply.candidates.is_empty(), "the commit reply keeps the list that was on screen");
        assert_eq!(reply.candidates[0].text, "你好");
    }

    #[test]
    fn edit_actions_follow_the_reply() {
        let base = Reply { consume: true, composition: String::new(), candidates: vec![], selected: 0, commit: None };
        assert_eq!(base.edit_action(), EditAction::EndComposition);
        let drawing = Reply { composition: "ni".into(), ..base.clone() };
        assert_eq!(drawing.edit_action(), EditAction::Write { text: "ni".into(), commit: false });
        let commit = Reply { commit: Some("你".into()), ..base.clone() };
        assert_eq!(commit.edit_action(), EditAction::Write { text: "你".into(), commit: true });
        // an empty commit is still a commit, not a cancel
        let empty_commit = Reply { commit: Some(String::new()), ..base };
        assert_eq!(empty_commit.edit_action(), EditAction::Write { text: String::new(), commit: true });
    }

    // port of TestNotifyCandidatesFollowsTheReducerReply
    #[test]
    fn candidate_model_follows_the_reducer() {
        let session = Session::headless();
        type_keys(&session, &NIHAO);
        let s = session.candidate_ui.snapshot();
        assert!(s.visible, "the list must be visible after typing nihao: {s:?}");
        assert_eq!(s.composition, "nihao");
        assert!(!s.candidates.is_empty(), "nihao must produce candidates");
        assert_eq!(s.caret, CARET);
        assert_ne!(s.dpi, 0, "the caret DPI must never be reported as 0");

        // Backspace regenerates the list rather than hiding it, as long as
        // candidates remain.
        type_keys(&session, &[0x08]);
        let s = session.candidate_ui.snapshot();
        assert!(s.visible && s.composition == "niha", "{s:?}");
    }

    // port of TestNotifyCandidatesTracksArrowSelection
    #[test]
    fn candidate_model_tracks_arrow_selection() {
        let session = Session::headless();
        type_keys(&session, &['N' as usize, 'I' as usize]);
        assert_eq!(session.candidate_ui.snapshot().selected, 0);
        type_keys(&session, &[0x28, 0x28]);
        let s = session.candidate_ui.snapshot();
        assert_eq!(s.selected, 2);
        assert_eq!(s.composition, "ni", "navigation must not change the composition");
        // Selection is clamped to the list, so it can never point past the end.
        type_keys(&session, &[0x28; 20]);
        let s = session.candidate_ui.snapshot();
        assert!(s.selected < s.candidates.len(), "selected {} of {}", s.selected, s.candidates.len());
    }

    // port of TestNotifyCandidatesHidesOnCommitAndCancel
    #[test]
    fn escape_space_enter_and_digit_all_take_the_list_down() {
        for (name, last) in [("escape", 0x1B), ("space", 0x20), ("numeric pick", '1' as usize), ("enter", 0x0D)] {
            let session = Session::headless();
            type_keys(&session, &NIHAO);
            assert!(session.candidate_ui.snapshot().visible);
            type_keys(&session, &[last]);
            let s = session.candidate_ui.snapshot();
            assert!(
                !s.visible && s.composition.is_empty() && s.candidates.is_empty(),
                "{name} must leave no list behind: {s:?}"
            );
        }
    }

    // The old "malformed reply" test guarded the hand-rolled JSON; a typed reply
    // cannot be malformed. What it protected still holds: a reply that offers
    // nothing to show must not leave the previous list on screen.
    #[test]
    fn a_reply_with_nothing_to_show_hides_the_previous_list() {
        let session = Session::headless();
        type_keys(&session, &['N' as usize]);
        assert!(session.candidate_ui.snapshot().visible);
        let bare = Reply { consume: true, composition: "x".into(), candidates: vec![], selected: 0, commit: None };
        session.notify_candidates(&bare, CARET, 96);
        assert!(!session.candidate_ui.snapshot().visible);
    }

    // port of TestNotifyCandidatesConsecutiveKeysLeaveTheLastState
    #[test]
    fn consecutive_keys_leave_exactly_the_last_state() {
        let session = Session::headless();
        type_keys(&session, &NIHAO);
        let s = session.candidate_ui.snapshot();
        assert_eq!(s.composition, session.ime_state().composition);
        let want = candidates_for(&session.ime_state().composition);
        assert_eq!(s.candidates, want, "the list is what the engine regenerates from its current state");
    }

    // port of TestCandidateUIKeepsUnicodeCandidatesIntact
    #[test]
    fn unicode_candidates_survive_the_whole_path() {
        let session = Session::headless();
        type_keys(&session, &NIHAO);
        let s = session.candidate_ui.snapshot();
        assert!(!s.candidates.is_empty());
        for (i, c) in s.candidates.iter().enumerate() {
            assert!(!c.text.is_empty(), "candidate {i} has no text");
            assert!(!c.text.contains('\u{FFFD}'), "candidate {i} ({:?}) was mangled", c.text);
        }
        assert_eq!(s.candidates[0].text, "你好");
        assert_eq!(s.candidates[0].annotation, "nihao");
    }

    // port of TestRestoreCandidatesIgnoresAServiceWithNoComposition
    #[test]
    fn restore_ignores_a_service_with_no_composition() {
        let session = Session::headless();
        session.candidate_ui.update(
            "ni",
            &[Candidate { text: "你".into(), index: 0, annotation: String::new() }],
            0,
            CARET,
            96,
        );
        session.candidate_ui.suspend();
        // no composition on screen: a remembered list alone must not come back,
        // because no keystroke could consume a pick from it
        session.restore_candidates(false, false);
        session.restore_candidates(false, true);
        session.restore_candidates(true, false);
        let s = session.candidate_ui.snapshot();
        assert!(!s.visible, "restore must not touch the model: {s:?}");
        assert_eq!(s.composition, "ni");
    }

    // port of TestRestoreCandidatesIgnoresNothingRemembered
    #[test]
    fn restore_ignores_nothing_remembered() {
        let session = Session::headless();
        session.restore_candidates(true, true);
        assert!(!session.candidate_ui.snapshot().visible, "a service that never showed a list stays hidden on focus");
    }

    // port of TestRestoreCandidatesPutsBackASuspendedList
    #[test]
    fn restore_puts_back_a_suspended_list() {
        let session = Session::headless();
        let two = [
            Candidate { text: "你好".into(), index: 0, annotation: String::new() },
            Candidate { text: "尼豪".into(), index: 1, annotation: String::new() },
        ];
        session.candidate_ui.update("nihao", &two, 1, CARET, 96);
        session.candidate_ui.suspend();
        assert!(!session.candidate_ui.snapshot().visible, "suspend must hide the list");
        session.restore_candidates(true, true);
        let s = session.candidate_ui.snapshot();
        assert!(s.visible, "restore must show the list again");
        assert!(
            s.composition == "nihao" && s.selected == 1 && s.candidates.len() == 2,
            "the restored list lost state: {s:?}"
        );
    }

    // Supersedes the Go TestWinSpaceBypassOnlyClaimsSpaceWhileWinIsHeld: Win+Space
    // is now one case of "any shortcut modifier held".
    #[test]
    fn shortcuts_are_left_to_the_application() {
        let held = |wanted: usize| move |vk: usize| vk == wanted;
        for (name, modifier) in [("Ctrl", 0x11), ("Alt", 0x12), ("left Win", 0x5B), ("right Win", 0x5C)] {
            for key in ['C' as usize, 'V' as usize, 0x20, 0x08, 0x0D, '1' as usize, 0x28] {
                assert!(is_shortcut(key, held(modifier)), "{name}+0x{key:x} is a shortcut");
            }
        }
        assert!(!is_shortcut('C' as usize, |_| false), "a bare letter is typing");
        assert!(!is_shortcut('C' as usize, held(0x10)), "Shift+letter types a capital, it is not a shortcut");
        assert!(
            !is_shortcut(0x70, |_| panic!("unmapped keys must not consult the keyboard")),
            "keys the engine ignores never need the lookup"
        );
    }

    // port of TestUtf16UnitsCountsUTF16CodeUnits
    #[test]
    fn utf16_units_counts_code_units() {
        for (name, text, want) in [
            ("empty", "", 0),
            ("ascii pinyin", "ni", 2),
            ("one chinese character", "\u{4f60}", 1),
            ("mixed commit", "\u{4f60}ni", 3),
            ("astral plane needs a surrogate pair", "\u{1f600}", 2),
        ] {
            assert_eq!(utf16_units(text), Some(want), "{name}");
        }
        // an embedded NUL cannot be encoded for TSF, so no range may be sized from it
        assert_eq!(utf16_units("ni\0"), None);
    }

    // port of TestWrittenCompositionStartsEmptyAndFollowsEachWrite
    #[test]
    fn written_composition_starts_empty_and_follows_each_write() {
        let session = Session::headless();
        assert_eq!(session.written_composition(), "");
        session.store_written_composition("n");
        assert_eq!(session.written_composition(), "n");
        // Ending or dropping the composition must clear it, otherwise the next
        // composition would replace text that no longer belongs to it.
        session.clear_written();
        assert_eq!(session.written_composition(), "");
    }

    #[test]
    fn terminated_flag_is_only_honoured_for_replies_without_composition_text() {
        let session = Session::headless();
        session.mark_terminated();
        assert!(!session.take_terminated_for("ni"), "a reply that draws text clears the flag instead");
        assert!(!session.take_terminated_for(""), "and the flag stays cleared");
        session.mark_terminated();
        assert!(session.take_terminated_for(""), "a commit/cancel reply drops the stale handle");
        assert!(!session.take_terminated_for(""), "the flag is read once");
    }

    #[test]
    fn deactivate_forgets_the_state_and_hides_the_list() {
        let session = Session::headless();
        type_keys(&session, &NIHAO);
        session.deactivate();
        assert_eq!(session.ime_state(), ImeState::default());
        assert!(!session.candidate_ui.snapshot().visible);
    }

    // port of TestDescribeReplyExtractsDecisionFields / KeepsCommitAndFullPayload
    #[test]
    fn describe_reports_the_decision_fields() {
        let _guard = env_lock();
        std::env::remove_var(crate::diag::LOG_TEXT_ENV);
        let reply = Reply {
            consume: true,
            composition: "a".into(),
            candidates: vec![
                Candidate { text: "啊".into(), index: 0, annotation: "a".into() },
                Candidate { text: "阿".into(), index: 1, annotation: "a".into() },
            ],
            selected: 0,
            commit: None,
        };
        let text = reply.describe();
        for want in ["consume=true", "composition=<1 chars>", "candidates=2", "selected=0"] {
            assert!(text.contains(want), "describe missing {want:?} in {text:?}");
        }
        assert!(!text.contains('啊') && !text.contains("raw="), "candidate text must not leak: {text:?}");

        let commit = Reply { commit: Some("啊".into()), composition: String::new(), ..reply.clone() };
        assert!(commit.describe().contains("commit=<1 chars>"), "{}", commit.describe());

        std::env::set_var(crate::diag::LOG_TEXT_ENV, "1");
        let verbose = commit.describe();
        std::env::remove_var(crate::diag::LOG_TEXT_ENV);
        assert!(verbose.contains("commit=\"啊\"") && verbose.contains("raw="), "{verbose}");
    }

    #[test]
    fn dictionary_summary_names_the_entry_count() {
        assert!(dictionary_summary().starts_with("embedded pinyin entries="));
    }
}
