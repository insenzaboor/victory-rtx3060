//! Wall time the victory-submit worker spends in each stage of a
//! submission (MoonYueya/VIctory-Kira ff9d4c1, from the idea in
//! stellarr-studio/kelvaroq's frame_wait.rs, GPL-3.0-or-later, patches 0046
//! and 0053), with the stages this fork's submitting path has.
//!
//! Off unless VICTORY_SUBMIT_STAGES=1: with it the worker keeps one clock,
//! running from its start, and each stage is marked where the work it names
//! begins. A stage is charged only its own time: a [`mark`] ends the one
//! before it, and a stage nested in another ([`enter`], [`during`]: a fence
//! wait inside a settle, a settle inside the watch, a submission inside a
//! present) hands the clock back to the one it interrupted when it ends.
//! The clock never stops, so the stages, idle among them, add up to the
//! worker's wall time.
//!
//! The line prints on its own every VICTORY_SUBMIT_STAGES_EVERY seconds (30
//! by default), per frame presented through the line; 0: with the frame
//! meter's line, as it did first.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The stages of the worker's time, in the order of the counters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Anything not named by another stage: before the first mark.
    Other,
    /// Waiting for the title to put something in the line, with nothing in
    /// it (queue/behind.rs `Lane::serve`).
    Idle,
    /// Waiting with entries in the line that are not free to run: a fence
    /// or memory value not met yet, lists not translated yet.
    Stalled,
    /// The line's own work: picking, taking along and unfolding entries,
    /// signals, what a submission does before it takes the lock.
    Line,
    /// A gathering's wait for lists still being translated (`W::hold`).
    Translation,
    /// Waiting for the submission lock (held by the thread that settles
    /// flights, or by a title thread submitting itself).
    Lock,
    /// The steps given to the card for the reads of a submission sent
    /// ahead (`card_steps`).
    CardSteps,
    /// With the relay: waiting for the thread that lands flights
    /// (`relay_wait`), whatever the reason.
    RelayWait,
    /// Flights settled on this thread (`Host::settle` without the relay),
    /// their fence wait apart: copy-backs, bottom-of-pipe writes, signals.
    Landing,
    /// The card's fence, waited for on the CPU (WaitForSingleObject).
    FenceWait,
    /// Descriptors copied into placed heaps, and the slots waiting for a
    /// texture's host copy (descriptor::begin_sync_heaps,
    /// descriptor::make_waiting).
    Descriptors,
    /// Waiting for the round that mirrors the heaps' records (the drop of
    /// the round begun with the submission).
    HeapsWait,
    /// The sweep of what no list names any more (with its settle).
    Sweep,
    /// The write-watch asking, the journal lookups the refreshes answer
    /// from it, and what is looked at against flights' writes
    /// (crate::watch, late_blocked, stale_under_flights).
    Watch,
    /// Refreshes of host copies: ahead of writes, and the last (Batch
    /// refresh_front, refresh_ahead, flush).
    Refreshes,
    /// The lists' own deferred steps: copies and fills of title memory
    /// taken out of the lists, barriers (run_deferred, Batch::fill).
    Deferred,
    /// The state preludes recorded ahead of each list (HostList::prelude).
    Preludes,
    /// Command lists closed (ID3D12GraphicsCommandList::Close).
    Closing,
    /// Snapshots of the ranges shaders may write
    /// (shadow::take_snapshots).
    Snapshots,
    /// What goes to copies in the card's memory, ahead of the lists
    /// (card_before), and the picked writes declared for views.
    CardSetup,
    /// The driver calls that send: ExecuteCommandLists and the Signal
    /// behind it, nothing else.
    Execute,
    /// After the sending: the flight noted for the relay, what goes back,
    /// the checks after a wait.
    Noting,
    /// Landed flights' lists reset and given back (land_after_submission).
    Recycle,
    /// The copy-back of a submission that is waited for.
    CopyBack,
    /// A present, but for the DXGI call: the copy to the back buffer, its
    /// submission (stages of its own), the frame meter.
    Present,
    /// IDXGISwapChain::Present.
    SwapPresent,
}

/// The stages, as the line names them, in the order it prints them.
pub(crate) const STAGES: [(&str, Stage); 25] = [
    ("idle (line empty)", Stage::Idle),
    ("line stands at a wait", Stage::Stalled),
    ("the line's own work", Stage::Line),
    ("waiting for translation", Stage::Translation),
    ("the submission lock", Stage::Lock),
    ("steps given to the card", Stage::CardSteps),
    ("waiting for landings", Stage::RelayWait),
    ("landing inline", Stage::Landing),
    ("card fence waits", Stage::FenceWait),
    ("descriptors", Stage::Descriptors),
    ("heap records wait", Stage::HeapsWait),
    ("the sweep", Stage::Sweep),
    ("watch and coherence", Stage::Watch),
    ("refreshes", Stage::Refreshes),
    ("deferred copies and barriers", Stage::Deferred),
    ("preludes", Stage::Preludes),
    ("closing lists", Stage::Closing),
    ("snapshots", Stage::Snapshots),
    ("card setup", Stage::CardSetup),
    ("ExecuteCommandLists", Stage::Execute),
    ("noting flights", Stage::Noting),
    ("landed lists reset", Stage::Recycle),
    ("copy back", Stage::CopyBack),
    ("present", Stage::Present),
    ("DXGI Present", Stage::SwapPresent),
];

/// Counters, one per stage.
const COUNTERS: usize = STAGES.len() + 1;

/// Stages that are waiting, not work: left out of "busy".
const WAITING: [Stage; 6] =
    [Stage::Idle, Stage::Stalled, Stage::Translation, Stage::Lock, Stage::RelayWait, Stage::FenceWait];

/// The worker's clock: the stage it is in, when that stage began, and when
/// the line last printed.
#[derive(Clone, Copy)]
struct Clock {
    began: Instant,
    stage: Stage,
    printed: Instant,
}

impl Clock {
    fn new(stage: Stage, now: Instant) -> Clock {
        Clock { began: now, stage, printed: now }
    }

    /// Ends the running stage at `now`, giving `charge` its nanoseconds,
    /// and begins `next`: the stage that ran.
    fn switch(&mut self, next: Stage, now: Instant, charge: impl FnOnce(Stage, u64)) -> Stage {
        let ran = self.stage;
        charge(ran, now.saturating_duration_since(self.began).as_nanos() as u64);
        self.began = now;
        self.stage = next;
        ran
    }
}

thread_local! {
    /// This thread's clock; ticking on the victory-submit worker alone.
    static CLOCK: Cell<Option<Clock>> = const { Cell::new(None) };
}

/// Since the last line, the nanoseconds of each stage, all submissions
/// together; how many driver batches and lists they sent; the frames
/// presented through the line.
static SPENT: [AtomicU64; COUNTERS] = [const { AtomicU64::new(0) }; COUNTERS];
static BATCH: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
static FRAMES: AtomicU64 = AtomicU64::new(0);

fn charge(stage: Stage, ns: u64) {
    SPENT[stage as usize].fetch_add(ns, Ordering::Relaxed);
}

/// Whether the stages are timed (VICTORY_SUBMIT_STAGES=1; off by default,
/// and never on a build without diagnostics).
pub(crate) fn on() -> bool {
    if cfg!(not(feature = "diagnostics")) {
        return false;
    }
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| matches!(std::env::var("VICTORY_SUBMIT_STAGES").as_deref(), Ok("1")))
}

/// How often the line prints on its own (VICTORY_SUBMIT_STAGES_EVERY,
/// seconds, 30 by default); None: with the frame meter's line instead.
fn every() -> Option<Duration> {
    static EVERY: std::sync::OnceLock<Option<Duration>> = std::sync::OnceLock::new();
    *EVERY.get_or_init(|| {
        let secs = std::env::var("VICTORY_SUBMIT_STAGES_EVERY").ok().and_then(|v| v.trim().parse::<f64>().ok());
        match secs {
            Some(s) if s <= 0.0 => None,
            Some(s) => Some(Duration::from_secs_f64(s)),
            None => Some(Duration::from_secs(30)),
        }
    })
}

/// Starts this thread's clock at `stage`, whatever the switch says.
fn start_clock(stage: Stage) {
    CLOCK.with(|clock| clock.set(Some(Clock::new(stage, Instant::now()))));
}

/// Switches this thread's clock to `next`, whatever the switch says: the
/// stage that was running (None: no clock on this thread).
fn switch_clock(next: Stage) -> Option<Stage> {
    CLOCK.with(|clock| {
        let mut held = clock.get()?;
        let ran = held.switch(next, Instant::now(), charge);
        clock.set(Some(held));
        Some(ran)
    })
}

/// Starts this thread's clock at stage Line: called on the worker once,
/// before it serves the line.
pub(crate) fn start() {
    if on() {
        start_clock(Stage::Line);
    }
}

/// Ends the stage that was running (charging it) and begins the next.
#[inline]
pub(crate) fn mark(next: Stage) {
    if on() {
        switch_clock(next);
    }
}

/// A stage nested in whatever runs: given back to it when this is dropped.
#[must_use]
pub(crate) struct Back(Option<Stage>);

impl Drop for Back {
    fn drop(&mut self) {
        if let Some(was) = self.0 {
            switch_clock(was);
        }
    }
}

/// Begins `next` until the value that comes back is dropped; then the stage
/// that ran before it (whatever the marks in between) runs again.
#[inline]
pub(crate) fn enter(next: Stage) -> Back {
    Back(if on() { switch_clock(next) } else { None })
}

/// `f` as stage `stage`, nested in whatever runs.
#[inline]
pub(crate) fn during<T>(stage: Stage, f: impl FnOnce() -> T) -> T {
    let _back = enter(stage);
    f()
}

/// A driver batch of lists lists went to the card.
pub(crate) fn executing(lists: usize) {
    if !on() {
        return;
    }
    BATCH[0].fetch_add(1, Ordering::Relaxed);
    BATCH[1].fetch_add(lists as u64, Ordering::Relaxed);
}

/// A frame was presented through the line (on the worker); the line
/// prints if it is due.
pub(crate) fn presented() {
    if !on() {
        return;
    }
    FRAMES.fetch_add(1, Ordering::Relaxed);
    tick();
}

/// Prints the line on its own if it is due (on the worker; with
/// VICTORY_SUBMIT_STAGES_EVERY not 0).
pub(crate) fn tick() {
    if !on() {
        return;
    }
    let Some(every) = every() else {
        return;
    };
    let due = CLOCK.with(|clock| {
        let mut held = clock.get()?;
        let now = Instant::now();
        let wall = now.saturating_duration_since(held.printed);
        if wall < every {
            return None;
        }
        // What runs now is charged up to now, so the stages add up to the
        // wall time since the last line.
        let running = held.stage;
        held.switch(running, now, charge);
        held.printed = now;
        clock.set(Some(held));
        Some(wall)
    });
    let Some(wall) = due else {
        return;
    };
    let frames = FRAMES.swap(0, Ordering::Relaxed);
    let text = take_line(frames as f64, Some(wall));
    tracing::info!(
        "the submitting thread (VICTORY_SUBMIT_STAGES=1), the last {:.1} s, {frames} frames{text}",
        wall.as_secs_f64()
    );
}

/// What the frame meter's line prints, per frame: empty when the stages are
/// off or print on their own.
#[cfg_attr(not(feature = "diagnostics"), allow(dead_code))]
pub(crate) fn line(frames: f64) -> String {
    if !on() || frames <= 0.0 || every().is_some() {
        return String::new();
    }
    take_line(frames, None)
}

/// Takes the counters and makes the line of them.
fn take_line(frames: f64, wall: Option<Duration>) -> String {
    let spent: [u64; COUNTERS] = std::array::from_fn(|i| SPENT[i].swap(0, Ordering::Relaxed));
    let [calls, lists] = BATCH.each_ref().map(|n| n.swap(0, Ordering::Relaxed));
    format_line(&spent, frames, wall, calls, lists)
}

/// The line: each stage per frame (per second without frames), then the
/// driver batches, the sums and how much of the wall time they hold.
fn format_line(spent: &[u64; COUNTERS], frames: f64, wall: Option<Duration>, calls: u64, lists: u64) -> String {
    // Without a frame (a load, a hang): per second of wall time.
    let (per, unit) = match (frames > 0.0, wall) {
        (true, _) => (frames, "a frame"),
        (false, Some(wall)) if !wall.is_zero() => (wall.as_secs_f64(), "a second"),
        _ => (1.0, "in all"),
    };
    let ms = |ns: u64| ns as f64 / 1e6 / per;
    let all: u64 = spent.iter().sum();
    let waiting: u64 = WAITING.iter().map(|&stage| spent[stage as usize]).sum();
    let stages: Vec<String> =
        STAGES.iter().map(|&(name, stage)| format!("{name} {:.2}", ms(spent[stage as usize]))).collect();
    let covered = match wall {
        Some(wall) if !wall.is_zero() => {
            format!(", {:.0}% of the wall time", 100.0 * all as f64 / wall.as_nanos() as f64)
        }
        _ => String::new(),
    };
    format!(
        ", work by stage, ms {unit}: {} ({:.2} driver batches {unit}, {:.2} lists a call; {:.1} ms in all, {:.1} busy, {:.2} other{covered})",
        stages.join(", "),
        calls as f64 / per,
        lists as f64 / calls.max(1) as f64,
        ms(all),
        ms(all - waiting),
        ms(spent[Stage::Other as usize]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stage is charged only its own time: what comes after a switch goes
    /// to the stage that follows.
    #[test]
    fn a_switch_charges_the_stage_that_ran() {
        let start = Instant::now();
        let mut spent = [0u64; COUNTERS];
        let mut clock = Clock::new(Stage::Deferred, start);
        let ms = |n: u64| start + Duration::from_millis(n);
        assert_eq!(clock.switch(Stage::Execute, ms(3), |s, ns| spent[s as usize] += ns), Stage::Deferred);
        assert_eq!(clock.switch(Stage::Deferred, ms(8), |s, ns| spent[s as usize] += ns), Stage::Execute);
        clock.switch(Stage::Idle, ms(14), |s, ns| spent[s as usize] += ns);
        assert_eq!(spent[Stage::Deferred as usize], 9_000_000);
        assert_eq!(spent[Stage::Execute as usize], 5_000_000);
        assert_eq!(spent[Stage::Other as usize], 0);
        // A clock read back in time charges nothing, never wraps.
        clock.switch(Stage::Line, ms(10), |s, ns| spent[s as usize] += ns);
        assert_eq!(spent[Stage::Idle as usize], 0);
    }

    /// A nested stage gives the clock back to the stage it interrupted,
    /// whatever was marked inside it, and every nanosecond goes somewhere.
    /// (The only test that touches the counters, on a thread of its own.)
    #[test]
    fn a_nested_stage_gives_back_to_its_caller() {
        std::thread::spawn(|| {
            let back = |stage| Back(switch_clock(stage));
            let began = Instant::now();
            start_clock(Stage::Line);
            {
                let _present = back(Stage::Present);
                {
                    let _submit = back(Stage::Lock);
                    switch_clock(Stage::Descriptors);
                    {
                        let _wait = back(Stage::FenceWait);
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    assert_eq!(CLOCK.with(|c| c.get().map(|c| c.stage)), Some(Stage::Descriptors));
                    switch_clock(Stage::Execute);
                }
                assert_eq!(CLOCK.with(|c| c.get().map(|c| c.stage)), Some(Stage::Present));
            }
            assert_eq!(CLOCK.with(|c| c.get().map(|c| c.stage)), Some(Stage::Line));
            switch_clock(Stage::Line);
            let wall = began.elapsed().as_nanos() as u64;
            let spent: [u64; COUNTERS] = std::array::from_fn(|i| SPENT[i].swap(0, Ordering::Relaxed));
            assert!(spent[Stage::FenceWait as usize] >= 2_000_000);
            let all: u64 = spent.iter().sum();
            assert!(all <= wall, "{all} ns charged in {wall}");
            assert_eq!(spent[Stage::Other as usize], 0);
        })
        .join()
        .unwrap();
    }

    /// Without a clock on the thread nothing is charged, and a nested stage
    /// gives back nothing.
    #[test]
    fn no_clock_no_charge() {
        std::thread::spawn(|| {
            assert_eq!(switch_clock(Stage::Execute), None);
            drop(Back(None));
            assert!(CLOCK.with(|c| c.get()).is_none());
        })
        .join()
        .unwrap();
    }

    /// Every stage the line names is a stage of the clock, none is named
    /// twice, each is a counter of its own, and only Other is unnamed.
    #[test]
    fn every_named_stage_is_counted_once() {
        let mut seen: Vec<usize> = STAGES.iter().map(|&(_, stage)| stage as usize).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), STAGES.len());
        assert!(STAGES.iter().all(|&(name, _)| !name.is_empty()));
        assert!(seen.iter().all(|&i| i < COUNTERS && i != Stage::Other as usize));
        assert_eq!(Stage::SwapPresent as usize, COUNTERS - 1);
    }

    /// The line: per frame, the sums, busy without the waits, the share of
    /// the wall time; per second without frames.
    #[test]
    fn the_line_adds_up() {
        let mut spent = [0u64; COUNTERS];
        spent[Stage::Idle as usize] = 40_000_000;
        spent[Stage::Execute as usize] = 20_000_000;
        spent[Stage::FenceWait as usize] = 10_000_000;
        spent[Stage::Deferred as usize] = 30_000_000;
        let line = format_line(&spent, 2.0, Some(Duration::from_millis(100)), 8, 12);
        assert!(line.contains("idle (line empty) 20.00"), "{line}");
        assert!(line.contains("ExecuteCommandLists 10.00"), "{line}");
        assert!(line.contains("4.00 driver batches a frame, 1.50 lists a call"), "{line}");
        assert!(line.contains("50.0 ms in all, 25.0 busy, 0.00 other, 100% of the wall time"), "{line}");
        let line = format_line(&spent, 0.0, Some(Duration::from_secs(2)), 0, 0);
        assert!(line.contains("ms a second"), "{line}");
        assert!(line.contains("ExecuteCommandLists 10.00"), "{line}");
    }

    /// Off, nothing counts and no line is made.
    #[test]
    fn off_counts_nothing() {
        if on() {
            // A run with the switch set in the environment: nothing to say.
            return;
        }
        mark(Stage::Execute);
        drop(enter(Stage::FenceWait));
        assert_eq!(during(Stage::Closing, || 7), 7);
        let line = line(1.0);
        assert!(line.is_empty(), "a line with the stages off: {line}");
    }
}
