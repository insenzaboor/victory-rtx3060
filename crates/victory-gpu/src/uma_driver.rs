// This file contains code ported from XCL (https://github.com/xcl-dev/xcl),
// crates/xcl/src/hle/d3d12x/uma_driver.rs as of commit 06f90c198 (the
// relicensing commit; the file is the same there as after f2248f068 by
// Kalmarv, "coherent UMA windows in a driver heap at the title's address on
// NVIDIA", and f78e05bcc by Prime1997, "the driver heap's grooming reserves
// stretches a smaller window fits in"). XCL is copyright its contributors
// and licensed under the GNU General Public License, version 3 or (at your
// option) any later version; the file itself carries no other notice. Ported
// to VIctory on 2026-11-27: the snapshot of written pages, the release, the
// grooming of free address space (split into a plan that is tested on its
// own), the CUSTOM write-back L0 heap with write watch, the checks of where
// it was mapped and the restore are XCL's; what is new here is when it runs
// (when the title allocates, before the title has the address), the table
// of such heaps, the title's frees in them, the self-test and the limits.
// See docs/THIRD_PARTY.md.

//! Title memory in a driver heap at the title's own address
//! (`VICTORY_UMA_DRIVER=1`, off by default), instead of the title's pages
//! opened with `OpenExistingHeapFromAddress1` (`crate::inplace`).
//!
//! Why: on NVIDIA, GPU loads from imported pages ran 8-24x (atomics 13-22x)
//! slower than from a driver `CUSTOM` heap of write-back pages in the PC's
//! memory (pool L0), at the same CPU caching; a whole-frame replay took
//! 1,427 ms of GPU time imported and 172 ms in such a heap (XCL f2248f068,
//! RTX 5070 Ti). Our run q21 fits that: with port 1 and in-place memory off,
//! 22.9 ms per 1,000 draws against 27.0 with them on.
//!
//! How (XCL's way): the memory's written pages are copied out, the memory is
//! released, every other free stretch of address space it would fit in is
//! reserved for a moment (all but a margin at its start for other small
//! mappings), so that the driver's CPU mapping of a new heap of that size
//! can only land in the hole the memory left, and the pages are copied
//! back. If the mapping lands anywhere else, or a step fails, the memory is
//! made again at its address with its pages. The heap is made with
//! `D3D12_HEAP_FLAG_ALLOW_WRITE_WATCH`, so `crate::watch` still finds the
//! title's CPU writes there with `GetWriteWatch`; a driver that refuses the
//! flag, or whose mapping `GetWriteWatch` cannot read, keeps the old path.
//!
//! When, here: XCL remakes its one coherent window before any title code
//! runs. Our title memory is the title's own allocations, so the remake is
//! done inside `XMemVirtualAlloc` (`victory_hle`'s `xmem`), for an
//! allocation that reserves and commits at once, with the console's GPU
//! access bits, before the call returns: no title thread has the address
//! yet, so no write can be lost, and if the address cannot be had again the
//! allocation is simply made anew elsewhere ([`Lost::Report`]). The host
//! device is made, and [`self_test`] run, before the title's TLS callbacks
//! and `DllMain`s ([`before_title`]).
//!
//! What uses the heaps: `crate::inplace::place_for` (`VICTORY_INPLACE=1`
//! windows, `=2` title buffers too) places its buffers in them, with no
//! `ALLOW_CROSS_ADAPTER`, before it would import. Renderer port 1
//! (`crate::port`) does not: its memory is views of a section (the title's
//! physical pages), which a driver heap cannot be.
//!
//! Cost: a driver heap is resident at once, every page of it, where the
//! title's committed memory takes RAM only as it is touched. On the RTX 3060
//! with Resizable BAR off it is still the PC's memory, read by the card over
//! PCIe (about 25 GB/s at best); what is gained is the import's penalty, not
//! the link. [`limits`] keeps the total small and refuses when RAM is short.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock, RwLockReadGuard};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows_sys::Win32::System::Memory as mem;

const K64: usize = 64 << 10;
/// The host's write watching (not in windows-sys' memory module).
const MEM_WRITE_WATCH: u32 = 0x20_0000;
/// The exit code when memory a title holds could not be had again (XCL's).
const STOPPED: u32 = 0x7C1_0003;
/// The highest address the grooming looks at (XCL's).
const TOP: usize = 0x7FFF_FFFE_0000;

/// `VICTORY_UMA_DRIVER`: `1` (or `custom`) on; unset, `0` or anything else
/// off, everything as before. (XCL turns it on by default for NVIDIA outside
/// Wine; here it waits for runs on a card.)
pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| decide(std::env::var("VICTORY_UMA_DRIVER").ok().as_deref()))
}

fn decide(switch: Option<&str>) -> bool {
    matches!(switch.map(str::trim), Some("1" | "custom"))
}

fn megabytes(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(default) << 20
}

/// The limits of one run: the smallest allocation remade
/// (`VICTORY_UMA_DRIVER_MIN_MB`, 64), the most bytes in driver heaps at once
/// (`VICTORY_UMA_DRIVER_MB`, 1024) and the RAM and commit to leave free
/// (`VICTORY_UMA_DRIVER_KEEP_MB`, 2048).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub(crate) least: u64,
    pub(crate) most: u64,
    pub(crate) keep: u64,
}

pub(crate) fn limits() -> Limits {
    static L: OnceLock<Limits> = OnceLock::new();
    // The smallest is what keeps the grooming's margin (half the size) big
    // enough for other mappings made in the moment it takes.
    *L.get_or_init(|| Limits {
        least: megabytes("VICTORY_UMA_DRIVER_MIN_MB", 64).max(K64 as u64),
        most: megabytes("VICTORY_UMA_DRIVER_MB", 1024),
        keep: megabytes("VICTORY_UMA_DRIVER_KEEP_MB", 2048),
    })
}

/// Why an allocation stays the title's own memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    Small,
    Budget,
    Memory,
}

/// Whether `size` more bytes may go into driver heaps, with `made` there
/// already, `ram` bytes of RAM free and `commit` bytes of commit left.
pub(crate) fn refuse(size: u64, made: u64, ram: u64, commit: u64, l: Limits) -> Option<Refusal> {
    if size < l.least {
        Some(Refusal::Small)
    } else if made.saturating_add(size) > l.most {
        Some(Refusal::Budget)
    } else if ram < size.saturating_add(l.keep) || commit < size.saturating_add(l.keep) {
        Some(Refusal::Memory)
    } else {
        None
    }
}

/// Free RAM and commit left, in bytes.
fn headroom() -> (u64, u64) {
    use windows_sys::Win32::System::ProcessStatus::{K32GetPerformanceInfo, PERFORMANCE_INFORMATION};
    let mut p = PERFORMANCE_INFORMATION { cb: size_of::<PERFORMANCE_INFORMATION>() as u32, ..Default::default() };
    // SAFETY: a sized structure filled in by the call.
    if unsafe { K32GetPerformanceInfo(&mut p, p.cb) } == 0 {
        return (0, 0);
    }
    let page = p.PageSize as u64;
    (p.PhysicalAvailable as u64 * page, p.CommitLimit.saturating_sub(p.CommitTotal) as u64 * page)
}

// Free address space: the grooming.

/// While a remake grooms the address space, mappings of ours and the
/// title's allocations wait: they could not find room in the moment.
static GATE: RwLock<()> = RwLock::new(());

/// Held around a call that takes address space (an allocation, a view), so
/// that it does not fall into a remake's grooming; `None` with the switch
/// off.
pub fn gate() -> Option<RwLockReadGuard<'static, ()>> {
    on().then(|| GATE.read().unwrap_or_else(|e| e.into_inner()))
}

/// The reservations (start, bytes) that leave a window of `len` bytes no
/// free stretch to be mapped in but its hole `[hole, hole + len)`, given the
/// free stretches `free` (start, end) of the address space: every stretch
/// the window fits in (1 GiB or more for a larger window) outside the hole,
/// but its first 256 MiB, or less than half the window, room for other small
/// mappings. A window under 1 GiB found a stretch between its size and 1 GiB
/// unreserved, and the driver mapped its heap there (XCL f78e05bcc).
pub(crate) fn plan(free: &[(usize, usize)], hole: usize, len: usize) -> Vec<(usize, usize)> {
    let len = len.next_multiple_of(K64);
    let (least, margin) = (len.min(1 << 30), (256usize << 20).min(len / 2) & !(K64 - 1));
    let mut got = Vec::new();
    for &(base, end) in free {
        let end = end.min(TOP);
        if end <= base || end - base < least {
            continue;
        }
        let s = base.next_multiple_of(K64) + margin;
        for (s, e) in [(s, end.min(hole)), (s.max(hole + len), end)] {
            let mut s = s;
            while s < e {
                let n = (e - s).min(1 << 40);
                got.push((s, n));
                s += n;
            }
        }
    }
    got
}

/// The free stretches of the address space, (start, end).
fn free_stretches() -> Vec<(usize, usize)> {
    let (mut a, mut out) = (0x1_0000usize, Vec::new());
    while a < TOP {
        // SAFETY: plain data filled in by the query.
        let mut m: mem::MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: a valid out pointer of the right size.
        if unsafe { mem::VirtualQuery(a as *const c_void, &mut m, size_of::<mem::MEMORY_BASIC_INFORMATION>()) } == 0 {
            break;
        }
        let (base, end) = (m.BaseAddress as usize, (m.BaseAddress as usize + m.RegionSize).min(TOP));
        if end <= a {
            break;
        }
        a = end;
        if m.State == mem::MEM_FREE {
            out.push((base, end));
        }
    }
    out
}

/// Reserves [`plan`]'s stretches; what was reserved.
fn groom(hole: usize, len: usize) -> Vec<usize> {
    let mut got = Vec::new();
    for (s, n) in plan(&free_stretches(), hole, len) {
        // SAFETY: a reservation of free address space, ours.
        let p = unsafe { mem::VirtualAlloc(s as *const c_void, n, mem::MEM_RESERVE, mem::PAGE_NOACCESS) };
        if !p.is_null() {
            got.push(p as usize);
        }
    }
    got
}

fn ungroom(got: Vec<usize>) {
    for p in got {
        // SAFETY: reservations [`groom`] made.
        unsafe { mem::VirtualFree(p as *mut c_void, 0, mem::MEM_RELEASE) };
    }
}

// The remake.

/// The written pages of `[base, base + size)` that hold anything but zeros,
/// with their bytes (XCL's). Err if the memory is not write-watched.
///
/// # Safety
/// `[base, base + size)` is committed, readable memory.
unsafe fn written_pages(base: usize, size: usize) -> Result<Vec<(usize, Vec<u8>)>, String> {
    let mut out = Vec::new();
    let mut buf: Vec<*mut c_void> = vec![std::ptr::null_mut(); 1 << 16];
    let end = base + size;
    let mut at = base;
    while at < end {
        let (mut n, mut page) = (buf.len(), 0u32);
        // SAFETY: the answer buffer holds `n` entries.
        let status = unsafe { mem::GetWriteWatch(0, at as *const c_void, end - at, buf.as_mut_ptr(), &mut n, &mut page) };
        if status != 0 {
            return Err(format!("GetWriteWatch at {at:#x} failed ({status})"));
        }
        for &p in &buf[..n] {
            // SAFETY: a page of the memory, which the caller says is readable.
            let s = unsafe { std::slice::from_raw_parts(p as *const u8, page as usize) };
            if s.iter().any(|&b| b != 0) {
                out.push((p as usize, s.to_vec()));
            }
        }
        if n < buf.len() {
            break;
        }
        at = buf[n - 1] as usize + page as usize;
    }
    Ok(out)
}

/// What to do when the memory was released and cannot be had again at its
/// address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lost {
    /// Memory the title already holds (XCL's coherent window): end the
    /// process rather than let the title run on an unbacked address.
    #[cfg_attr(not(test), allow(dead_code))]
    Stop,
    /// Memory nobody holds yet (an allocation not returned yet): said, and
    /// the caller makes it anew.
    Report,
}

#[derive(Debug)]
pub(crate) enum Failure {
    /// The memory is as it was: never released, or made again at its
    /// address with its pages.
    Kept(String),
    /// Released, and not to be had again at its address ([`Lost::Report`]).
    Gone(String),
}

/// A driver heap mapped at the memory's address, a buffer over all of it,
/// mapped for good.
pub(crate) struct Remade {
    pub(crate) heap: ID3D12Heap,
    pub(crate) whole: ID3D12Resource,
    /// Groomed reservations, pages kept and their bytes, milliseconds.
    pub(crate) said: (usize, usize, usize, u128),
}

/// Memory `[base, base + size)` (a whole `VirtualAlloc` allocation of its
/// own, write-watched, `size` a multiple of 64 KiB) remade as a driver heap
/// mapped at `base`, its pages as they were and write-watched. On failure
/// the memory is the process's own again, its pages as they were, or, with
/// [`Lost::Report`], perhaps gone ([`Failure::Gone`]).
pub(crate) fn remake(device: &ID3D12Device, base: usize, size: usize, lost: Lost) -> Result<Remade, Failure> {
    remake_with_after_release(device, base, size, lost, || Ok(()))
}


fn remake_with_after_release(
    device: &ID3D12Device,
    base: usize,
    size: usize,
    lost: Lost,
    after_release: impl FnOnce() -> Result<(), String>,
) -> Result<Remade, Failure> {
    let t0 = std::time::Instant::now();
    let _groomed_alone = GATE.write().unwrap_or_else(|e| e.into_inner());
    // SAFETY: the caller's committed allocation.
    let pages = unsafe { written_pages(base, size) }.map_err(Failure::Kept)?;
    let kept: usize = pages.iter().map(|p| p.1.len()).sum();
    // SAFETY: the caller's own allocation; its pages are copied out above.
    if unsafe { mem::VirtualFree(base as *mut c_void, 0, mem::MEM_RELEASE) } == 0 {
        return Err(Failure::Kept(format!("VirtualFree of the memory at {base:#x} failed")));
    }
    let restore = |why: String| -> Failure {
        // A heap's mapping may go a moment after its last reference: a few
        // tries (here, not in XCL).
        let mut p = std::ptr::null_mut();
        for _ in 0..50 {
            // SAFETY: a reservation at an address the caller had.
            p = unsafe { mem::VirtualAlloc(base as *const c_void, size, mem::MEM_RESERVE | mem::MEM_COMMIT | MEM_WRITE_WATCH, mem::PAGE_READWRITE) };
            if p as usize == base {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        if p as usize != base {
            let error = std::io::Error::last_os_error();
            let line = format!("UMA driver: the memory at {base:#x} ({size} bytes) could not be restored at its address (got {p:?}: {error}) after {why}");
            match lost {
                Lost::Report => return Failure::Gone(line),
                Lost::Stop => {
                    tracing::error!("{line}; stopping before title memory is used");
                    eprintln!("{line}; stopping before title memory is used");
                    // SAFETY: ends this process.
                    unsafe {
                        windows_sys::Win32::System::Threading::TerminateProcess(windows_sys::Win32::System::Threading::GetCurrentProcess(), STOPPED);
                    }
                    std::process::abort();
                }
            }
        }
        for (a, bytes) in &pages {
            // SAFETY: pages of the memory just made again, as they were.
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), *a as *mut u8, bytes.len()) };
        }
        Failure::Kept(why)
    };
    let groomed = groom(base, size);
    let reservations = groomed.len();
    let made = (|| -> Result<(ID3D12Heap, ID3D12Resource, usize), String> {
        after_release()?;
        let properties = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_CUSTOM,
            CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_WRITE_BACK,
            MemoryPoolPreference: D3D12_MEMORY_POOL_L0,
            ..Default::default()
        };
        // Write-watched as the title's own memory was: its CPU writes to
        // textures there are found so.
        let flags = D3D12_HEAP_FLAG_ALLOW_ONLY_BUFFERS | D3D12_HEAP_FLAG_ALLOW_WRITE_WATCH;
        let desc = D3D12_HEAP_DESC { SizeInBytes: size as u64, Properties: properties, Alignment: K64 as u64, Flags: flags };
        let mut heap: Option<ID3D12Heap> = None;
        // SAFETY: plain creation with a valid description.
        unsafe { device.CreateHeap(&desc, &mut heap) }.map_err(|e| format!("CreateHeap (write-watched): {e}"))?;
        let heap = heap.ok_or("CreateHeap gave nothing")?;
        let buf_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: size as u64,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            ..Default::default()
        };
        let mut buf: Option<ID3D12Resource> = None;
        // SAFETY: plain creation in the live heap.
        unsafe { device.CreatePlacedResource(&heap, 0, &buf_desc, D3D12_RESOURCE_STATE_COMMON, None, &mut buf) }.map_err(|e| format!("CreatePlacedResource: {e}"))?;
        let buf = buf.ok_or("CreatePlacedResource gave nothing")?;
        let mut p: *mut c_void = std::ptr::null_mut();
        let range = D3D12_RANGE::default();
        // SAFETY: the buffer's one subresource.
        unsafe { buf.Map(0, Some(&range), Some(&mut p)) }.map_err(|e| format!("Map: {e}"))?;
        Ok((heap, buf, p as usize))
    })();
    // `GetWriteWatch` reads the mapping: the memory can stay watched.
    let watched = || {
        let (mut one, mut n, mut page) = ([std::ptr::null_mut::<c_void>(); 1], 1usize, 0u32);
        // SAFETY: a one-entry answer buffer.
        unsafe { mem::GetWriteWatch(0, base as *const c_void, size, one.as_mut_ptr(), &mut n, &mut page) == 0 }
    };
    // Checked, and restored on failure, while the address space is still
    // groomed (XCL ungrooms first): the hole is still free then.
    let result = match made {
        Ok((heap, whole, p)) if p == base && watched() => Ok((heap, whole)),
        Ok((heap, whole, p)) => {
            // SAFETY: mapped above.
            unsafe { whole.Unmap(0, None) };
            drop((whole, heap));
            Err(restore(if p == base {
                "GetWriteWatch cannot read the heap's mapping".to_owned()
            } else {
                format!("Map gave {p:#x}, not the memory's {base:#x} ({reservations} reservations)")
            }))
        }
        Err(e) => Err(restore(e)),
    };
    ungroom(groomed);
    let (heap, whole) = result?;
    for (a, bytes) in &pages {
        // SAFETY: pages of the heap's mapping at the memory's address.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), *a as *mut u8, bytes.len()) };
    }
    Ok(Remade { heap, whole, said: (reservations, pages.len(), kept, t0.elapsed().as_millis()) })
}

// The title's allocations in driver heaps.

struct Region {
    len: usize,
    heap: ID3D12Heap,
    /// Mapped for good: the title's address is its mapping.
    whole: ID3D12Resource,
}

// SAFETY: D3D12 heaps and resources are free-threaded.
unsafe impl Send for Region {}
// SAFETY: as above.
unsafe impl Sync for Region {}

/// By address.
static REGIONS: RwLock<BTreeMap<usize, Region>> = RwLock::new(BTreeMap::new());

/// One remake at a time, with its limits checked.
static MAKING: Mutex<()> = Mutex::new(());

/// Since the start: made, bytes; refused (count, bytes) for: small, no GPU
/// access bits, budget, memory, by the driver (kept), lost (made anew);
/// freed; title buffers and windows placed in them.
static STATS: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];
/// Bytes in driver heaps now.
static BYTES: AtomicU64 = AtomicU64::new(0);

/// Set by [`before_title`]: the self-test passed.
static READY: OnceLock<bool> = OnceLock::new();

fn ready() -> bool {
    READY.get().copied().unwrap_or(false)
}

/// What became of a title allocation ([`title_allocation`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Allocation {
    /// The title's own memory, as before.
    Plain,
    /// A driver heap at the same address.
    Driver,
    /// Released and not to be had again: the caller allocates anew.
    Gone,
}

fn count(at: usize, bytes: u64) {
    STATS[at].fetch_add(1, Ordering::Relaxed);
    STATS[at + 1].fetch_add(bytes, Ordering::Relaxed);
}

/// The title's allocation `[at, at + size)`, just made by `xmem` (reserved
/// and committed at once, read-write, write-watched, at an address of its
/// choosing) and not yet returned to the title; `gpu` if the title asked for
/// the console's GPU access bits. With the switch on and the self-test
/// passed, made a driver heap at the same address if [`refuse`] lets it.
/// The caller holds no [`gate`].
pub fn title_allocation(at: usize, size: usize, gpu: bool) -> Allocation {
    if !ready() {
        return Allocation::Plain;
    }
    let size = size.next_multiple_of(K64);
    if !gpu {
        count(4, size as u64);
        return Allocation::Plain;
    }
    let _alone = MAKING.lock().unwrap_or_else(|e| e.into_inner());
    let (ram, commit) = headroom();
    if let Some(why) = refuse(size as u64, BYTES.load(Ordering::Relaxed), ram, commit, limits()) {
        count(
            match why {
                Refusal::Small => 2,
                Refusal::Budget => 6,
                Refusal::Memory => 8,
            },
            size as u64,
        );
        return Allocation::Plain;
    }
    let Some(host) = crate::host::host() else {
        return Allocation::Plain;
    };
    match remake(&host.device, at, size, Lost::Report) {
        Ok(Remade { heap, whole, said: (reservations, _, _, ms) }) => {
            REGIONS.write().unwrap_or_else(|e| e.into_inner()).insert(at, Region { len: size, heap, whole });
            BYTES.fetch_add(size as u64, Ordering::Relaxed);
            count(0, size as u64);
            let n = STATS[0].load(Ordering::Relaxed);
            if n <= 16 || n.is_power_of_two() {
                tracing::info!(
                    at = format_args!("{at:#x}"),
                    mb = size >> 20,
                    reservations,
                    ms,
                    total_mb = BYTES.load(Ordering::Relaxed) >> 20,
                    "UMA driver: a title allocation is a driver CUSTOM write-back L0 heap mapped at its address, write-watched"
                );
            }
            Allocation::Driver
        }
        Err(Failure::Kept(why)) => {
            count(10, size as u64);
            if STATS[10].load(Ordering::Relaxed) <= 8 {
                tracing::warn!(at = format_args!("{at:#x}"), mb = size >> 20, why, "UMA driver: a title allocation stays the title's own memory");
            }
            Allocation::Plain
        }
        Err(Failure::Gone(why)) => {
            count(12, size as u64);
            tracing::warn!(at = format_args!("{at:#x}"), mb = size >> 20, why, "UMA driver: a title allocation is made anew elsewhere");
            Allocation::Gone
        }
    }
}

/// The region holding `at`: its base and bytes.
fn region_of(regions: &BTreeMap<usize, Region>, at: usize) -> Option<(usize, usize)> {
    let (&base, r) = regions.range(..=at).next_back()?;
    (at < base + r.len).then_some((base, r.len))
}

/// Whether `[at, at + len)` lies in one driver heap.
pub fn holds(at: usize, len: usize) -> bool {
    if BYTES.load(Ordering::Relaxed) == 0 {
        return false;
    }
    let regions = REGIONS.read().unwrap_or_else(|e| e.into_inner());
    region_of(&regions, at).is_some_and(|(base, n)| at.saturating_add(len) <= base + n)
}

/// The title's `VirtualFree(address, size, kind)` of memory in a driver
/// heap, done here: `Some` with the call's result, `None` if the memory is
/// no driver heap's. A release of the whole lets the heap go; a decommit
/// zeroes the pages (they stay committed: a heap cannot lose pages, and a
/// commit there again is a no-op, [`holds`]).
pub fn title_free(address: usize, size: usize, kind: u32) -> Option<bool> {
    if BYTES.load(Ordering::Relaxed) == 0 {
        return None;
    }
    let mut regions = REGIONS.write().unwrap_or_else(|e| e.into_inner());
    let (base, len) = region_of(&regions, address)?;
    if kind & mem::MEM_RELEASE != 0 {
        if address != base || size != 0 {
            // SAFETY: plain call.
            unsafe { windows_sys::Win32::Foundation::SetLastError(windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER) };
            return Some(false);
        }
        let region = regions.remove(&base)?;
        // SAFETY: mapped when made; the title lets go of the memory.
        unsafe { region.whole.Unmap(0, None) };
        drop(region);
        BYTES.fetch_sub(len as u64, Ordering::Relaxed);
        count(14, len as u64);
        return Some(true);
    }
    if kind & mem::MEM_DECOMMIT != 0 {
        let from = address & !0xFFF;
        let to = if size == 0 { base + len } else { (address + size).next_multiple_of(0x1000).min(base + len) };
        // SAFETY: pages of the heap's mapping, which stays.
        unsafe { std::ptr::write_bytes(from as *mut u8, 0, to.saturating_sub(from)) };
        return Some(true);
    }
    None
}

/// Title memory `[start, start + len)` in a driver heap, if all of it lies
/// in one: a buffer placed over its 64 KiB pages (no `ALLOW_CROSS_ADAPTER`:
/// the heap is no import).
pub(crate) fn place_for(start: u64, len: u64) -> Option<crate::inplace::Placed> {
    if BYTES.load(Ordering::Relaxed) == 0 {
        return None;
    }
    let at = start & !(K64 as u64 - 1);
    let end = (start + len.max(1)).next_multiple_of(K64 as u64);
    let regions = REGIONS.read().unwrap_or_else(|e| e.into_inner());
    let (base, n) = region_of(&regions, at as usize)?;
    if end > (base + n) as u64 {
        return None;
    }
    let heap = regions.get(&base)?.heap.clone();
    drop(regions);
    let host = crate::host::host()?;
    let buffer = crate::inplace::place_with(&host.device, &heap, at - base as u64, end - at, false).ok()?;
    STATS[15].fetch_add(1, Ordering::Relaxed);
    // SAFETY: a live buffer.
    let gpu = unsafe { buffer.GetGPUVirtualAddress() } + (start - at);
    Some(crate::inplace::Placed { buffer, heap, gpu, bias: start - at })
}

/// For the in-place line every ten seconds.
pub(crate) fn line() -> Option<String> {
    if !on() {
        return None;
    }
    let s: Vec<u64> = STATS.iter().map(|s| s.load(Ordering::Relaxed)).collect();
    let mb = |b: u64| b >> 20;
    Some(format!(
        "UMA driver heaps (VICTORY_UMA_DRIVER=1, self-test {}): {} title allocations made driver heaps ({} MB, {} MB now), {} buffers placed in them; \
         left the title's own: {} under VICTORY_UMA_DRIVER_MIN_MB ({} MB), {} without GPU access bits ({} MB), {} over VICTORY_UMA_DRIVER_MB ({} MB), \
         {} short of RAM or commit ({} MB), {} refused by the driver or mapped elsewhere ({} MB); {} made anew elsewhere; {} freed",
        match READY.get() {
            Some(true) => "passed",
            Some(false) => "FAILED, off",
            None => "not run",
        },
        s[0],
        mb(s[1]),
        mb(BYTES.load(Ordering::Relaxed)),
        s[15],
        s[2],
        mb(s[3]),
        s[4],
        mb(s[5]),
        s[6],
        mb(s[7]),
        s[8],
        mb(s[9]),
        s[10],
        mb(s[11]),
        s[12],
        s[14],
    ))
}

// Before the title.

/// Before the title's first code (its TLS callbacks and `DllMain`s): with
/// the switch on, the host device is made and [`self_test`] run; title
/// allocations become driver heaps only if it passed.
pub fn before_title() {
    if !on() {
        return;
    }
    let verdict = (|| -> Result<Vec<String>, String> {
        if crate::inplace::level() == 0 {
            return Err("VICTORY_INPLACE is 0: nothing places title memory in a heap, so no driver heap would be read (set VICTORY_INPLACE=1 or 2)".to_owned());
        }
        let host = crate::host::host().ok_or("no host device")?;
        self_test(&host.device)
    })();
    if crate::port::on() {
        tracing::info!(
            "UMA driver: renderer port 1's 32-bit heap stays imported (OpenExistingHeapFromAddress1 on a view of its section): \
             a section's pages cannot be a driver heap"
        );
    }
    let l = limits();
    match verdict {
        Ok(lines) => {
            for line in lines {
                tracing::info!("UMA driver self-test: {line}");
            }
            tracing::info!(
                least_mb = l.least >> 20,
                most_mb = l.most >> 20,
                keep_mb = l.keep >> 20,
                "UMA driver: on; title allocations reserved and committed at once with GPU access bits become driver heaps"
            );
            let _ = READY.set(true);
        }
        Err(why) => {
            tracing::error!(why, "UMA driver: the self-test failed; off for the run, title memory as before");
            let _ = READY.set(false);
        }
    }
}

fn fill(at: usize, len: usize, seed: u8) {
    for i in 0..len {
        // SAFETY: the caller's own committed memory of `len` bytes.
        unsafe { ((at + i) as *mut u8).write((i as u8).wrapping_mul(31).wrapping_add(seed)) };
    }
}

fn same(at: usize, bytes: &[u8]) -> bool {
    // SAFETY: the caller's own committed memory of that length.
    unsafe { std::slice::from_raw_parts(at as *const u8, bytes.len()) == bytes }
}

/// The start-up self-test (`docs/design/in-place-memory.md`), on memory of
/// its own made as `xmem` makes title memory and remade through the same
/// steps: the GPU sees the CPU's bytes (those kept through the remake, and
/// a second write with no call in between), the GPU's writes (a copy, a
/// shader through unordered access) show at the address, the memory stays
/// committed and read-write, the write watch sees CPU writes, and the guard
/// `crate::watch` sets takes. Then the card's copy rate out of it and out of
/// an import of the same size, said and not judged. Ok with the lines, Err
/// naming the step that failed.
pub(crate) fn self_test(device: &ID3D12Device) -> Result<Vec<String>, String> {
    const SIZE: usize = 64 << 20;
    let mut lines = Vec::new();
    // SAFETY: memory of the test's own.
    let base = unsafe { mem::VirtualAlloc(std::ptr::null(), SIZE, mem::MEM_RESERVE | mem::MEM_COMMIT | MEM_WRITE_WATCH, mem::PAGE_READWRITE) } as usize;
    if base == 0 {
        return Err("no memory to test with".to_owned());
    }
    let marks = [0, SIZE / 2, SIZE - K64];
    for (i, &m) in marks.iter().enumerate() {
        fill(base + m, K64, 11 + i as u8);
    }
    let remade = match remake(device, base, SIZE, Lost::Report) {
        Ok(r) => r,
        Err(Failure::Kept(why)) => {
            // SAFETY: the test's own memory, as it was.
            unsafe { mem::VirtualFree(base as *mut c_void, 0, mem::MEM_RELEASE) };
            return Err(format!("the remake: {why}"));
        }
        Err(Failure::Gone(why)) => return Err(format!("the remake: {why}")),
    };
    let (reservations, pages, kept, ms) = remade.said;
    lines.push(format!("{SIZE} bytes remade at {base:#x} in {ms} ms: {reservations} reservations, {pages} written pages kept ({} KB)", kept >> 10));
    let steps = (|| -> Result<(), String> {
        let mut rig = crate::inplace::Rig::new(device).map_err(|e| format!("no queue of its own: {e}"))?;
        let whole = &remade.whole;
        let seen = |rig: &mut crate::inplace::Rig, step: &str| -> Result<(), String> {
            for &m in &marks {
                let got = rig.fetch(whole, m as u64, K64).map_err(|e| format!("{step}: the GPU's copy failed: {e}"))?;
                if !same(base + m, &got) {
                    return Err(format!("{step}: the GPU read other bytes than the CPU wrote at +{m:#x}"));
                }
            }
            Ok(())
        };
        seen(&mut rig, "step 1, the CPU's bytes kept through the remake")?;
        for (i, &m) in marks.iter().enumerate() {
            fill(base + m, K64, 101 + i as u8);
        }
        seen(&mut rig, "step 2, a second CPU write, no call in between")?;
        let new: Vec<u8> = (0..K64).map(|i| (i as u8) ^ 0x5a).collect();
        rig.store(whole, (SIZE / 2) as u64, &new).map_err(|e| format!("step 3: the GPU's copy failed: {e}"))?;
        if !same(base + SIZE / 2, &new) {
            return Err("step 3: the GPU's copy does not show at the address".to_owned());
        }
        // SAFETY: the first three words of the heap's mapping.
        unsafe { (base as *mut [u32; 3]).write([40, 2, 0]) };
        // SAFETY: a live buffer.
        rig.shade(unsafe { whole.GetGPUVirtualAddress() }).map_err(|e| format!("step 3: the shader failed: {e}"))?;
        // SAFETY: as above.
        if unsafe { (base as *const [u32; 3]).read_volatile() }[2] != 42 {
            return Err("step 3: the shader's write through unordered access does not show at the address".to_owned());
        }
        let mut kind = 0;
        let mut here = base;
        while here < base + SIZE {
            // SAFETY: plain data filled in by the query.
            let mut info: mem::MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
            // SAFETY: a valid out pointer of the right size.
            if unsafe { mem::VirtualQuery(here as *const c_void, &mut info, size_of_val(&info)) } == 0
                || info.State != mem::MEM_COMMIT
                || info.Protect != mem::PAGE_READWRITE
            {
                return Err(format!("step 4: VirtualQuery at {here:#x}: state {:#x}, protection {:#x}", info.State, info.Protect));
            }
            kind = info.Type;
            here = (info.BaseAddress as usize).saturating_add(info.RegionSize);
        }
        // SAFETY: the heap's mapping.
        unsafe { mem::ResetWriteWatch(base as *const c_void, SIZE) };
        let page = base + 3 * K64;
        // SAFETY: as above.
        unsafe { (page as *mut u8).write_volatile(7) };
        let (mut found, mut n, mut granularity) = ([std::ptr::null_mut::<c_void>(); 16], 16usize, 0u32);
        // SAFETY: a sized answer buffer.
        let asked = unsafe { mem::GetWriteWatch(0, base as *const c_void, SIZE, found.as_mut_ptr(), &mut n, &mut granularity) } == 0;
        if !asked || !found[..n].iter().any(|&p| (p as usize) <= page && page < p as usize + granularity as usize) {
            return Err(format!("step 5: the write watch did not see a CPU write (asked {asked}, {n} pages)"));
        }
        let mut old = 0;
        // SAFETY: one block of the heap's mapping, made read-only and back.
        let guarded = unsafe { mem::VirtualProtect((base + 4 * K64) as *const c_void, K64, mem::PAGE_READONLY, &mut old) } != 0;
        // SAFETY: as above.
        let back = unsafe { mem::VirtualProtect((base + 4 * K64) as *const c_void, K64, mem::PAGE_READWRITE, &mut old) } != 0;
        if !(guarded && back) {
            return Err(format!("step 6: VirtualProtect read-only {guarded}, read-write again {back}"));
        }
        lines.push(format!("steps 1-6 passed (the memory's type {kind:#x}: 0x20000 private, 0x40000 mapped)"));
        // The rate, said only.
        const RATE: usize = 16 << 20;
        let gbs = |ms: f64| RATE as f64 / (ms / 1000.0) / 1e9;
        let driver = rig.copy_ms(whole, RATE as u64).map(gbs);
        let imported = (|| {
            // SAFETY: memory of the test's own, opened and let go here.
            let at = unsafe { mem::VirtualAlloc(std::ptr::null(), RATE, mem::MEM_RESERVE | mem::MEM_COMMIT, mem::PAGE_READWRITE) } as usize;
            if at == 0 {
                return None;
            }
            let rate = crate::inplace::open(device, crate::inplace::From::Part(at, RATE))
                .and_then(|heap| crate::inplace::place(device, &heap, 0, RATE as u64))
                .and_then(|buffer| rig.copy_ms(&buffer, RATE as u64))
                .map(gbs)
                .ok();
            // The import lets go of the pages a moment after its last reference.
            for _ in 0..100 {
                // SAFETY: as above.
                if unsafe { mem::VirtualFree(at as *mut c_void, 0, mem::MEM_RELEASE) } != 0 {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            rate
        })();
        let say = |r: Option<f64>| r.map_or("?".to_owned(), |r| format!("{r:.1} GB/s"));
        lines.push(format!(
            "the card copies 16 MB out of the driver heap at {}, out of the same memory imported (OpenExistingHeapFromAddress1) at {}",
            say(driver.ok()),
            say(imported)
        ));
        Ok(())
    })();
    // SAFETY: mapped when made; the test's memory goes.
    unsafe { remade.whole.Unmap(0, None) };
    drop(remade);
    steps.map(|()| lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Off unless asked for: `1` or `custom` on, anything else off.
    #[test]
    fn the_switch_is_off_unless_asked_for() {
        assert!(!decide(None));
        assert!(!decide(Some("0")));
        assert!(!decide(Some("yes")));
        assert!(decide(Some("1")));
        assert!(decide(Some(" 1 ")));
        assert!(decide(Some("custom")));
    }

    const MB: usize = 1 << 20;
    const GB: usize = 1 << 30;

    /// What stays free after the plan's reservations: the free stretches
    /// less the reserved ones.
    fn left(free: &[(usize, usize)], plan: &[(usize, usize)]) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for &(base, end) in free {
            let mut at = base;
            let mut cuts: Vec<(usize, usize)> = plan.iter().copied().filter(|&(s, n)| s < end && s + n > base).collect();
            cuts.sort();
            for (s, n) in cuts {
                if s > at {
                    out.push((at, s));
                }
                at = at.max(s + n);
            }
            if at < end {
                out.push((at, end));
            }
        }
        out
    }

    /// The window's hole stays free, and nothing else it fits in.
    fn only_the_hole_fits(free: &[(usize, usize)], hole: usize, len: usize) {
        let plan = plan(free, hole, len);
        for &(s, n) in &plan {
            assert!(s + n <= hole || s >= hole + len, "a reservation {s:#x}+{n:#x} in the hole {hole:#x}+{len:#x}");
            assert_eq!(s % K64, 0, "reservations start on 64 KiB");
        }
        let fits: Vec<_> = left(free, &plan).into_iter().filter(|&(s, e)| e - s >= len).collect();
        assert!(fits.iter().all(|&(s, e)| s <= hole && hole + len <= e), "free room for the window outside its hole: {fits:x?}");
        assert!(fits.iter().any(|&(s, e)| s <= hole && hole + len <= e), "the hole is free");
    }

    #[test]
    fn the_plan_leaves_only_the_hole_for_a_big_window() {
        let hole = 0x13_0000_0000;
        let len = 4 * GB;
        // The hole within a big free stretch, others above and below, one
        // too small for the window.
        let free = [(0x1_0000_0000, 0x1_4000_0000), (0x2_0000_0000, 0x10_0000_0000), (0x12_0000_0000, 0x40_0000_0000), (0x100_0000_0000, 0x7FFF_0000_0000)];
        only_the_hole_fits(&free, hole, len);
        let plan = plan(&free, hole, len);
        // A window over 1 GiB: a stretch of 1 GiB is the least reserved
        // (but for its margin).
        assert!(plan.iter().any(|&(s, _)| s == 0x1_0000_0000 + 256 * MB), "{plan:x?}");
        // A margin at each stretch's start, also before the hole.
        assert!(plan.iter().any(|&(s, n)| s == 0x12_0000_0000 + 256 * MB && s + n == hole));
        assert!(plan.iter().any(|&(s, _)| s == hole + len));
        // A stretch under 1 GiB cannot hold it: left alone.
        assert!(super::plan(&[(0x2000_0000, 0x2000_0000 + 768 * MB)], hole, len).is_empty());
    }

    /// XCL f78e05bcc: a window under 1 GiB fits in a stretch between its size
    /// and 1 GiB, which the old rule (1 GiB or more) left free.
    #[test]
    fn the_plan_reserves_stretches_a_window_under_1_gib_fits_in() {
        let len = 512 * MB;
        let hole = 0x13_0000_0000;
        let stretch = (0x4a72_0000, 0x4a72_0000 + 768 * MB);
        let free = [stretch, (hole, hole + len)];
        only_the_hole_fits(&free, hole, len);
        let plan = plan(&free, hole, len);
        assert_eq!(plan[0], (stretch.0 + 256 * MB, 512 * MB), "the stretch less its margin");
        // A stretch smaller than the window is left alone.
        assert!(super::plan(&[(0x1000_0000, 0x1000_0000 + 500 * MB)], hole, len).is_empty());
    }

    /// The margin is under half a small window, on 64 KiB, and a stretch
    /// that does not start on 64 KiB is reserved from the next.
    #[test]
    fn the_plan_keeps_a_margin_under_half_a_small_window() {
        let len = 64 * MB;
        let hole = 0x8_0000_0000;
        let free = [(0x2_0000_1000, 0x2_1000_0000), (hole - 32 * MB, hole + len + 32 * MB)];
        only_the_hole_fits(&free, hole, len);
        let plan = plan(&free, hole, len);
        assert_eq!(plan[0].0, 0x2_0001_0000 + 32 * MB);
        // Below the hole: less than the margin, nothing reserved there.
        assert!(plan.iter().all(|&(s, n)| s + n <= 0x2_1000_0000 || s >= hole + len));
        // A size off 64 KiB: the hole is its 64 KiB pages.
        let plan = super::plan(&free, hole, len - 100);
        assert!(plan.iter().all(|&(s, n)| s + n <= hole || s >= hole + len));
    }

    /// Stretches over 1 TiB are reserved in pieces of 1 TiB; the top of the
    /// address space is not passed.
    #[test]
    fn the_plan_cuts_huge_stretches() {
        let len = GB;
        let hole = 0x10_0000_0000;
        let free = [(hole, 0x7FFF_FFFF_0000)];
        let plan = plan(&free, hole, len);
        assert!(plan.iter().all(|&(_, n)| n <= 1 << 40));
        assert_eq!(plan.first().unwrap().0, hole + len);
        let (s, n) = *plan.last().unwrap();
        assert_eq!(s + n, TOP);
        only_the_hole_fits(&free, hole, len);
    }

    #[test]
    fn limits_refuse_small_over_budget_and_short_of_memory() {
        let l = Limits { least: 64 << 20, most: 1 << 30, keep: 2 << 30 };
        let plenty = 16u64 << 30;
        assert_eq!(refuse(16 << 20, 0, plenty, plenty, l), Some(Refusal::Small));
        assert_eq!(refuse(64 << 20, 0, plenty, plenty, l), None);
        assert_eq!(refuse(512 << 20, 600 << 20, plenty, plenty, l), Some(Refusal::Budget));
        assert_eq!(refuse(512 << 20, 512 << 20, plenty, plenty, l), None);
        // 22-25 GB of 32 used: 2.4 GB free is not 512 MB plus 2 GB kept.
        assert_eq!(refuse(512 << 20, 0, 2400 << 20, plenty, l), Some(Refusal::Memory));
        assert_eq!(refuse(512 << 20, 0, plenty, 1 << 30, l), Some(Refusal::Memory));
    }

    /// XCL's: memory that is not write-watched is not released (its written
    /// pages could not be found).
    #[test]
    fn warp_failed_write_watch_does_not_release_the_memory() {
        let _turn = crate::indirect::tests::warp_turn();
        let Some(device) = crate::inplace::tests::warp() else { return };
        const SIZE: usize = 1 << 16;
        // SAFETY: memory of the test's own.
        unsafe {
            let base = mem::VirtualAlloc(std::ptr::null(), SIZE, mem::MEM_RESERVE | mem::MEM_COMMIT, mem::PAGE_READWRITE) as usize;
            assert_ne!(base, 0);
            std::ptr::write_bytes(base as *mut u8, 0x5a, SIZE);
            assert!(matches!(remake(&device, base, SIZE, Lost::Stop), Err(Failure::Kept(_))));
            let mut info: mem::MEMORY_BASIC_INFORMATION = std::mem::zeroed();
            assert_ne!(mem::VirtualQuery(base as *const c_void, &mut info, size_of_val(&info)), 0);
            assert_eq!(info.AllocationBase as usize, base);
            assert_eq!(info.State, mem::MEM_COMMIT);
            assert!(std::slice::from_raw_parts(base as *const u8, SIZE).iter().all(|&b| b == 0x5a));
            assert_ne!(mem::VirtualFree(base as *mut c_void, 0, mem::MEM_RELEASE), 0);
        }
    }

    /// Runs the test `name` again in a process of its own with `var` set
    /// (the grooming reserves the address space of the whole process); its
    /// exit code and what it printed.
    fn alone(name: &str, var: &str) -> (Option<i32>, String) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("uma_driver::tests::{name}"), "--nocapture", "--test-threads=1"])
            .env(var, "1")
            .output()
            .unwrap();
        (output.status.code(), format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)))
    }

    /// XCL's: memory a title holds that cannot be had again at its address
    /// ends the process before anything runs on it.
    #[test]
    fn warp_failed_restore_stops_before_the_title_runs_on_unbacked_memory() {
        const SIZE: usize = 1 << 16;
        if std::env::var_os("VICTORY_TEST_RESTORE_COLLISION").is_some() {
            let device = crate::inplace::tests::warp().unwrap();
            // SAFETY: memory of the test's own.
            unsafe {
                let base = mem::VirtualAlloc(std::ptr::null(), SIZE, mem::MEM_RESERVE | mem::MEM_COMMIT | MEM_WRITE_WATCH, mem::PAGE_READWRITE) as usize;
                assert_ne!(base, 0);
                (base as *mut u8).write(0x5a);
                let result = remake_with_after_release(&device, base, SIZE, Lost::Stop, || {
                    let occupied = mem::VirtualAlloc(base as *const c_void, SIZE, mem::MEM_RESERVE, mem::PAGE_NOACCESS);
                    assert_eq!(occupied as usize, base);
                    Err("synthetic post-release failure".into())
                });
                eprintln!("continued after failed restore: {:?}", result.err());
            }
            return;
        }
        if crate::inplace::tests::warp().is_none() {
            return;
        }
        let (code, transcript) = alone("warp_failed_restore_stops_before_the_title_runs_on_unbacked_memory", "VICTORY_TEST_RESTORE_COLLISION");
        assert_eq!(code, Some(STOPPED as i32), "{transcript}");
        assert!(transcript.contains("could not be restored at its address"), "{transcript}");
        assert!(!transcript.contains("continued after failed restore"), "{transcript}");
    }

    /// The same collision with [`Lost::Report`] (memory nobody holds yet):
    /// said, and the process goes on.
    #[test]
    fn warp_a_failed_restore_of_fresh_memory_is_reported() {
        const SIZE: usize = 1 << 16;
        if std::env::var_os("VICTORY_TEST_RESTORE_REPORT").is_some() {
            let device = crate::inplace::tests::warp().unwrap();
            // SAFETY: memory of the test's own.
            unsafe {
                let base = mem::VirtualAlloc(std::ptr::null(), SIZE, mem::MEM_RESERVE | mem::MEM_COMMIT | MEM_WRITE_WATCH, mem::PAGE_READWRITE) as usize;
                let result = remake_with_after_release(&device, base, SIZE, Lost::Report, || {
                    mem::VirtualAlloc(base as *const c_void, SIZE, mem::MEM_RESERVE, mem::PAGE_NOACCESS);
                    Err("synthetic post-release failure".into())
                });
                std::process::exit(if matches!(result, Err(Failure::Gone(_))) { 0 } else { 1 });
            }
        }
        if crate::inplace::tests::warp().is_none() {
            return;
        }
        let (code, transcript) = alone("warp_a_failed_restore_of_fresh_memory_is_reported", "VICTORY_TEST_RESTORE_REPORT");
        assert_eq!(code, Some(0), "{transcript}");
    }

    /// XCL's (f78e05bcc): grooming for a window under 1 GiB leaves it no free
    /// stretch but its own hole: a window-sized reservation lands there, not
    /// in a 768 MB stretch between two other reservations below it. In a
    /// process of its own: while grooming, no other thread could reserve
    /// more than the margins.
    #[test]
    fn grooming_leaves_a_window_under_1_gib_only_its_hole() {
        const MEM_TOP_DOWN: u32 = 0x0010_0000;
        const LEN: usize = 512 << 20;
        if std::env::var_os("VICTORY_TEST_GROOM_SMALL").is_some() {
            let reserve = |at: usize, n: usize, flags: u32| {
                // SAFETY: reservations of the test's own.
                unsafe { mem::VirtualAlloc(at as *const c_void, n, mem::MEM_RESERVE | flags, mem::PAGE_NOACCESS) as usize }
            };
            let free = |at: usize| {
                // SAFETY: as above.
                unsafe { mem::VirtualFree(at as *mut c_void, 0, mem::MEM_RELEASE) };
            };
            // The window's place, high up; held while the stretch below is made.
            let hole = reserve(0, LEN, MEM_TOP_DOWN);
            assert_ne!(hole, 0);
            // A free stretch of 768 MB between two reservations, below the hole.
            let probe = reserve(0, 2 << 30, 0);
            assert!(probe != 0 && probe < hole);
            free(probe);
            let (low, high) = (reserve(probe, 256 << 20, 0), reserve(probe + (1024 << 20), 256 << 20, 0));
            assert!(low == probe && high == probe + (1024 << 20));
            free(hole);
            let got = groom(hole, LEN);
            let p = reserve(0, LEN, 0);
            ungroom(got);
            for a in [p, low, high] {
                if a != 0 {
                    free(a);
                }
            }
            println!("window reservation at {p:#x}, hole {hole:#x}, stretch {:#x}..{:#x}", probe + (256 << 20), probe + (1024 << 20));
            std::process::exit(if p == hole { 0 } else { 1 });
        }
        let (code, transcript) = alone("grooming_leaves_a_window_under_1_gib_only_its_hole", "VICTORY_TEST_GROOM_SMALL");
        assert_eq!(code, Some(0), "{transcript}");
    }

    /// The whole path on the software adapter, in a process of its own:
    /// fresh memory remade at its address, its written page kept, a title
    /// buffer placed in it as `place_for` places one; then, with that heap
    /// still standing, the self-test. (With the first heap let go first, the
    /// software adapter mapped the self-test's heap where the first one had
    /// been, address space it keeps for itself and the grooming cannot
    /// reserve: the remake then kept the memory as it was.)
    #[test]
    fn warp_a_remade_allocation_and_the_self_test() {
        if std::env::var_os("VICTORY_TEST_UMA_WARP").is_some() {
            let device = crate::inplace::tests::warp().unwrap();
            const SIZE: usize = 64 << 20;
            // SAFETY: memory of the test's own.
            unsafe {
                let base = mem::VirtualAlloc(std::ptr::null(), SIZE, mem::MEM_RESERVE | mem::MEM_COMMIT | MEM_WRITE_WATCH, mem::PAGE_READWRITE) as usize;
                assert_ne!(base, 0);
                fill(base + 5 * K64, K64, 3);
                let want: Vec<u8> = std::slice::from_raw_parts((base + 5 * K64) as *const u8, K64).to_vec();
                let remade = match remake(&device, base, SIZE, Lost::Report) {
                    Ok(remade) => remade,
                    Err(e) => panic!("not remade: {e:?}"),
                };
                assert!(same(base + 5 * K64, &want), "the written page is kept");
                assert!(std::slice::from_raw_parts((base + 6 * K64) as *const u8, K64).iter().all(|&b| b == 0), "unwritten pages are zeros");
                let mut rig = crate::inplace::Rig::new(&device).unwrap();
                let got = rig.fetch(&remade.whole, 5 * K64 as u64, K64).unwrap();
                assert_eq!(got, want, "the GPU reads the kept page");
                let placed = crate::inplace::place_with(&device, &remade.heap, 5 * K64 as u64, K64 as u64, false).unwrap();
                rig.store(&placed, 16, &[0xa5; 64]).unwrap();
                assert!(same(base + 5 * K64 + 16, &[0xa5; 64]), "the GPU's write shows at the address");
                println!("remade at its address");
                match self_test(&device) {
                    Ok(lines) => lines.iter().for_each(|l| println!("self-test: {l}")),
                    Err(e) => panic!("self-test failed: {e}"),
                }
                drop(placed);
                remade.whole.Unmap(0, None);
            }
            std::process::exit(0);
        }
        if crate::inplace::tests::warp().is_none() {
            return;
        }
        let (code, transcript) = alone("warp_a_remade_allocation_and_the_self_test", "VICTORY_TEST_UMA_WARP");
        eprintln!("{transcript}");
        assert_eq!(code, Some(0), "{transcript}");
    }
}
