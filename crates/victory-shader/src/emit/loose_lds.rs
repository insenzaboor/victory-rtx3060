//! Loose-LDS Wave32 fallback: console wave64 programs that read and write LDS
//! inside one lane-branch region, run as two host waves of 32 that meet only
//! where such a region is entered and left (no barriers inside divergent paths).
//! After deynesdev's fab7fda (Protonida).
//!
//! Two switches, both off by default because they change what the translator
//! makes of some programs; the shader cache keys on each only while it is on
//! (`victory-gpu`'s `shader_cache`), so the translations cached without them
//! keep their key. Making either the default needs a
//! [`crate::TRANSLATOR_VERSION`] bump.
//!
//! - `VICTORY_WAVE_LDS_LOOSE=1` ([`wave_lds_loose`]): the loose-LDS forms, tried
//!   last of all (`emit`), and what the approximate per-host-wave forms
//!   (`lower::with_local_waves_any`) take on top: a wave-wide branch, the SCC of a
//!   mask operation and a constant lane mask per host wave, and a 64-bit sum kept
//!   per host wave as the 32-bit reductions are.
//! - `VICTORY_WAVE32_FIXES=1` ([`wave32_fixes`]): the exact pieces of the same
//!   commit: `ds_bpermute_b32` within its own 32-lane half of a wave64 (as RDNA
//!   runs it), so it no longer keeps a program at 64 lanes; and D3D12's 32 KiB of
//!   group-shared memory kept to (no exchange dwords for programs whose host waves
//!   do not exchange, a program whose exchange dwords would not fit keeps 64
//!   lanes, and the count of `wave_base` in the title's dword 0 when the title
//!   takes all 32 KiB).

use std::cell::Cell;
use std::sync::OnceLock;

thread_local! {
    static LDS_LOOSE: Cell<bool> = const { Cell::new(false) };
}

/// `VICTORY_WAVE_LDS_LOOSE=1`: the loose-LDS forms of a console wave64 program on
/// a host of 32-lane waves (APPROXIMATE; off by default). Part of the shader
/// cache's key while on.
pub fn wave_lds_loose() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_WAVE_LDS_LOOSE").is_some_and(|v| v == "1"))
}

/// `VICTORY_WAVE32_FIXES=1`: `ds_bpermute_b32` within 32-lane halves and the
/// 32 KiB group-shared limit kept to (see the module; off by default). Part of
/// the shader cache's key while on.
pub fn wave32_fixes() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_WAVE32_FIXES").is_some_and(|v| v == "1"))
}

/// Whether the lowering runs inside [`with`].
pub(super) fn on() -> bool {
    LDS_LOOSE.get()
}

/// Runs `f` with LDS reads and writes inside one lane-branch region accepted
/// (`lower::lds_meetings`): the two host waves of a console wave meet only where
/// such a region is entered and left. APPROXIMATE: a value one half writes and
/// the other half reads inside the region may be read stale. The last resort
/// for console wave64 programs a 32-lane host would otherwise refuse
/// (`VICTORY_WAVE_LDS_LOOSE=1` turns it on).
pub(super) fn with<R>(f: impl FnOnce() -> R) -> R {
    let was = LDS_LOOSE.replace(true);
    let r = f();
    LDS_LOOSE.set(was);
    r
}
