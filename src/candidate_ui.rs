//! The candidate window's view model.
//!
//! Port of `tip/candidate_ui.go`. The model is owned by one text-service
//! instance and holds *what* should be shown: composition, candidates, selection,
//! caret geometry and DPI. A [`Presenter`] decides *how*. The model never holds a
//! COM pointer, so presentation code cannot extend the lifetime of a TSF context
//! or composition, and it hands out copies, so a painter never shares a mutable
//! slice with a TSF callback.

use std::sync::RwLock;

use crate::candidate_log;
use crate::engine::Candidate;

/// Immutable view state handed to a presenter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub composition: String,
    pub candidates: Vec<Candidate>,
    pub selected: usize,
    /// Insertion-point rectangle in screen coordinates: left, top, right, bottom.
    pub caret: [i32; 4],
    pub dpi: u32,
    pub visible: bool,
}

/// Where snapshots go. Implementations must not block: they are called from TSF
/// key callbacks.
pub trait Presenter: Send + Sync {
    /// The UI only runs between `Activate` and `Deactivate`.
    fn set_enabled(&self, enabled: bool);
    /// Offers the newest state. A presenter may drop intermediate snapshots.
    fn publish(&self, snapshot: Snapshot);
}

/// Presenter that shows nothing (tests, and the model's default).
#[derive(Clone, Copy, Debug, Default)]
pub struct NullPresenter;

impl Presenter for NullPresenter {
    fn set_enabled(&self, _enabled: bool) {}
    fn publish(&self, _snapshot: Snapshot) {}
}

pub struct CandidateUiModel {
    state: RwLock<Snapshot>,
    presenter: Box<dyn Presenter>,
}

impl CandidateUiModel {
    pub fn new(presenter: Box<dyn Presenter>) -> Self {
        CandidateUiModel { state: RwLock::new(Snapshot::default()), presenter }
    }

    /// A model with no window behind it.
    pub fn headless() -> Self {
        Self::new(Box::new(NullPresenter))
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.presenter.set_enabled(enabled);
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Snapshot> {
        self.state.write().unwrap_or_else(|p| p.into_inner())
    }

    /// Replaces the state. The list is visible only with a composition *and* at
    /// least one candidate; a selection outside the list is moved to the top.
    pub fn update(&self, composition: &str, candidates: &[Candidate], selected: usize, caret: [i32; 4], dpi: u32) {
        candidate_log!(
            "candidate.update.begin component=tip stage=model.update candidate_count={} selected_index={} composition_empty={}",
            candidates.len(),
            selected,
            composition.is_empty()
        );
        let mut selected = selected;
        if selected >= candidates.len() {
            candidate_log!(
                "candidate.update.normalize component=tip stage=model.update reason=selected_index_out_of_range old_index={} candidate_count={}",
                selected,
                candidates.len()
            );
            selected = 0;
        }
        let visible = !composition.is_empty() && !candidates.is_empty();
        let snapshot = Snapshot {
            composition: composition.to_owned(),
            candidates: if visible { candidates.to_vec() } else { Vec::new() },
            selected: if visible { selected } else { 0 },
            caret,
            dpi: if dpi == 0 { 96 } else { dpi },
            visible,
        };
        *self.write() = snapshot.clone();
        self.presenter.publish(snapshot);
    }

    /// Forgets everything and hides the list: commit, cancel, deactivation.
    pub fn hide(&self) {
        let snapshot = Snapshot { dpi: 96, ..Snapshot::default() };
        *self.write() = snapshot.clone();
        candidate_log!("candidate.hide component=tip stage=model.hide");
        self.presenter.publish(snapshot);
    }

    /// Hides the list but keeps its content, for a focus loss.
    pub fn suspend(&self) {
        let snapshot = {
            let mut state = self.write();
            state.visible = false;
            state.clone()
        };
        self.presenter.publish(snapshot);
    }

    /// Shows the remembered list again, if there is still one to show.
    pub fn resume(&self) {
        let snapshot = {
            let mut state = self.write();
            state.visible = !state.composition.is_empty() && !state.candidates.is_empty();
            state.clone()
        };
        self.presenter.publish(snapshot);
    }

    /// A copy of the current state; mutating it does not touch the model.
    pub fn snapshot(&self) -> Snapshot {
        self.state.read().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn cand(text: &str, index: usize) -> Candidate {
        Candidate { text: text.into(), index, annotation: String::new() }
    }

    /// Records everything a presenter is told.
    #[derive(Clone, Default)]
    struct Recorder {
        published: Arc<Mutex<Vec<Snapshot>>>,
        enabled: Arc<Mutex<Vec<bool>>>,
    }

    impl Presenter for Recorder {
        fn set_enabled(&self, enabled: bool) {
            self.enabled.lock().unwrap().push(enabled);
        }
        fn publish(&self, snapshot: Snapshot) {
            self.published.lock().unwrap().push(snapshot);
        }
    }

    // port of TestCandidateUIShowUpdateAndSelectedIndexClamping
    #[test]
    fn show_update_and_selected_index_clamping() {
        let model = CandidateUiModel::headless();
        let candidates = [cand("你好", 0), cand("拟好", 1)];
        model.update("nihao", &candidates, 99, [1, 2, 3, 4], 144);
        let mut s = model.snapshot();
        assert!(s.visible && s.composition == "nihao" && s.selected == 0 && s.dpi == 144, "{s:?}");
        assert_eq!(s.candidates.len(), 2);
        assert_eq!(s.candidates[0].text, "你好");
        assert_eq!(s.caret, [1, 2, 3, 4]);
        // A snapshot must not expose the model's backing storage.
        s.candidates[0].text = "mutated".into();
        assert_eq!(model.snapshot().candidates[0].text, "你好");
        model.update("nihao", &candidates, 1, [0; 4], 0);
        let s = model.snapshot();
        assert_eq!((s.selected, s.dpi), (1, 96), "selection update / default DPI: {s:?}");
    }

    // port of TestCandidateUIHidesForEmptyCompositionOrCandidates
    #[test]
    fn hides_for_empty_composition_or_candidates() {
        let model = CandidateUiModel::headless();
        model.update("ni", &[cand("你", 0)], 0, [0; 4], 96);
        model.update("", &[cand("你", 0)], 0, [0; 4], 96);
        let s = model.snapshot();
        assert!(!s.visible && s.candidates.is_empty() && s.selected == 0, "{s:?}");
        model.update("ni", &[], 5, [0; 4], 96);
        let s = model.snapshot();
        assert!(!s.visible && s.candidates.is_empty(), "{s:?}");
    }

    // port of TestCandidateUIHideResetsLifecycleState
    #[test]
    fn hide_resets_lifecycle_state() {
        let model = CandidateUiModel::headless();
        model.update("nihao", &[cand("你好", 0)], 0, [1, 2, 3, 4], 120);
        model.hide();
        let s = model.snapshot();
        assert!(!s.visible && s.composition.is_empty() && s.candidates.is_empty() && s.dpi == 96, "{s:?}");
    }

    // port of TestCandidateUISuspendHidesWithoutLosingTheList
    #[test]
    fn suspend_hides_without_losing_the_list() {
        let model = CandidateUiModel::headless();
        model.update("nihao", &[cand("你好", 0)], 0, [0; 4], 120);
        model.suspend();
        let s = model.snapshot();
        assert!(!s.visible, "suspend must hide the list");
        assert!(s.composition == "nihao" && s.candidates.len() == 1, "suspend must keep the list: {s:?}");
        model.resume();
        assert!(model.snapshot().visible, "resume must show the list again");
        // Resuming after a hide must not resurrect a list that is gone.
        model.hide();
        model.resume();
        assert!(!model.snapshot().visible);
    }

    #[test]
    fn every_change_is_published_to_the_presenter() {
        let recorder = Recorder::default();
        let model = CandidateUiModel::new(Box::new(recorder.clone()));
        model.set_enabled(true);
        model.update("ni", &[cand("你", 0)], 0, [0; 4], 96);
        model.suspend();
        model.resume();
        model.hide();
        let published = recorder.published.lock().unwrap();
        let visible: Vec<bool> = published.iter().map(|s| s.visible).collect();
        assert_eq!(visible, [true, false, true, false]);
        assert_eq!(*recorder.enabled.lock().unwrap(), [true]);
        // the presenter's copy is independent of the model's
        assert_eq!(published[0].candidates[0].text, "你");
    }
}
