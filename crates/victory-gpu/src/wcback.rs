//! `VICTORY_SKIP_WC_BACK=1` (off by default), an experiment: what shaders
//! wrote to a copy in the card's memory (`crate::shadow::cards`) is not
//! fetched back into title memory when the declared range lies wholly in a
//! reservation the title asked write-combining (console protection bit
//! 0x400). `VICTORY_SKIP_WC_KEEP=<bytes>` (0 by default) still fetches back
//! declared ranges of at most that many bytes.
//!
//! Why it can hold: write-combined memory is the console's memory for the
//! CPU to stream into and the GPU to read and write; the CPU reading it is
//! uncached and slow there, so a title does not read it back. The card copy
//! is what every later GPU use reads, and the CPU's own changes are still
//! sent up. Peppacodes/playablegames-peppa's census (6035addd) found 97-99 %
//! of some 100 MB fetched back a frame came from such memory.
//!
//! Why it is an experiment:
//! - Small results the title does read on the CPU (exposure, likely) live in
//!   such memory too: alone the switch turned the picture black and the
//!   title drew a third less (Peppa's ca2432bd). `VICTORY_SKIP_WC_KEEP` keeps
//!   those fetched; Peppa ran with 1048576.
//! - Where the title's CPU writes part of a piece the card wrote, what goes
//!   up next can carry the CPU side's stale bytes over the card's (less so
//!   for copies that send only the dwords the title changed, as windows do).
//! - A copy made anew over the same title memory starts from title memory,
//!   which lacks what was left on the card.
//!
//! Measured by Peppa (RTX 5080 + 5700X3D, in-run A/B, `KEEP=65536`): 51.4 ->
//! 47.5 ms median (ca2432bd), and -3.9 ms in 44 of 44 cycles with
//! `KEEP=1048576` (its README); no device loss reported with this switch.
//! Its `VICTORY_SKIP_WC_COMPARE`, which also leaves such copies out of the
//! compare, lost a device in one run and is not taken here.
//!
//! The reservations are recorded only while the switch is set; in the in-run
//! A/B (`VICTORY_AB`, diagnostics builds) it must be set as well, and the A/B
//! then turns the skipping on and off.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

/// The console protection's write-combined bit.
const WRITE_COMBINED: u32 = 0x400;

/// `XMemVirtualAlloc` reservations at addresses of the runtime's choice:
/// base, then length and the protection the title asked for (before the
/// runtime drops the console bits).
static RESERVATIONS: Mutex<BTreeMap<usize, (usize, u32)>> = Mutex::new(BTreeMap::new());

/// Whether the experiment is on (`VICTORY_SKIP_WC_BACK=1`).
pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("VICTORY_SKIP_WC_BACK").is_some_and(|v| v == "1");
        if on {
            tracing::warn!(
                "VICTORY_SKIP_WC_BACK=1: shader writes to write-combining title memory stay on the card, declared ranges \
                 of at most {} bytes excepted (VICTORY_SKIP_WC_KEEP); an experiment",
                keep()
            );
        }
        on
    })
}

/// `VICTORY_SKIP_WC_KEEP`: declared ranges of at most this many bytes are
/// still fetched back.
fn keep() -> usize {
    static KEEP: OnceLock<usize> = OnceLock::new();
    *KEEP.get_or_init(|| std::env::var("VICTORY_SKIP_WC_KEEP").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(0))
}

/// The runtime reserved `[at, at + len)` for the title with the console
/// protection `protect`. A reservation recorded before over any of it is
/// gone (the system gave the addresses again).
pub fn title_memory_reserved(at: usize, len: usize, protect: u32) {
    if !on() || at == 0 || len == 0 {
        return;
    }
    let mut map = RESERVATIONS.lock().unwrap_or_else(|e| e.into_inner());
    record(&mut map, at, len, protect);
}

/// The title released the reservation at `at`.
pub fn title_memory_released(at: usize) {
    if !on() {
        return;
    }
    RESERVATIONS.lock().unwrap_or_else(|e| e.into_inner()).remove(&at);
}

fn record(map: &mut BTreeMap<usize, (usize, u32)>, at: usize, len: usize, protect: u32) {
    let end = at.saturating_add(len);
    let stale: Vec<usize> = map
        .range(..end)
        .rev()
        .take_while(|(base, (n, _))| base.saturating_add(*n) > at)
        .map(|(&base, _)| base)
        .collect();
    for base in stale {
        map.remove(&base);
    }
    map.insert(at, (len, protect));
}

/// Whether `[from, to)` lies wholly in one recorded write-combined
/// reservation.
fn in_wc(map: &BTreeMap<usize, (usize, u32)>, (from, to): (usize, usize)) -> bool {
    // Seeking from the exclusive end also finds a reservation beginning
    // inside the range.
    from < to
        && map.range(..to).next_back().is_some_and(|(&base, &(len, protect))| {
            base <= from && protect & WRITE_COMBINED != 0 && base.checked_add(len).is_some_and(|end| to <= end)
        })
}

/// Pieces left on the card, and their bytes.
static SKIPPED: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

/// Whether what shaders wrote in the declared range `declared` (title
/// addresses, as joined for the copy-back) stays on the card. Asked once per
/// range; each piece left there is then counted with [`skipped`].
pub(crate) fn skips(declared: (usize, usize)) -> bool {
    if !on() || declared.1.saturating_sub(declared.0) <= keep() {
        return false;
    }
    #[cfg(feature = "diagnostics")]
    if crate::ab::pick("VICTORY_SKIP_WC_BACK") == Some(false) {
        return false;
    }
    in_wc(&RESERVATIONS.lock().unwrap_or_else(|e| e.into_inner()), declared)
}

/// A piece of `bytes` was left on the card ([`skips`]).
pub(crate) fn skipped(bytes: u64) {
    let n = SKIPPED[0].fetch_add(1, Relaxed) + 1;
    let b = SKIPPED[1].fetch_add(bytes, Relaxed) + bytes;
    if n.is_power_of_two() {
        tracing::info!("VICTORY_SKIP_WC_BACK: {n} pieces ({} MB) of write-combining memory left on the card so far", b >> 20);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: usize = 1 << 20;

    #[test]
    fn a_write_in_the_ordinary_half_of_a_card_window_still_comes_back() {
        let window = 8 * MIB;
        let middle = window + 2 * MIB;
        let mut map = BTreeMap::new();
        record(&mut map, window, 2 * MIB, 0x404);
        record(&mut map, middle, 2 * MIB, 0x04);
        assert!(in_wc(&map, (middle - 65536, middle)));
        assert!(!in_wc(&map, (middle, middle + 65536)));
        assert!(!in_wc(&map, (middle - 65536, middle + 65536)));
    }

    #[test]
    fn a_declaration_must_fit_one_known_reservation() {
        let mut map = BTreeMap::new();
        record(&mut map, 0x10000, 0x10000, 0x404);
        record(&mut map, 0x20000, 0x10000, 0x404);
        map.insert(usize::MAX - 8, (16, 0x404));
        assert!(in_wc(&map, (0x10000, 0x20000)));
        assert!(in_wc(&map, (0x20000, 0x30000)));
        assert!(!in_wc(&map, (0x1ffff, 0x20001)));
        assert!(!in_wc(&map, (0x30000, 0x30001)));
        assert!(!in_wc(&map, (0x10000, 0x10000)));
        assert!(!in_wc(&map, (usize::MAX - 8, 1)));
        assert!(!in_wc(&map, (usize::MAX - 8, usize::MAX)));
    }

    #[test]
    fn a_reservation_over_old_ones_replaces_them() {
        let mut map = BTreeMap::new();
        record(&mut map, 4 * MIB, 4 * MIB, 0x404);
        record(&mut map, 9 * MIB, MIB, 0x404);
        // Freed unseen; the system hands out [3, 10) MiB as ordinary memory.
        record(&mut map, 3 * MIB, 7 * MIB, 0x04);
        assert_eq!(map.len(), 1);
        assert!(!in_wc(&map, (5 * MIB, 5 * MIB + 4096)));
        assert!(!in_wc(&map, (9 * MIB, 9 * MIB + 4096)));
        // Next to it, untouched.
        record(&mut map, 10 * MIB, MIB, 0x404);
        assert_eq!(map.len(), 2);
        assert!(in_wc(&map, (10 * MIB, 11 * MIB)));
    }
}
