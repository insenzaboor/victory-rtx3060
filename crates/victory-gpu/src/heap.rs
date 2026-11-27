//! Where the Rust side's blocks come from, in builds with the `mimalloc`
//! feature: mimalloc (per-thread caches) unless `VICTORY_ALLOC=system` asks
//! for the process heap, which the title and the drivers share. The choice
//! is read once, at the first allocation and before any block is handed out,
//! so every block goes back to the allocator it came from. Builds without
//! the feature never compile this: the process heap, as before.
//!
//! The way of it is Peppacodes/playablegames-peppa cd950293 (a port of
//! trustno1-coder/VIctory-NVIDIA PR 11 by Dastano): the submitting thread
//! spent about 15 % of its samples in the process heap. Peppa measured about
//! 2 ms a frame on the median (ae45b296; a first 5.6 ms was mostly load
//! difference). There the feature is on by default; here it is off.
//!
//! mimalloc keeps freed memory for a while before it hands it back, and our
//! own heap is some 1.6 GB of a 22-25 GB process, so the options that say
//! how it gives memory back are set here ([`RETURN`]), before its first
//! block; a `MIMALLOC_<NAME>` variable set for the run wins over each.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU8, Ordering::Relaxed};

/// mimalloc 3.3.2's option numbers (`mi_option_t` in its `mimalloc.h`;
/// libmimalloc-sys 0.1.49, pinned, does not name these). A test checks each
/// against the name mimalloc prints for it.
mod option {
    pub(super) const ARENA_EAGER_COMMIT: i32 = 4;
    pub(super) const PURGE_DECOMMITS: i32 = 5;
    pub(super) const ALLOW_LARGE_OS_PAGES: i32 = 6;
    pub(super) const PURGE_DELAY: i32 = 15;
}

/// How mimalloc gives memory back, as set at the first allocation: the
/// option, its name (`MIMALLOC_<NAME>` overrides it), and our value.
///
/// - `purge_delay` 250 ms (mimalloc 3's own is 1000; 2's was 10): memory
///   freed and not reused for a quarter of a second is purged. At our 10 fps
///   that is 2-3 frames, so the vectors a submission grows and frees every
///   frame are reused before then; what a load or a burst frees goes back
///   four times sooner. Peppa measured with 1000 (`MIMALLOC_PURGE_DELAY=1000`
///   gives that back).
/// - `purge_decommits` 1 (its default): a purge decommits, so the private
///   bytes fall; 0 would only reset the pages, which keeps them committed on
///   Windows.
/// - `arena_eager_commit` 0 (its default, 2, means only where the system
///   overcommits, which Windows does not): the 1 GiB arenas it reserves are
///   committed as used, not whole.
/// - `allow_large_os_pages` 0 (its default): large pages are never purged.
pub(crate) const RETURN: [(i32, &str, i64); 4] = [
    (option::PURGE_DELAY, "purge_delay", 250),
    (option::PURGE_DECOMMITS, "purge_decommits", 1),
    (option::ARENA_EAGER_COMMIT, "arena_eager_commit", 0),
    (option::ALLOW_LARGE_OS_PAGES, "allow_large_os_pages", 0),
];

/// 0: not read yet, 1: mimalloc, 2: the process heap.
static CHOICE: AtomicU8 = AtomicU8::new(0);

/// True when `VICTORY_ALLOC=system` was set when the process started.
#[inline]
fn system() -> bool {
    match CHOICE.load(Relaxed) {
        1 => false,
        2 => true,
        _ => read(),
    }
}

/// The first allocation reads the switch (and sets mimalloc's options
/// first); a thread that loses the race takes the winner's answer (both read
/// the same environment, and setting the options twice sets the same).
#[cold]
fn read() -> bool {
    let system = env_is("VICTORY_ALLOC", "system");
    if !system {
        set_return_options();
    }
    let wanted = if system { 2 } else { 1 };
    match CHOICE.compare_exchange(0, wanted, Relaxed, Relaxed) {
        Ok(_) => system,
        Err(set) => set == 2,
    }
}

/// [`RETURN`], each unless its `MIMALLOC_<NAME>` is set (mimalloc read that
/// itself when the process loaded).
fn set_return_options() {
    for &(option, name, value) in &RETURN {
        if !env_set_mimalloc(name) {
            // SAFETY: an option number of this mimalloc (see the test);
            // setting one allocates nothing.
            unsafe { libmimalloc_sys::mi_option_set(option, value as _) };
        }
    }
}

/// The longest variable name read here, with its 0.
const NAME_MAX: usize = 48;

/// `prefix` then `name` upper-cased, as a wide string ending in a 0, and its
/// length; `None` when it does not fit.
fn wide_name(prefix: &str, name: &str) -> Option<([u16; NAME_MAX], usize)> {
    let mut out = [0u16; NAME_MAX];
    let mut n = 0;
    for b in prefix.bytes().chain(name.bytes()) {
        if n + 1 >= NAME_MAX {
            return None;
        }
        out[n] = u16::from(b.to_ascii_uppercase());
        n += 1;
    }
    Some((out, n))
}

/// Whether `MIMALLOC_<name>` is set for the run (to anything).
fn env_set_mimalloc(name: &str) -> bool {
    wide_name("MIMALLOC_", name).is_some_and(|(wide, _)| env_value(&wide, &mut [0u16; 32]).is_some())
}

/// Whether the variable `name` is `value` (ASCII, any case).
fn env_is(name: &str, value: &str) -> bool {
    let Some((wide, _)) = wide_name("", name) else {
        return false;
    };
    let mut buffer = [0u16; 32];
    env_value(&wide, &mut buffer).is_some_and(|got| {
        got.len() == value.len() && got.iter().zip(value.bytes()).all(|(&w, b)| w < 0x80 && (w as u8).eq_ignore_ascii_case(&b))
    })
}

/// The variable `name` (a wide string ending in a 0) into `buffer`, without
/// allocating: this runs inside the allocator. A value too long for the
/// buffer is set, but not given.
#[cfg(windows)]
fn env_value<'a>(name: &[u16; NAME_MAX], buffer: &'a mut [u16; 32]) -> Option<&'a [u16]> {
    use windows_sys::Win32::System::Environment::GetEnvironmentVariableW;
    // SAFETY: `name` ends in a 0 and the buffer's length is the one passed.
    let n = unsafe { GetEnvironmentVariableW(name.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if n == 0 {
        // Unset (or set and empty, which mimalloc ignores as well).
        return None;
    }
    Some(if n < buffer.len() { &buffer[..n] } else { &[] })
}

#[cfg(not(windows))]
fn env_value<'a>(_name: &[u16; NAME_MAX], _buffer: &'a mut [u16; 32]) -> Option<&'a [u16]> {
    None
}

/// Which allocator the Rust side uses, for the log.
pub(crate) fn name() -> &'static str {
    if system() {
        "the process heap (VICTORY_ALLOC=system; built with mimalloc)"
    } else {
        "mimalloc 3.3 (purge_delay 250 ms, purge_decommits 1, arena_eager_commit 0 unless MIMALLOC_* says; \
         VICTORY_ALLOC=system gives the process heap)"
    }
}

/// mimalloc, or the process heap when `VICTORY_ALLOC=system`.
pub(crate) struct Chosen;

// SAFETY: every call goes unchanged to one allocator, the same one for the
// whole run (picked before the first block), so each block is released and
// grown by the allocator that made it. mimalloc's aligned calls honour any
// alignment, as `GlobalAlloc` asks.
unsafe impl GlobalAlloc for Chosen {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if system() {
            // SAFETY: the caller's contract, passed on.
            unsafe { System.alloc(layout) }
        } else {
            // SAFETY: a size and a power-of-two alignment, from the layout.
            unsafe { libmimalloc_sys::mi_malloc_aligned(layout.size(), layout.align()).cast() }
        }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if system() {
            // SAFETY: the caller's contract, passed on.
            unsafe { System.alloc_zeroed(layout) }
        } else {
            // SAFETY: as in `alloc`.
            unsafe { libmimalloc_sys::mi_zalloc_aligned(layout.size(), layout.align()).cast() }
        }
    }

    #[inline]
    unsafe fn dealloc(&self, at: *mut u8, layout: Layout) {
        if system() {
            // SAFETY: the caller's contract, passed on.
            unsafe { System.dealloc(at, layout) }
        } else {
            // SAFETY: `at` came from mimalloc (the choice never changes).
            unsafe { libmimalloc_sys::mi_free(at.cast()) }
        }
    }

    #[inline]
    unsafe fn realloc(&self, at: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if system() {
            // SAFETY: the caller's contract, passed on.
            unsafe { System.realloc(at, layout, size) }
        } else {
            // SAFETY: `at` came from mimalloc with this alignment.
            unsafe { libmimalloc_sys::mi_realloc_aligned(at.cast(), size, layout.align()).cast() }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_wide_upper_case_and_end_in_a_zero() {
        let (name, n) = wide_name("MIMALLOC_", "purge_delay").unwrap();
        assert_eq!(n, "MIMALLOC_PURGE_DELAY".len());
        assert_eq!(name[0], u16::from(b'M'));
        assert_eq!(name[9], u16::from(b'P'));
        assert_eq!(name[n], 0);
        assert!(wide_name("", &"X".repeat(NAME_MAX)).is_none());
    }

    #[test]
    fn blocks_go_back_to_the_allocator_that_made_them() {
        let layout = Layout::from_size_align(4096, 64).unwrap();
        // SAFETY: a non-zero layout; the block is grown and released with
        // the layout it has.
        unsafe {
            let at = Chosen.alloc_zeroed(layout);
            assert!(!at.is_null());
            assert_eq!(at as usize % 64, 0);
            assert!(std::slice::from_raw_parts(at, 4096).iter().all(|&b| b == 0));
            let grown = Chosen.realloc(at, layout, 1 << 20);
            assert!(!grown.is_null());
            assert_eq!(grown as usize % 64, 0);
            Chosen.dealloc(grown, Layout::from_size_align(1 << 20, 64).unwrap());
            let big = Layout::from_size_align(1 << 16, 1 << 16).unwrap();
            let at = Chosen.alloc(big);
            assert_eq!(at as usize % (1 << 16), 0);
            Chosen.dealloc(at, big);
        }
        assert_ne!(CHOICE.load(Relaxed), 0);
    }

    /// The option numbers are this mimalloc's: each is printed with its
    /// name, and set to a value only it has.
    #[test]
    fn the_option_numbers_are_mimallocs() {
        unsafe extern "C" {
            fn mi_options_print_out(out: libmimalloc_sys::mi_output_fun, arg: *mut std::ffi::c_void);
        }
        unsafe extern "C" fn collect(msg: *const std::ffi::c_char, arg: *mut std::ffi::c_void) {
            // SAFETY: `arg` is the String below; `msg` a C string.
            unsafe { (*arg.cast::<String>()).push_str(&std::ffi::CStr::from_ptr(msg).to_string_lossy()) };
        }
        // The first allocation (this test binary's own allocator is
        // `Chosen`) set ours, unless the run's environment did.
        let _ = Box::new(1u8);
        if !env_is("VICTORY_ALLOC", "system") {
            for &(option, name, value) in RETURN.iter().filter(|(_, name, _)| !env_set_mimalloc(name)) {
                // SAFETY: reading an option.
                assert_eq!(i64::from(unsafe { libmimalloc_sys::mi_option_get(option) }), value, "{name}");
            }
        }
        for &(option, name, _) in &RETURN {
            // SAFETY: setting an option; the previous value is put back.
            let was = unsafe { libmimalloc_sys::mi_option_get(option) };
            unsafe { libmimalloc_sys::mi_option_set(option, 7_654_321) };
            let mut printed = String::new();
            // SAFETY: `collect` takes the String passed.
            unsafe { mi_options_print_out(Some(collect), (&raw mut printed).cast()) };
            unsafe { libmimalloc_sys::mi_option_set(option, was) };
            assert!(printed.contains(&format!("option '{name}': 7654321")), "{name} is not option {option}:\n{printed}");
        }
    }
}
