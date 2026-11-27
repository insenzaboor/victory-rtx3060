//! `VICTORY_LAND_NOT=<lengths>` (unset by default: nothing changes): host
//! copies of title memory whose length is listed (hex, comma separated, with
//! or without `0x`) keep what shaders wrote in them on the card. Their pieces
//! are not fetched back into title memory after a flight (neither landed by
//! the relay nor fetched by `Host::card_back`), except the 64 KiB pieces the
//! title sent bytes up into within the last [`SENT_FRAMES`] frames
//! ([`title_sent`]), which land as before.
//!
//! Ported from adanaport/adanaport 692cebe6e, 26d310d9f and 50a0b550c
//! (avatarmanz-like base, as ours). For memory only shaders use, which the
//! title's code neither reads nor writes after it set it up: the card's side
//! is the one that counts, and the title's later lists find it there. Keyed
//! by length, as the addresses differ between runs; the log names each copy
//! a listed length catches, the first time (a length is no name: another
//! buffer of the same size is caught too).
//!
//! # This title's list (GTA VI, Series S)
//!
//! adanaport's `scripts/play.ps1` (RTX 4070 SUPER + 6-core 7500F, Series S):
//!
//! `VICTORY_LAND_NOT=1412200,2d00000,5265c0,1cc000,3200000,c00000,2a3000`
//!
//! | length | what it is (adanaport's notes) | fetched a frame without it |
//! |---|---|---|
//! | `0x1412200` | the title upscaler chain's work buffer | 21 MB |
//! | `0x2d00000` | a buffer shaders fill (a piece or two sent up now and then: those keep landing) | 14 MB with the next two |
//! | `0x5265c0` | a buffer shaders fill | (above) |
//! | `0x1cc000` | a buffer shaders fill (two copies of this length in a walk) | (above) |
//! | `0x3200000` | the title and the shaders write pieces of it apart | 11 MB with the next two |
//! | `0xc00000` | shaders only (four copies of this length in a walk) | (above) |
//! | `0x2a3000` | shaders only (two copies of this length in a walk) | (above) |
//!
//! Measured there: 22.7 -> 25.4 fps with the first four (copying back
//! 24 -> 16 ms a frame, merging what was fetched 13.2 -> 6.2 ms), 26.1 fps
//! with all seven (17.7 MB fetched a frame, was about 63); in-run A/B 45.3 ->
//! 39.7 ms (first four) and 39.0 -> 37.5 ms (last three), the same draws a
//! frame, the pictures alike, no device lost. Copies that the title's CPU and
//! the shaders both write pieces of (the 174 MB ring, `0x1900000`,
//! `0x564000`) must stay off the list. The upscaler buffer's length follows
//! the title's render size: with another output resolution or
//! `VICTORY_RENDER_SCALE` check the log line "a host copy of a listed length
//! stays on the card" (and the copy-back census) for the length it has here.
//!
//! # Risks
//!
//! - A listed copy the title's CPU does read (after a flight) sees stale
//!   bytes: wrong pictures or logic, not a crash. The 300-frame window keeps
//!   pieces the title writes landing, not pieces it only reads.
//! - A host copy made anew over the same title memory starts from title
//!   memory, which lacks what was left on the card (as with
//!   `VICTORY_SKIP_WC_BACK`).
//! - With `VICTORY_AB` naming `VICTORY_LAND_NOT` (diagnostics builds; the list
//!   must be set too) the skipping turns on and off while the title runs;
//!   what was left on the card in an "on" phase is not fetched when it turns
//!   off, only what shaders write next.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

/// How long a piece of a listed copy the title sent up keeps landing: about
/// ten seconds.
const SENT_FRAMES: u64 = 300;

/// The unit pieces are noted in (`crate::land::BLOCK`).
const PIECE_SHIFT: u32 = 16;

/// The listed lengths, from `text`: hex, comma separated, `0x` optional;
/// what does not read as one is left out.
fn parse(text: &str) -> Vec<u64> {
    text.split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .filter_map(|v| {
            let v = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")).unwrap_or(v);
            u64::from_str_radix(v, 16).ok()
        })
        .filter(|&len| len > 0)
        .collect()
}

fn list() -> &'static [u64] {
    static LIST: OnceLock<Vec<u64>> = OnceLock::new();
    LIST.get_or_init(|| {
        let list = parse(&std::env::var("VICTORY_LAND_NOT").unwrap_or_default());
        if !list.is_empty() {
            let shown: Vec<String> = list.iter().map(|l| format!("{l:#x}")).collect();
            tracing::warn!(
                "VICTORY_LAND_NOT: host copies of these lengths keep what shaders wrote on the card, but for pieces the \
                 title sends up: {}",
                shown.join(", ")
            );
        }
        list
    })
}

/// Whether a copy of `len` bytes is listed.
fn listed(len: u64) -> bool {
    let list = list();
    !list.is_empty() && list.contains(&len)
}

/// Whether the skipping is on now (a list is set; with the in-run A/B, its
/// phase).
fn skipping() -> bool {
    #[cfg(feature = "diagnostics")]
    if crate::ab::pick("VICTORY_LAND_NOT") == Some(false) {
        return false;
    }
    true
}

/// Of the listed copies, the pieces the title sent up: (title address of the
/// copy, piece) -> the frame it last did.
type Sent = HashMap<(u64, u32), u64>;

fn sent() -> &'static Mutex<Sent> {
    static SENT: OnceLock<Mutex<Sent>> = OnceLock::new();
    SENT.get_or_init(Default::default)
}

/// Whether the piece at `offset` of the copy at `start` was sent up within
/// [`SENT_FRAMES`] of `now`.
fn sent_lately(sent: &Sent, start: u64, offset: usize, now: u64) -> bool {
    sent.get(&(start, (offset >> PIECE_SHIFT) as u32)).is_some_and(|&at| now.saturating_sub(at) <= SENT_FRAMES)
}

/// Notes the pieces of `ranges` (offsets in the copy at `start`) sent up at
/// frame `now`.
fn note_sent(sent: &mut Sent, start: u64, ranges: &[(usize, usize)], now: u64) {
    for &(from, to) in ranges.iter().filter(|r| r.0 < r.1) {
        for p in (from >> PIECE_SHIFT) as u32..=((to - 1) >> PIECE_SHIFT) as u32 {
            sent.insert((start, p), now);
        }
    }
}

/// Whether what shaders wrote in the piece at `offset` of the host copy of
/// `span` (title address, length) stays on the card. Asked per piece by the
/// copy-back; each piece left there is counted.
pub(crate) fn left_on_card(span: (u64, u64), offset: usize, bytes: usize) -> bool {
    if !listed(span.1) || !skipping() {
        return false;
    }
    let now = crate::frame::current();
    if sent_lately(&sent().lock().unwrap_or_else(|e| e.into_inner()), span.0, offset, now) {
        return false;
    }
    caught(span);
    let n = LEFT[0].fetch_add(1, Relaxed) + 1;
    let b = LEFT[1].fetch_add(bytes as u64, Relaxed) + bytes as u64;
    if n.is_power_of_two() && n >= 64 {
        tracing::info!("VICTORY_LAND_NOT: {n} pieces ({} MB) left on the card so far", b >> 20);
    }
    true
}

/// The title's bytes in `ranges` (offsets) of the host copy of `span` went
/// up to the card: those pieces land for a while ([`left_on_card`]).
pub(crate) fn title_sent(span: (u64, u64), ranges: &[(usize, usize)]) {
    if !listed(span.1) {
        return;
    }
    let now = crate::frame::current();
    note_sent(&mut sent().lock().unwrap_or_else(|e| e.into_inner()), span.0, ranges, now);
}

/// Pieces left on the card, and their bytes, since the start.
static LEFT: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

/// Says in the log each copy a listed length catches, the first time.
fn caught(span: (u64, u64)) {
    static SEEN: OnceLock<Mutex<std::collections::HashSet<(u64, u64)>>> = OnceLock::new();
    let mut seen = SEEN.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if seen.insert(span) {
        let of_length = seen.iter().filter(|s| s.1 == span.1).count();
        tracing::info!(
            "a host copy of a listed length stays on the card (VICTORY_LAND_NOT): {:#x}+{:#x}, {of_length} of that length so far",
            span.0,
            span.1
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_are_read_as_hex_with_or_without_0x() {
        assert_eq!(
            parse("1412200, 0x2d00000,5265c0 ,,zz,0,0X1cc000"),
            vec![0x1412200, 0x2d00000, 0x5265c0, 0x1cc000]
        );
        assert!(parse("").is_empty());
        assert_eq!(parse("1412200,2d00000,5265c0,1cc000,3200000,c00000,2a3000").len(), 7);
    }

    #[test]
    fn pieces_the_title_sent_up_land_for_a_while() {
        let mut sent = Sent::new();
        let start = 0x2_0000_0000;
        // Bytes 0x1fff0..0x20010 went up at frame 100: pieces 1 and 2.
        note_sent(&mut sent, start, &[(0x1fff0, 0x20010), (0x50000, 0x50000)], 100);
        assert_eq!(sent.len(), 2);
        assert!(!sent_lately(&sent, start, 0x0, 100));
        assert!(sent_lately(&sent, start, 0x1_0000, 100));
        assert!(sent_lately(&sent, start, 0x2_ffff, 100 + SENT_FRAMES));
        assert!(!sent_lately(&sent, start, 0x2_0000, 101 + SENT_FRAMES));
        // An empty range notes nothing; another copy's pieces are its own.
        assert!(!sent_lately(&sent, start, 0x5_0000, 100));
        assert!(!sent_lately(&sent, start + 0x10_0000, 0x1_0000, 100));
        // Sent again later: it lands again.
        note_sent(&mut sent, start, &[(0x2_0000, 0x2_0001)], 1000);
        assert!(sent_lately(&sent, start, 0x2_0000, 1000));
    }

    #[test]
    fn nothing_is_left_on_the_card_without_a_list() {
        // The test process has no VICTORY_LAND_NOT.
        if std::env::var_os("VICTORY_LAND_NOT").is_none() {
            assert!(!left_on_card((0x1000_0000, 0x1412200), 0, 65536));
            title_sent((0x1000_0000, 0x1412200), &[(0, 16)]);
            assert!(sent().lock().unwrap().is_empty());
        }
    }
}
