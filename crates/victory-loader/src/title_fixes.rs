//! Fixes to bugs in the title's own code, made at load where the bytes are the ones expected
//! (another build of the title is left alone).
//!
//! The frame tracker's present (title+0x5267720, called through the device's vtable slot +0x80 on
//! every frame) takes the tracker's lock (title+0x4b40), then returns at once when the tracker has
//! no entries yet (`cmp dword [rdi+0x52c],0 ; je` to a plain `ret` at +0x5267cfb), without the
//! unlock its other way out makes (`jmp +0x4d9dab0` from +0x5267c7f). The count is 0 between the
//! display's release and the tracker's setup again, which the title goes through while it loads: a
//! frame job that presents in that window leaves the lock to the job worker that ran it, the device
//! loop blocks on the lock, the GPU frame counter stops, and title-main waits for ever on a slot
//! semaphore only that counter frees (title+0x56a0d < +0x225130). Runs thirtyeight to fortyfive
//! hung that way 14 times in 17 launches once loading had become faster. The branch is pointed at
//! the unlocking way out: same stack frame, the tracker still in rdi, and both callers ignore the
//! value returned (`VICTORY_TRACKER_UNLOCK=0`: left as the title has it).

/// A patch: where (offset in the image), the bytes that must be there, the bytes put there.
pub struct Fix {
    pub name: &'static str,
    pub at: usize,
    pub expect: &'static [u8],
    pub with: &'static [u8],
}

/// The frame tracker's early way out unlocks (see the module's documentation).
pub const TRACKER_UNLOCK: Fix = Fix {
    name: "frame tracker present: early return unlocks (VICTORY_TRACKER_UNLOCK)",
    at: 0x5267747,
    // je +0x5267cfb (the plain ret)
    expect: &[0x0f, 0x84, 0xae, 0x05, 0x00, 0x00],
    // je +0x5267c7f (the epilogue that jumps to the unlock)
    with: &[0x0f, 0x84, 0x32, 0x05, 0x00, 0x00],
};

/// Whether [`TRACKER_UNLOCK`] is made (on unless `VICTORY_TRACKER_UNLOCK=0`).
pub fn tracker_unlock() -> bool {
    std::env::var_os("VICTORY_TRACKER_UNLOCK").is_none_or(|v| v != "0")
}

/// What became of a fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    Done,
    /// The bytes there are not the expected ones: left alone.
    Differs,
    Failed(&'static str),
}

/// Makes `fix` in the image of `size` bytes at `base`, said in the log.
///
/// # Safety
/// The image stays mapped and none of its code runs yet.
pub unsafe fn apply(base: usize, size: usize, fix: &Fix) -> Applied {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let done = (|| {
        if fix.expect.len() != fix.with.len() || fix.at.checked_add(fix.expect.len()).is_none_or(|end| end > size) {
            return Applied::Failed("outside the image");
        }
        let at = base + fix.at;
        // SAFETY: inside the mapped image (checked above).
        let there = unsafe { std::slice::from_raw_parts(at as *const u8, fix.expect.len()) };
        if there == fix.with {
            return Applied::Done;
        }
        if there != fix.expect {
            return Applied::Differs;
        }
        let mut old = 0;
        // SAFETY: the title's code, which nothing runs yet; its page made writable and put back.
        unsafe {
            if VirtualProtect(at as *const _, fix.with.len(), PAGE_EXECUTE_READWRITE, &mut old) == 0 {
                return Applied::Failed("Windows would not make it writable");
            }
            std::ptr::copy_nonoverlapping(fix.with.as_ptr(), at as *mut u8, fix.with.len());
            VirtualProtect(at as *const _, fix.with.len(), old, &mut old);
            FlushInstructionCache(GetCurrentProcess(), at as *const _, fix.with.len());
        }
        Applied::Done
    })();
    let offset = format_args!("{:#x}", fix.at);
    match done {
        Applied::Done => tracing::info!(at = %offset, "title fix made: {}", fix.name),
        Applied::Differs => tracing::warn!(at = %offset, "title fix not made, the bytes there are not the ones expected (another build?): {}", fix.name),
        Applied::Failed(why) => tracing::warn!(at = %offset, why, "title fix not made: {}", fix.name),
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tracker_branch_lands_on_the_unlocking_epilogue() {
        // je rel32: target = at + 6 + rel.
        let rel = |b: &[u8]| i32::from_le_bytes([b[2], b[3], b[4], b[5]]) as i64;
        assert_eq!(TRACKER_UNLOCK.at as i64 + 6 + rel(TRACKER_UNLOCK.expect), 0x5267cfb);
        assert_eq!(TRACKER_UNLOCK.at as i64 + 6 + rel(TRACKER_UNLOCK.with), 0x5267c7f);
    }
}
