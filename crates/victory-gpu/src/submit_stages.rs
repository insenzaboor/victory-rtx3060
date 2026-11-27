//! Wall time the victory-submit worker spends in each stage of a
//! submission (MoonYueya/VIctory-Kira ff9d4c1, from the idea in
//! stellarr-studio/kelvaroq's frame_wait.rs, GPL-3.0-or-later, patches 0046
//! and 0053), with the stages this fork's submitting path has.
//!
//! Off unless VICTORY_SUBMIT_STAGES=1: with it the worker keeps one clock,
//! each stage is marked where the work it names begins, and the frame-rate
//! line prints what the stages took per frame. The stages nest (the card
//! fence wait happens inside Execute), so a stage is charged only its own
//! time: a mark ends the one before it.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// The stages of a submission on our own thread, in the order of the
/// counters the frame line prints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Anything not named by another stage: the line's own bookkeeping.
    Other,
    /// Descriptors copied into placed heaps, and the slots waiting for a
    /// texture's host copy (descriptor::begin_sync_heaps,
    /// descriptor::make_waiting).
    Descriptors,
    /// The one write-watch asking a submission makes, and the journal
    /// lookups the refreshes answer from it (crate::watch).
    Watch,
    /// Copies and fills of title memory taken out of the lists, the
    /// refreshes among them (Batch).
    Deferred,
    /// Snapshots of the ranges shaders may write
    /// (shadow::take_snapshots).
    Snapshots,
    /// What goes to copies in the card's memory, ahead of the lists
    /// (card_before), and the picked writes declared for views.
    CardSetup,
    /// ExecuteCommandLists and the fence behind it, the wait for the
    /// card's own fence among them.
    Execute,
    /// The copy-back of a submission that is waited for.
    CopyBack,
}

/// The stages, as the frame line names them (a diagnostics build's).
#[cfg_attr(not(feature = "diagnostics"), allow(dead_code))]
pub(crate) const STAGES: [(&str, Stage); 7] = [
    ("descriptors", Stage::Descriptors),
    ("watch and coherence", Stage::Watch),
    ("deferred copies and barriers", Stage::Deferred),
    ("snapshots", Stage::Snapshots),
    ("card setup", Stage::CardSetup),
    ("ExecuteCommandLists", Stage::Execute),
    ("copy back", Stage::CopyBack),
];

/// Counters, one per stage.
const COUNTERS: usize = 8;

/// The worker's clock: the stage it is in, and when that stage began.
struct Clock {
    began: Instant,
    /// None: the clock is not ticking (off, or not on the worker).
    stage: Option<Stage>,
}

thread_local! {
    /// This thread's clock; ticking on the victory-submit worker alone.
    static CLOCK: Cell<Option<Clock>> = const { Cell::new(None) };
}

/// Since the last frame line, the nanoseconds of each stage, all
/// submissions together, and how many driver batches and lists they sent.
static SPENT: [AtomicU64; COUNTERS] = [const { AtomicU64::new(0) }; COUNTERS];
static BATCH: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

/// Whether the stages are timed (VICTORY_SUBMIT_STAGES=1; off by default,
/// and never on a build without diagnostics, which has no frame line to
/// print them on).
pub(crate) fn on() -> bool {
    if cfg!(not(feature = "diagnostics")) {
        return false;
    }
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| matches!(std::env::var("VICTORY_SUBMIT_STAGES").as_deref(), Ok("1")))
}

/// Starts this thread's clock at stage Other: called on the worker once,
/// before it serves the line.
pub(crate) fn start() {
    if !on() {
        return;
    }
    CLOCK.with(|clock| {
        clock.set(Some(Clock { began: Instant::now(), stage: Some(Stage::Other) }));
    });
}

/// Ends the stage that was running (charging it) and begins the next.
pub(crate) fn mark(next: Stage) {
    if !on() {
        return;
    }
    CLOCK.with(|clock| {
        let Some(mut held) = clock.take() else {
            return;
        };
        let now = Instant::now();
        if let Some(stage) = held.stage {
            let ns = (now - held.began).as_nanos() as u64;
            SPENT[stage as usize].fetch_add(ns, Ordering::Relaxed);
        }
        held.began = now;
        held.stage = Some(next);
        clock.set(Some(held));
    });
}

/// A driver batch of lists lists went to the card.
pub(crate) fn executing(lists: usize) {
    if !on() {
        return;
    }
    BATCH[0].fetch_add(1, Ordering::Relaxed);
    BATCH[1].fetch_add(lists as u64, Ordering::Relaxed);
}

/// What the frame line prints, per frame: the stages of the submitting
/// thread, and the driver batches; empty when the stages are off.
#[cfg_attr(not(feature = "diagnostics"), allow(dead_code))]
pub(crate) fn line(frames: f64) -> String {
    if !on() || frames <= 0.0 {
        return String::new();
    }
    let ms = |ns: u64| ns as f64 / 1e6 / frames;
    let spent: Vec<f64> = SPENT.iter().map(|n| ms(n.swap(0, Ordering::Relaxed))).collect();
    let other = spent[Stage::Other as usize];
    let named: f64 = STAGES.iter().map(|&(_, stage)| spent[stage as usize]).sum();
    let [calls, lists] = BATCH.each_ref().map(|n| n.swap(0, Ordering::Relaxed));
    let stages: Vec<String> =
        STAGES.iter().map(|&(name, stage)| format!("{name} {:.2}", spent[stage as usize])).collect();
    format!(
        ", work by stage: {} ({} driver batches a frame, {:.2} lists a call, {:.1} ms in all, {other:.2} other)",
        stages.join(", "),
        calls as f64 / frames,
        lists as f64 / calls.max(1) as f64,
        named + other,
    )
}

/// One clock, ticked by hand: what a test needs of mark without the
/// thread-local or the counters.
#[cfg(test)]
use std::time::Duration;

#[cfg(test)]
pub(crate) struct Tick {
    began: Instant,
    stage: Stage,
    pub(crate) spent: [Duration; COUNTERS],
}

#[cfg(test)]
impl Tick {
    /// A clock that began at `began`.
    pub(crate) fn from(stage: Stage, began: Instant) -> Tick {
        Tick { began, stage, spent: [Duration::ZERO; COUNTERS] }
    }

    /// Charges what came since the last tick to the stage that ran.
    pub(crate) fn at(&mut self, now: Instant) {
        self.spent[self.stage as usize] += now - self.began;
        self.began = now;
    }

    pub(crate) fn enter(&mut self, next: Stage, now: Instant) {
        self.at(now);
        self.stage = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stage is charged only its own time: a nested wait inside it does
    /// not end it, and what comes after goes to the stage that follows.
    #[test]
    fn a_stage_excludes_a_nested_wait() {
        let start = Instant::now();
        let mut clock = Tick::from(Stage::Deferred, start);
        clock.enter(Stage::Execute, start + Duration::from_millis(3));
        clock.enter(Stage::Deferred, start + Duration::from_millis(8));
        assert_eq!(clock.spent[Stage::Deferred as usize], Duration::from_millis(3));
        assert_eq!(clock.spent[Stage::Execute as usize], Duration::from_millis(5));
        clock.at(start + Duration::from_millis(14));
        assert_eq!(clock.spent[Stage::Deferred as usize], Duration::from_millis(9));
        assert_eq!(clock.spent[Stage::Other as usize], Duration::ZERO);
    }

    /// Every stage the frame line names is a stage of the clock, none is
    /// named twice, and each is a counter of its own.
    #[test]
    fn every_named_stage_is_counted_once() {
        let mut seen: Vec<Stage> = STAGES.iter().map(|&(_, stage)| stage).collect();
        seen.sort_by_key(|&stage| stage as usize);
        seen.dedup();
        assert_eq!(seen.len(), STAGES.len());
        assert!(STAGES.iter().all(|&(name, _)| !name.is_empty()));
        assert!(STAGES.iter().all(|&(_, stage)| (stage as usize) < COUNTERS));
    }

    /// Off, the clock neither starts nor counts.
    #[test]
    fn off_counts_nothing() {
        if on() {
            // A run with the switch set in the environment: nothing to say.
            return;
        }
        mark(Stage::Execute);
        let line = line(1.0);
        assert!(line.is_empty(), "a line with the stages off: {line}");
    }
}
