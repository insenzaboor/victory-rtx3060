//! Renderer port #5: the title's constant buffers read by the card where the
//! title has them (`VICTORY_PORT_CBV=1`, off by default; read in [`on`]).
//!
//! What the console allows: a constant buffer anywhere on 16 bytes. What
//! D3D12 asks: a constant buffer view on 256. So the layer made every
//! constant buffer a draw's tables name afresh at each submission: a copy of
//! its bytes (out of title memory, on the CPU) into upload memory of its own
//! on 256, and a view of that ([`crate::draw`]'s tables, `defer_constants`):
//! ~23,000 a frame of the city, ~9 MB, and as many steps of the submitting
//! thread. With renderer port 1 (`VICTORY_PORT_CONSTANTS=1`) the memory they
//! are copied out of is the card's to read in place already.
//!
//! What is done here instead: shaders are translated to read constant buffers
//! as raw bytes (`victory_shader::emit::constants_raw`): a table's through
//! the raw twin of its slot, a root one as a root SRV. A raw view starts on 16
//! bytes, a root SRV on 4. So a constant buffer of a table in title memory in
//! place is a raw view of the title's bytes, made in the twin of its slot,
//! and nothing is copied or done at submission; a root one is bound at its
//! address as root SRVs are. Constant buffers anywhere else are copied as
//! before (the twin of their slot is a raw view of the copy, as it was).
//! The twin of a title slot that holds a constant buffer view is a raw view
//! of its bytes too (it was a constant buffer view: a raw read through one
//! on AMD has no end).
//!
//! What it assumes: what the card reads of a constant buffer at a draw is
//! what the title's CPU wrote before it submitted, as for every other title
//! buffer in place (renderer port 1): before, the copy at submission was
//! taken then whatever the title wrote after. The title writes its constants
//! into a ring it waits on its fences for.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Whether renderer port #5 is on: the shaders' side
/// (`victory_shader::emit::constants_raw`) and the host's views follow the
/// same switch.
pub(crate) fn on() -> bool {
    victory_shader::emit::constants_raw()
}

/// Whether a table's constant buffer in title memory in place is given to the
/// card where it is (on with the port; `VICTORY_PORT_CBV_PLACE=0`: copied as
/// before, and read through the raw twin all the same). With
/// `VICTORY_AB=<s>,VICTORY_PORT_CBV_PLACE` (diagnostics builds) on and off
/// while the title runs: both sides measured in one run.
pub(crate) fn in_place_on() -> bool {
    on() && crate::toggled!("VICTORY_PORT_CBV_PLACE")
}

/// The port on a host without raw twins (`VICTORY_PORT_CBV_SLOT=1`, see
/// `victory_shader::emit::constants_in_slot`; NVIDIA): a table's constant
/// buffer is a raw view in its own slot, of its bytes on the host copy they
/// have (in place or not), brought up to date as a buffer view's are, and
/// nothing is copied or done at submission for it.
pub(crate) fn slot_on() -> bool {
    victory_shader::emit::constants_in_slot() && victory_shader::emit::raw_views() == 0
}

/// Since the last line: tables' constant buffers given in place, their bytes;
/// copied as before because not in place, because not on 16 bytes or with
/// no twin; title slots' constant buffers whose twin is a raw view; tables'
/// constant buffers made raw views in their own slot ([`slot_on`]).
static STATS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

/// A table's constant buffer of `size` bytes was given in place.
pub(crate) fn in_place(size: u32) {
    STATS[0].fetch_add(1, Relaxed);
    STATS[1].fetch_add(u64::from(size), Relaxed);
}

/// A table's constant buffer of `size` bytes was made a raw view in its own
/// slot ([`slot_on`]).
pub(crate) fn in_slot(size: u32) {
    STATS[5].fetch_add(1, Relaxed);
    STATS[1].fetch_add(u64::from(size), Relaxed);
}

/// A table's constant buffer was copied as before: `elsewhere` when its
/// memory is not in place (with [`slot_on`]: has no host copy), else no raw
/// view of it could be made (not on 16 bytes, or no twin).
pub(crate) fn copied(elsewhere: bool) {
    STATS[if elsewhere { 2 } else { 3 }].fetch_add(1, Relaxed);
}

/// A title slot's constant buffer view got a raw twin.
pub(crate) fn twinned() {
    STATS[4].fetch_add(1, Relaxed);
}

/// Whether a constant buffer at host address `gpu` can be a raw view of
/// those bytes: on 16 bytes.
pub(crate) fn raw_ok(gpu: u64) -> bool {
    gpu.is_multiple_of(16)
}

/// For the frame-rate line.
pub(crate) fn log(frames: f64) {
    if !on() {
        return;
    }
    let s: Vec<f64> = STATS.iter().map(|s| s.swap(0, Relaxed) as f64 / frames.max(1e-9)).collect();
    tracing::info!(
        "renderer port 5 (VICTORY_PORT_CBV=1) per frame: {:.0} tables' constant buffers read by the card in place and {:.0} through \
         their own slot (VICTORY_PORT_CBV_SLOT) ({:.1} KB not copied, no step at submission); copied as before: {:.0} not in place \
         (with the slot: no host copy), {:.0} not on 16 bytes or with no twin; {:.0} title slots' constant buffers with a raw twin",
        s[0],
        s[5],
        s[1] / 1024.0,
        s[2],
        s[3],
        s[4],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_views_need_sixteen_bytes() {
        assert!(raw_ok(0x3500_0010));
        assert!(raw_ok(0x3500_0100));
        assert!(!raw_ok(0x3500_0008));
        assert!(!raw_ok(0x3500_0004));
    }
}
