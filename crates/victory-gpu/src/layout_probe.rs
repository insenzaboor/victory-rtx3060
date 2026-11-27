//! `VICTORY_PROBE_LAYOUT=1`: counters for whether title graphics memory can
//! stay where the title put it (`docs/design/in-place-memory.md`, stage 0).
//! Off by default; on, a few lines every 30 seconds and a summary at the
//! end of the run, all of it prefixed `layout probe:`.
//!
//! It answers three questions before any of the later stages is built:
//!
//! - **Descriptor origin.** Of the view and sampler slots draws read
//!   through tables, how many hold a record our API wrote at that very slot
//!   ([`SlotOrigin::Api`]), a record of ours the title moved there as bytes
//!   ([`SlotOrigin::Copied`]), or bytes that are no record of ours at all
//!   ([`SlotOrigin::RawTitle`], [`SlotOrigin::RawApi`]). Descriptors that
//!   carry their host index (stage 3) work for the first two; the raw ones
//!   are what would need the positional fallback.
//! - **Allocation origin.** Which HLE call gave out the memory the GPU
//!   uses ([`Origin`]), by size class and memory type, and how the shadow
//!   copies' traffic (refresh bytes, write faults, bytes landed back) splits
//!   by the kind of region it falls in ([`Region`]); which of those regions
//!   an allocator of ours could have placed in memory committed up front
//!   ([`Region::placeable`]); and how often the title remaps pages
//!   ([`Remap`]) where the GPU reads.
//! - **Who writes.** Per region kind, the bytes the CPU wrote that a refresh
//!   copied to the card, against the bytes shaders wrote that were landed
//!   back in title memory.
//!
//! Cost: one relaxed load of a cached flag when off. When on, atomic adds
//! (descriptor uses on per-thread stripes, since five recording threads
//! count some 400,000 a frame) and, allocated at the first use, a page
//! directory of a byte per 64 KiB of title memory touched (64 KiB per 4 GiB
//! of address space) and 18 MiB of descriptor slot tables.

use std::cell::Cell;
use std::sync::OnceLock;
use std::sync::atomic::{
    AtomicBool, AtomicPtr, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering::Relaxed,
};

/// 0 not asked yet, 1 off, 2 on.
static STATE: AtomicU8 = AtomicU8::new(0);

/// Whether `VICTORY_PROBE_LAYOUT=1`.
#[inline]
pub fn on() -> bool {
    match STATE.load(Relaxed) {
        0 => init(),
        state => state == 2,
    }
}

#[cold]
fn init() -> bool {
    let on = std::env::var("VICTORY_PROBE_LAYOUT").is_ok_and(|v| v.trim() == "1");
    STATE.store(if on { 2 } else { 1 }, Relaxed);
    on
}

/// [`on`] without asking the environment: for the write-fault handler,
/// which must not allocate. Every earlier hook asked already.
#[inline]
fn on_known() -> bool {
    STATE.load(Relaxed) == 2
}

// ---------------------------------------------------------------- buckets

/// Size classes of allocations.
pub const SIZES: [&str; 5] = ["<64K", "64K-1M", "1M-16M", "16M-256M", ">=256M"];

/// The size class of an allocation of `bytes`.
pub fn size_class(bytes: u64) -> usize {
    match bytes {
        0..0x1_0000 => 0,
        0x1_0000..0x10_0000 => 1,
        0x10_0000..0x100_0000 => 2,
        0x100_0000..0x1000_0000 => 3,
        _ => 4,
    }
}

/// How the processors reach memory, as the title asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemType {
    /// Write-back for the CPU.
    Cached,
    /// The console's uncached protection bit (0x200).
    Uncached,
    /// The console's write-combined bit (0x400), or a CPU page property of
    /// write-combining.
    WriteCombined,
    /// A heap only the GPU reaches (D3D12 default, or custom with no CPU
    /// access).
    GpuOnly,
    /// A D3D12 upload heap.
    Upload,
    /// A D3D12 readback heap.
    Readback,
    /// An `XMemAlloc` memory-type code: the code itself is counted beside
    /// (the console's names for the codes are not ours to assume).
    Coded,
    Unknown,
}

const TYPES: usize = 8;
const TYPE_NAMES: [&str; TYPES] = [
    "cached",
    "uncached",
    "write-combined",
    "GPU-only",
    "upload",
    "readback",
    "XALLOC code",
    "unknown",
];

/// The console protection's cache bits (the host's `PAGE_NOCACHE` and
/// `PAGE_WRITECOMBINE` positions); write-combining wins if both are set.
pub fn mem_type_of_protect(protect: u32) -> MemType {
    if protect & 0x400 != 0 {
        MemType::WriteCombined
    } else if protect & 0x200 != 0 {
        MemType::Uncached
    } else {
        MemType::Cached
    }
}

/// The console's GPU access bits of a protection (0x800 to 0x40000), as a
/// number 0..256.
pub fn gpu_bits_of_protect(protect: u32) -> usize {
    ((protect & 0x7_f800) >> 11) as usize
}

/// A D3D12 heap type (1 default, 2 upload, 3 readback, 4 custom) and, for
/// a custom heap, its CPU page property (1 not available, 2
/// write-combine, 3 write-back).
pub fn mem_type_of_heap(kind: u32, cpu_pages: u32) -> MemType {
    match (kind, cpu_pages) {
        (1, _) => MemType::GpuOnly,
        (2, _) => MemType::Upload,
        (3, _) => MemType::Readback,
        (4, 1) => MemType::GpuOnly,
        (4, 2) => MemType::WriteCombined,
        (4, 3) => MemType::Cached,
        _ => MemType::Unknown,
    }
}

/// The memory-type field of `XMemAlloc` attributes (bits 29 to 32).
pub fn xalloc_code(attributes: u64) -> usize {
    (attributes >> 29 & 0xf) as usize
}

/// The HLE call memory came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// `XMemVirtualAlloc`, reserved and committed at once at an address of
    /// our choice.
    VirtualCommit,
    /// `XMemVirtualAlloc`, reserved only (not a placeholder).
    VirtualReserve,
    /// `XMemVirtualAlloc` of 64 KiB pages, reserved: a placeholder the
    /// title maps physical pages into.
    Placeholder,
    /// `XMemVirtualAlloc` committing inside an earlier reservation.
    CommitInReservation,
    /// `XMemAlloc` handed to the title's own allocator hook (the memory is
    /// cut from its earlier `XMemVirtualAlloc` blocks).
    XMemAllocHooked,
    /// `XMemAllocDefault` from the host heap.
    XMemAllocHeap,
    /// `XMemAllocDefault` of 32-bit memory.
    XMemAlloc32,
    /// `XMemAllocatePhysicalPages` (no address until mapped).
    PhysicalPages,
    /// `CreateHeap`: memory of ours.
    DeviceHeap,
    /// A committed resource: ours, or from the title's allocator.
    Committed,
    /// D3D12.X: a resource at an address the title mapped itself.
    AtAddress,
    /// A resource placed in a heap of ours.
    PlacedInHeap,
    /// A descriptor heap: memory of ours, or the title's (placed).
    DescriptorHeap,
}

const ORIGINS: usize = 13;
const ORIGIN_NAMES: [&str; ORIGINS] = [
    "XMemVirtualAlloc commit",
    "XMemVirtualAlloc reserve",
    "XMemVirtualAlloc 64K placeholder",
    "XMemVirtualAlloc commit in a reservation",
    "XMemAlloc to the title's hook",
    "XMemAllocDefault host heap",
    "XMemAllocDefault 32-bit",
    "XMemAllocatePhysicalPages",
    "CreateHeap (ours)",
    "committed resource",
    "D3D12.X resource at an address",
    "placed resource in a heap",
    "descriptor heap",
];

/// What kind of region a 64 KiB page of title memory lies in, as the page
/// directory knows it (by the call that made the region).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Region {
    Unknown = 0,
    /// Private memory committed when it was reserved.
    PrivateCommitted = 1,
    /// Private memory reserved first, committed later.
    PrivateReserved = 2,
    /// A placeholder: views of physical pages, which the title remaps.
    PhysicalViews = 3,
    /// `XMemAllocDefault`'s host heap.
    XMemHeap = 4,
    /// `XMemAllocDefault`'s 32-bit memory.
    XMem32 = 5,
    /// A D3D12 heap of ours.
    DeviceHeap = 6,
    /// A committed resource's memory of ours.
    Committed = 7,
    /// A descriptor heap's memory of ours.
    DescriptorHeap = 8,
}

const REGIONS: usize = 16;
/// Traffic is counted by region kind, and one more: views of physical
/// pages that were remapped after a GPU resource or shadow lay in them
/// ([`REMAPPED_VIEWS`]).
const REGION_NAMES: [&str; 10] = [
    "unknown",
    "private, committed at once",
    "private, reserved then committed",
    "views of physical pages",
    "XMemAlloc host heap",
    "XMemAlloc 32-bit",
    "D3D12 heap (ours)",
    "committed resource (ours)",
    "descriptor heap (ours)",
    "views of physical pages, remapped under the GPU",
];

/// The traffic slot of views remapped under the GPU.
const REMAPPED_VIEWS: usize = 9;

/// What later stage could keep the bytes of a traffic slot in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Share {
    /// An owned allocator (stage 2): private memory, the XMem heaps, ours.
    Owned,
    /// Views of physical pages never remapped under the GPU: a committed
    /// page-pool section opened once as a heap could hold them, as renderer
    /// port 1 does for the 32-bit heap.
    Section,
    /// Remapped views, and memory of unknown origin: copies stay.
    Neither,
}

/// See [`Share`]; `slot` a traffic slot (a region kind or
/// [`REMAPPED_VIEWS`]).
pub fn share_of(slot: usize) -> Share {
    match slot {
        REMAPPED_VIEWS => Share::Neither,
        3 => Share::Section,
        _ if Region::from_code(slot as u8).placeable() => Share::Owned,
        _ => Share::Neither,
    }
}

/// Bytes moved per traffic slot (refreshed, landed, texture bytes read),
/// summed by [`Share`]: owned, section, neither.
fn shares(v: &[u64]) -> [u64; 3] {
    let mut out = [0u64; 3];
    for r in 0..REGION_NAMES.len() {
        let t = |f: T| v[TRAFFIC + r * TF + f as usize];
        let bytes = t(T::RefreshWatched) + t(T::RefreshWhole) + t(T::Landed) + t(T::TextureRead);
        out[share_of(r) as usize] += bytes;
    }
    out
}

impl Region {
    fn from_code(code: u8) -> Region {
        match code & CLASS_MASK {
            1 => Region::PrivateCommitted,
            2 => Region::PrivateReserved,
            3 => Region::PhysicalViews,
            4 => Region::XMemHeap,
            5 => Region::XMem32,
            6 => Region::DeviceHeap,
            7 => Region::Committed,
            8 => Region::DescriptorHeap,
            _ => Region::Unknown,
        }
    }

    /// Whether an allocator of ours could have given this memory out of a
    /// region committed and opened as a D3D12 heap at start-up: private
    /// memory, the XMem heaps and our own. Not views of physical pages
    /// (their addresses show other pages when the title remaps), nor memory
    /// of unknown origin.
    pub fn placeable(self) -> bool {
        !matches!(self, Region::Unknown | Region::PhysicalViews)
    }
}

/// The region kind an allocation of `origin` makes, and whether it replaces
/// what the directory knew of the pages (a new reservation) or only fills
/// in unknown ones (a commit inside one). `ours`: the memory was ours, not
/// cut from title memory.
pub fn region_of(origin: Origin, ours: bool) -> Option<(Region, bool)> {
    match origin {
        Origin::VirtualCommit => Some((Region::PrivateCommitted, true)),
        Origin::VirtualReserve => Some((Region::PrivateReserved, true)),
        Origin::Placeholder => Some((Region::PhysicalViews, true)),
        Origin::CommitInReservation => Some((Region::PrivateReserved, false)),
        Origin::XMemAllocHeap => Some((Region::XMemHeap, true)),
        Origin::XMemAlloc32 => Some((Region::XMem32, true)),
        Origin::DeviceHeap if ours => Some((Region::DeviceHeap, true)),
        Origin::Committed if ours => Some((Region::Committed, true)),
        Origin::DescriptorHeap if ours => Some((Region::DescriptorHeap, true)),
        _ => None,
    }
}

/// Where a descriptor slot's bytes came from, when a draw reads the slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotOrigin {
    /// A record of ours that our API wrote at this slot.
    Api,
    /// A record of ours that our API did not write here: the title moved it
    /// as bytes.
    Copied,
    /// Bytes that are no record of ours, put here by our API's copy.
    RawApi,
    /// Bytes that are no record of ours, written by the title itself.
    RawTitle,
    /// All zero.
    Empty,
}

const SLOT_ORIGINS: usize = 5;
const SLOT_NAMES: [&str; SLOT_ORIGINS] = [
    "API at the slot",
    "copied by the title",
    "raw, by API copy",
    "raw, by the title",
    "empty",
];
/// Kinds of a record: table entry, constant view named whole, placed view
/// named whole, none (not a record).
const RECORD_KINDS: usize = 4;
const RECORD_NAMES: [&str; RECORD_KINDS] =
    ["table entry", "constant view", "placed view", "no record"];

/// A slot's origin from its first three words and whether our API wrote
/// exactly these words at this slot last.
pub fn classify_slot(words: [u32; 3], api_wrote: bool) -> SlotOrigin {
    match (
        words[0] == crate::descriptor::VIEW_TAG,
        words == [0; 3],
        api_wrote,
    ) {
        (true, _, true) => SlotOrigin::Api,
        (true, _, false) => SlotOrigin::Copied,
        (false, true, _) => SlotOrigin::Empty,
        (false, false, true) => SlotOrigin::RawApi,
        (false, false, false) => SlotOrigin::RawTitle,
    }
}

/// The kind of record the words are (see [`RECORD_NAMES`]): by the two top
/// bits of the generation word, as `descriptor.rs` writes them.
pub fn record_kind(words: [u32; 3]) -> usize {
    if words[0] != crate::descriptor::VIEW_TAG {
        return 3;
    }
    match words[2] >> 30 {
        0 => 0,
        1 => 1,
        2 => 2,
        _ => 3,
    }
}

/// A slot's place in the slot tables: 16-byte units (samplers are 16 bytes,
/// views 32), 64 MiB of descriptor memory before two slots share one.
const SLOT_BITS: u32 = 22;
const SLOTS: usize = 1 << SLOT_BITS;

fn slot_of(handle: u64) -> usize {
    (handle >> 4) as usize & (SLOTS - 1)
}

/// What our API wrote at `handle`, in 32 bits (never 0).
pub fn fingerprint(handle: u64, words: [u32; 3]) -> u32 {
    let mut x = handle
        ^ u64::from(words[0]).rotate_left(17)
        ^ (u64::from(words[1]) << 32 | u64::from(words[2]));
    // splitmix64's finish.
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    (x >> 32) as u32 | 1
}

/// The page-mapping calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remap {
    MapPhysicalPages,
    Scatter,
    /// The queues' page-mapping copies (`CopyPageMappings*`).
    CopyPageMappings,
}

const REMAPS: usize = 3;
const REMAP_NAMES: [&str; REMAPS] = [
    "XMemMapPhysicalPages",
    "XMemMapPhysicalPagesScatter",
    "CopyPageMappings",
];

// --------------------------------------------------------- counter layout

/// Per region kind: what the shadow copies do there.
#[derive(Clone, Copy)]
pub(crate) enum T {
    /// Bytes a refresh copied that the write watch or the guards said the
    /// CPU wrote.
    RefreshWatched,
    /// Bytes a refresh copied whole (the memory is not watched).
    RefreshWhole,
    Refreshes,
    /// Guard faults that opened a block, and stores the fault handler made
    /// itself.
    Faults,
    Stores,
    /// Bytes shaders wrote that were put back into title memory.
    Landed,
    Lands,
    /// Shadow copies made and let go, in bytes (the RAM they cost).
    ShadowMade,
    ShadowGone,
    /// Shadows that are the title's memory in place (`VICTORY_INPLACE`).
    InPlace,
    /// Bytes of textures in title memory read for a refresh (hashed or
    /// detiled and uploaded): CPU-side work on bytes the CPU may have
    /// written.
    TextureRead,
}

const TF: usize = 11;

const ALLOC: usize = 0;
const ALLOC_LEN: usize = ORIGINS * SIZES.len() * TYPES * 2;
const CODES: usize = ALLOC + ALLOC_LEN;
const CODES_LEN: usize = ORIGINS * 16 * 2;
const GPU_BITS: usize = CODES + CODES_LEN;
const GPU_BITS_LEN: usize = 256 * 2;
const TRAFFIC: usize = GPU_BITS + GPU_BITS_LEN;
const TRAFFIC_LEN: usize = REGIONS * TF;
/// Writes of slots: by our API, by record kind; then the slots our API's
/// copies wrote; then the title's writes the heap sync noticed, by origin.
const DESC_WRITES: usize = TRAFFIC + TRAFFIC_LEN;
const DESC_WRITES_LEN: usize = RECORD_KINDS + 1 + SLOT_ORIGINS;
const DESC_DISTINCT: usize = DESC_WRITES + DESC_WRITES_LEN;
/// Per call: calls, pages mapped, pages unmapped, of all those the pages a
/// GPU resource or shadow lay in.
const REMAP_COUNTS: usize = DESC_DISTINCT + SLOT_ORIGINS;
const REMAP_LEN: usize = REMAPS * 4;
/// Frees: calls, those that touch pages the GPU used, of those the ones in
/// placeable regions.
const FREES: usize = REMAP_COUNTS + REMAP_LEN;
const FREES_LEN: usize = 3;
const COUNTERS: usize = FREES + FREES_LEN;
/// Descriptor uses by origin and record kind, summed over the stripes.
const DESC_USES: usize = COUNTERS;
const USES: usize = SLOT_ORIGINS * RECORD_KINDS;
const SNAPSHOT: usize = DESC_USES + USES;

fn alloc_at(origin: Origin, size: usize, kind: MemType, bytes: bool) -> usize {
    ALLOC
        + ((origin as usize * SIZES.len() + size) * TYPES + kind as usize) * 2
        + usize::from(bytes)
}

#[cfg(test)]
fn traffic_at(region: Region, field: T) -> usize {
    TRAFFIC + region as usize * TF + field as usize
}

// ------------------------------------------------------------ the probe

const STRIPES: usize = 16;

#[repr(align(64))]
struct Stripe([AtomicU64; USES]);

const PAGE_SHIFT: u32 = 16;
const CHUNK_SHIFT: u32 = 32;
const ADDRESS_BITS: u32 = 47;
const CHUNKS: usize = 1 << (ADDRESS_BITS - CHUNK_SHIFT);
const PER_CHUNK: usize = 1 << (CHUNK_SHIFT - PAGE_SHIFT);
/// A page's byte: the region kind, and whether a GPU resource or a shadow
/// ever lay in it.
const CLASS_MASK: u8 = 0x0f;
const GPU: u8 = 0x10;
/// The page was remapped after the GPU bit was set.
const REMAPPED: u8 = 0x20;

/// The traffic slot of a page's byte.
fn slot_of_page(byte: u8) -> usize {
    match Region::from_code(byte) {
        Region::PhysicalViews if byte & REMAPPED != 0 => REMAPPED_VIEWS,
        region => region as usize,
    }
}

/// The counters and tables; one for the run ([`PROBE`]), others in tests.
pub(crate) struct Probe {
    counters: [AtomicU64; COUNTERS],
    uses: [Stripe; STRIPES],
    /// By 4 GiB of address space: a byte per 64 KiB page, made when a page
    /// in it is first noted.
    pages: OnceLock<Box<[AtomicPtr<AtomicU8>]>>,
    /// By slot: what our API last wrote there ([`fingerprint`]).
    written: OnceLock<Box<[AtomicU32]>>,
    /// By slot origin and slot: whether a draw read such a slot.
    seen: OnceLock<Box<[AtomicU64]>>,
}

pub(crate) static PROBE: Probe = Probe::new();

impl Probe {
    pub(crate) const fn new() -> Probe {
        Probe {
            counters: [const { AtomicU64::new(0) }; COUNTERS],
            uses: [const { Stripe([const { AtomicU64::new(0) }; USES]) }; STRIPES],
            pages: OnceLock::new(),
            written: OnceLock::new(),
            seen: OnceLock::new(),
        }
    }

    fn add(&self, at: usize, n: u64) {
        self.counters[at].fetch_add(n, Relaxed);
    }

    // The page directory.

    fn chunks(&self) -> &[AtomicPtr<AtomicU8>] {
        self.pages.get_or_init(|| {
            (0..CHUNKS)
                .map(|_| AtomicPtr::new(std::ptr::null_mut()))
                .collect()
        })
    }

    /// The bytes of the 4 GiB chunk `chunk`, made if `make`.
    fn chunk(&self, chunk: usize, make: bool) -> Option<&[AtomicU8]> {
        let chunks = if make {
            self.chunks()
        } else {
            self.pages.get()?
        };
        let slot = chunks.get(chunk)?;
        let mut at = slot.load(std::sync::atomic::Ordering::Acquire);
        if at.is_null() {
            if !make {
                return None;
            }
            let fresh: Box<[AtomicU8]> = (0..PER_CHUNK).map(|_| AtomicU8::new(0)).collect();
            let fresh = Box::into_raw(fresh).cast::<AtomicU8>();
            at = match slot.compare_exchange(
                std::ptr::null_mut(),
                fresh,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            ) {
                Ok(_) => fresh,
                Err(other) => {
                    // SAFETY: ours, just made, never shared.
                    drop(unsafe {
                        Box::from_raw(std::ptr::slice_from_raw_parts_mut(fresh, PER_CHUNK))
                    });
                    other
                }
            };
        }
        // SAFETY: chunks are made `PER_CHUNK` long and never freed.
        Some(unsafe { std::slice::from_raw_parts(at, PER_CHUNK) })
    }

    /// The pages of `[at, at + len)` (at least the first), each with `f`.
    fn each_page(&self, at: usize, len: usize, make: bool, mut f: impl FnMut(&AtomicU8)) {
        let first = at >> PAGE_SHIFT;
        let last = at.saturating_add(len.max(1) - 1) >> PAGE_SHIFT;
        let mut page = first;
        while page <= last && page >> (ADDRESS_BITS - PAGE_SHIFT) == 0 {
            let chunk = page >> (CHUNK_SHIFT - PAGE_SHIFT);
            let end = last.min(((chunk + 1) << (CHUNK_SHIFT - PAGE_SHIFT)) - 1);
            if let Some(bytes) = self.chunk(chunk, make) {
                for p in page..=end {
                    f(&bytes[p & (PER_CHUNK - 1)]);
                }
            }
            page = end + 1;
        }
    }

    /// The byte of the page that holds `at`; 0 if none was noted. Takes no
    /// lock and makes nothing.
    fn page(&self, at: usize) -> u8 {
        if at >> ADDRESS_BITS != 0 {
            return 0;
        }
        self.chunk(at >> CHUNK_SHIFT, false).map_or(0, |bytes| {
            bytes[(at >> PAGE_SHIFT) & (PER_CHUNK - 1)].load(Relaxed)
        })
    }

    pub(crate) fn region(&self, at: usize) -> Region {
        Region::from_code(self.page(at))
    }

    fn mark_region(&self, at: usize, len: usize, region: Region, replace: bool) {
        self.each_page(at, len, true, |b| {
            let _ = b.fetch_update(Relaxed, Relaxed, |old| {
                (replace || old & CLASS_MASK == 0).then_some(old & !CLASS_MASK | region as u8)
            });
        });
    }

    fn mark_gpu(&self, at: usize, len: usize) {
        self.each_page(at, len, true, |b| {
            if b.load(Relaxed) & GPU == 0 {
                b.fetch_or(GPU, Relaxed);
            }
        });
    }

    /// Pages of `[at, at + len)` a GPU resource or shadow ever lay in.
    fn gpu_pages(&self, at: usize, len: usize) -> u64 {
        let mut n = 0;
        self.each_page(at, len, false, |b| {
            n += u64::from(b.load(Relaxed) & GPU != 0)
        });
        n
    }

    // Allocations.

    pub(crate) fn allocated(&self, origin: Origin, at: usize, len: u64, kind: MemType, ours: bool) {
        let size = size_class(len);
        self.add(alloc_at(origin, size, kind, false), 1);
        self.add(alloc_at(origin, size, kind, true), len);
        if at != 0
            && let Some((region, replace)) = region_of(origin, ours)
        {
            self.mark_region(at, len as usize, region, replace);
        }
    }

    fn coded(&self, origin: Origin, code: usize, len: u64) {
        self.add(CODES + (origin as usize * 16 + (code & 15)) * 2, 1);
        self.add(CODES + (origin as usize * 16 + (code & 15)) * 2 + 1, len);
    }

    fn gpu_bits(&self, bits: usize, len: u64) {
        self.add(GPU_BITS + (bits & 255) * 2, 1);
        self.add(GPU_BITS + (bits & 255) * 2 + 1, len);
    }

    // Traffic.

    pub(crate) fn traffic(&self, at: usize, field: T, n: u64) {
        self.add(
            TRAFFIC + slot_of_page(self.page(at)) * TF + field as usize,
            n,
        );
    }

    pub(crate) fn remapped(&self, call: Remap, at: usize, len: usize, unmap: bool) {
        let base = REMAP_COUNTS + call as usize * 4;
        let pages = len.div_ceil(1 << PAGE_SHIFT) as u64;
        self.add(base + if unmap { 2 } else { 1 }, pages);
        let mut gpu = 0;
        self.each_page(at, len, false, |b| {
            if b.load(Relaxed) & GPU != 0 {
                gpu += 1;
                b.fetch_or(REMAPPED, Relaxed);
            }
        });
        self.add(base + 3, gpu);
        // Whatever made the region, its pages are views of physical pages
        // now (the title's 32-bit heap is reserved without the 64 KiB page
        // bits and mapped all the same).
        self.mark_region(at, len, Region::PhysicalViews, true);
    }

    pub(crate) fn freed(&self, at: usize, len: usize) {
        self.add(FREES, 1);
        if self.gpu_pages(at, len) > 0 {
            self.add(FREES + 1, 1);
            if self.region(at).placeable() {
                self.add(FREES + 2, 1);
            }
        }
    }

    // Descriptors.

    fn written_table(&self) -> &[AtomicU32] {
        self.written
            .get_or_init(|| (0..SLOTS).map(|_| AtomicU32::new(0)).collect())
    }

    fn seen_table(&self) -> &[AtomicU64] {
        self.seen.get_or_init(|| {
            (0..SLOT_ORIGINS * SLOTS / 64)
                .map(|_| AtomicU64::new(0))
                .collect()
        })
    }

    /// Our API wrote `words` at `handle`; `copy`: by a descriptor copy.
    pub(crate) fn slot_written(&self, handle: u64, words: [u32; 3], copy: bool) {
        self.written_table()[slot_of(handle)].store(fingerprint(handle, words), Relaxed);
        self.add(
            DESC_WRITES
                + if copy {
                    RECORD_KINDS
                } else {
                    record_kind(words)
                },
            1,
        );
    }

    fn origin(&self, handle: u64, words: [u32; 3]) -> SlotOrigin {
        let api = self.written_table()[slot_of(handle)].load(Relaxed) == fingerprint(handle, words);
        classify_slot(words, api)
    }

    /// The heap sync found `words` at `handle`, changed since it last
    /// looked: a write of the title's, or one of our API's it sees too.
    pub(crate) fn slot_changed(&self, handle: u64, words: [u32; 3]) {
        let origin = self.origin(handle, words);
        self.add(DESC_WRITES + RECORD_KINDS + 1 + origin as usize, 1);
    }

    /// A draw reads the slot at `handle`, which holds `words`.
    pub(crate) fn slot_used(&self, handle: u64, words: [u32; 3]) -> SlotOrigin {
        let origin = self.origin(handle, words);
        let stripe = &self.uses[stripe()];
        stripe.0[origin as usize * RECORD_KINDS + record_kind(words)].fetch_add(1, Relaxed);
        let bit = origin as usize * SLOTS + slot_of(handle);
        let word = &self.seen_table()[bit / 64];
        let mask = 1u64 << (bit % 64);
        if word.load(Relaxed) & mask == 0 && word.fetch_or(mask, Relaxed) & mask == 0 {
            self.add(DESC_DISTINCT + origin as usize, 1);
        }
        origin
    }

    /// Every counter, the descriptor uses summed over the stripes.
    pub(crate) fn snapshot(&self) -> Vec<u64> {
        let mut out: Vec<u64> = self.counters.iter().map(|c| c.load(Relaxed)).collect();
        out.resize(SNAPSHOT, 0);
        for stripe in &self.uses {
            for (k, c) in stripe.0.iter().enumerate() {
                out[DESC_USES + k] += c.load(Relaxed);
            }
        }
        out
    }
}

/// This thread's stripe of the descriptor-use counters.
fn stripe() -> usize {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    thread_local!(static STRIPE: usize = NEXT.fetch_add(1, Relaxed) % STRIPES);
    STRIPE.with(|s| *s)
}

// ---------------------------------------------------------- the hooks

/// The runtime gave the title `[at, at + len)` (`origin` one of the
/// `XMemVirtualAlloc` kinds), with the console protection `protect`.
pub fn title_allocated(origin: Origin, at: usize, len: usize, protect: u32) {
    if !on() {
        return;
    }
    PROBE.allocated(origin, at, len as u64, mem_type_of_protect(protect), false);
    PROBE.gpu_bits(gpu_bits_of_protect(protect), len as u64);
}

/// The runtime gave the title `len` bytes at `at` (0: none yet) with
/// `XMemAlloc` attributes `attributes`.
pub fn title_allocated_coded(origin: Origin, at: usize, len: usize, attributes: u64) {
    if !on() {
        return;
    }
    PROBE.allocated(origin, at, len as u64, MemType::Coded, false);
    PROBE.coded(origin, xalloc_code(attributes), len as u64);
}

/// The device made a heap or resource over `[at, at + len)` of D3D12 heap
/// type `kind` (CPU page property `cpu_pages` for custom heaps); `ours` if
/// the memory is ours, not the title's.
pub(crate) fn gpu_allocated(
    origin: Origin,
    at: usize,
    len: u64,
    kind: u32,
    cpu_pages: u32,
    ours: bool,
) {
    if !on() {
        return;
    }
    PROBE.allocated(origin, at, len, mem_type_of_heap(kind, cpu_pages), ours);
    if at != 0 && origin != Origin::DescriptorHeap {
        PROBE.mark_gpu(at, len as usize);
    }
}

/// A shadow copy of `[at, at + len)` was made.
pub(crate) fn shadow_made(at: u64, len: u64) {
    if !on() {
        return;
    }
    PROBE.mark_gpu(at as usize, len as usize);
    PROBE.traffic(at as usize, T::ShadowMade, len);
}

/// The shadow [`shadow_made`] just told of is the title's memory in place,
/// no copy.
pub(crate) fn shadow_in_place(at: u64, len: u64) {
    if !on() {
        return;
    }
    let slot = slot_of_page(PROBE.page(at as usize));
    PROBE.counters[TRAFFIC + slot * TF + T::ShadowMade as usize].fetch_sub(len, Relaxed);
    PROBE.traffic(at as usize, T::InPlace, len);
}

/// A texture's refresh read `bytes` of title memory at `at`.
pub(crate) fn texture_read(at: usize, bytes: usize) {
    if on() && bytes > 0 {
        PROBE.traffic(at, T::TextureRead, bytes as u64);
    }
}

/// The shadow copy [`shadow_made`] told of is let go.
pub(crate) fn shadow_gone(at: u64, len: u64) {
    if on() {
        PROBE.traffic(at as usize, T::ShadowGone, len);
    }
}

/// A refresh of `[at, ..)` copied `bytes`; `watched`: only what the watch or
/// the guards said the CPU wrote.
pub(crate) fn refreshed(at: usize, bytes: usize, watched: bool) {
    if !on() || bytes == 0 {
        return;
    }
    PROBE.traffic(
        at,
        if watched {
            T::RefreshWatched
        } else {
            T::RefreshWhole
        },
        bytes as u64,
    );
    PROBE.traffic(at, T::Refreshes, 1);
}

/// A guard fault at `at` opened its block (`store` false), or the handler
/// made the store itself. In the fault handler: nothing is allocated.
pub(crate) fn write_fault(at: usize, store: bool) {
    if on_known() {
        PROBE.traffic(at, if store { T::Stores } else { T::Faults }, 1);
    }
}

/// `bytes` that shaders wrote were put into title memory from `at` on.
pub(crate) fn landed(at: usize, bytes: u64) {
    if !on() || bytes == 0 {
        return;
    }
    PROBE.traffic(at, T::Landed, bytes);
    PROBE.traffic(at, T::Lands, 1);
}

/// The title frees or decommits `[at, at + len)` (`len` 0: the whole
/// allocation; its first page is looked at).
pub fn title_freed(at: usize, len: usize) {
    if on() {
        PROBE.freed(at, len);
    }
}

thread_local!(static REMAPPING: Cell<bool> = const { Cell::new(false) });

/// A page-mapping call of the title's, while it lasts: only the outermost
/// call on a thread is counted (scatter maps through
/// `XMemMapPhysicalPages`, the many-range copy through the one-range one).
pub struct RemapScope {
    outer: bool,
}

/// See [`RemapScope`]; counts the call.
pub fn remap_scope(call: Remap) -> RemapScope {
    if !on() {
        return RemapScope { outer: false };
    }
    let outer = !REMAPPING.with(|r| r.replace(true));
    if outer {
        PROBE.add(REMAP_COUNTS + call as usize * 4, 1);
    }
    RemapScope { outer }
}

impl RemapScope {
    /// Whether this scope counts (the probe is on and the call outermost).
    pub fn counting(&self) -> bool {
        self.outer
    }

    /// `[at, at + len)` is mapped anew (`unmap`: unmapped).
    pub fn note(&self, call: Remap, at: usize, len: usize, unmap: bool) {
        if self.outer {
            PROBE.remapped(call, at, len, unmap);
        }
    }
}

impl Drop for RemapScope {
    fn drop(&mut self) {
        if self.outer {
            REMAPPING.with(|r| r.set(false));
        }
    }
}

/// Our API wrote `words` at the slot `handle` (view creation; `copy`: a
/// descriptor copy).
pub(crate) fn slot_written(handle: u64, words: [u32; 3], copy: bool) {
    if on() && handle != 0 {
        PROBE.slot_written(handle, words, copy);
    }
}

/// The heap sync found the slot at `handle` changed, now `words`.
pub(crate) fn slot_changed(handle: u64, words: [u32; 3]) {
    if on() {
        PROBE.slot_changed(handle, words);
    }
}

/// A draw reads the slot at `handle` through a table.
pub(crate) fn slot_used(handle: u64, words: [u32; 3]) {
    if on() {
        PROBE.slot_used(handle, words);
    }
}

// ------------------------------------------------------------ the lines

/// Called once a second by the submitting thread: the lines every 30
/// seconds.
pub(crate) fn tick() {
    if !on() {
        return;
    }
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64;
    let last = LAST.load(Relaxed);
    if now < last + 30_000 || LAST.compare_exchange(last, now, Relaxed, Relaxed).is_err() {
        return;
    }
    say(false, (now - last) as f64 / 1000.0);
}

/// The summary at the end of the run (once).
pub fn summary() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if on() && !DONE.swap(true, Relaxed) {
        say(true, 0.0);
    }
}

static PREVIOUS: std::sync::Mutex<(Vec<u64>, u64)> = std::sync::Mutex::new((Vec::new(), 0));

fn say(last: bool, seconds: f64) {
    let now = PROBE.snapshot();
    let frames = crate::frame::current();
    let mut previous = PREVIOUS.lock().unwrap_or_else(|e| e.into_inner());
    let (before, before_frames) = (&previous.0, previous.1);
    let window: Vec<u64> = if before.is_empty() {
        now.clone()
    } else {
        now.iter()
            .zip(before)
            .map(|(a, b)| a.saturating_sub(*b))
            .collect()
    };
    let lines = if last {
        lines(&now, frames, true)
    } else {
        lines_window(
            &now,
            &window,
            frames,
            frames.saturating_sub(before_frames),
            seconds,
        )
    };
    *previous = (now, frames);
    drop(previous);
    for line in lines {
        tracing::info!("layout probe: {line}");
    }
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / (1 << 20) as f64)
}

fn percent(part: u64, whole: u64) -> String {
    if whole == 0 {
        "-".to_owned()
    } else {
        format!("{:.2} %", part as f64 * 100.0 / whole as f64)
    }
}

/// The per-window lines: descriptors per frame, traffic and remaps of the
/// window; then the totals.
fn lines_window(
    total: &[u64],
    window: &[u64],
    frames: u64,
    window_frames: u64,
    seconds: f64,
) -> Vec<String> {
    let mut out = vec![format!("last {seconds:.0} s, {window_frames} frames:")];
    out.extend(descriptor_lines(window, window_frames.max(1), false));
    out.extend(traffic_lines(window, window_frames.max(1)));
    out.extend(remap_lines(window, seconds.max(1.0)));
    out.push(format!("since the start, {frames} frames:"));
    out.extend(lines(total, frames, false));
    out
}

/// The totals; `all`: every allocation bucket, else the biggest.
fn lines(v: &[u64], frames: u64, all: bool) -> Vec<String> {
    let mut out = Vec::new();
    if all {
        out.push(format!("SUMMARY of the run, {frames} frames"));
    }
    out.extend(descriptor_lines(v, frames.max(1), true));
    out.extend(allocation_lines(v, all));
    out.extend(traffic_lines(v, frames.max(1)));
    out.extend(remap_lines(v, 0.0));
    out.extend(verdict_lines(v));
    out
}

pub(crate) fn descriptor_lines(v: &[u64], frames: u64, totals: bool) -> Vec<String> {
    let use_of = |o: usize, k: usize| v[DESC_USES + o * RECORD_KINDS + k];
    let by_origin: Vec<u64> = (0..SLOT_ORIGINS)
        .map(|o| (0..RECORD_KINDS).map(|k| use_of(o, k)).sum())
        .collect();
    let all: u64 = by_origin.iter().sum();
    let mut out = vec![format!(
        "descriptors read by draws: {all} ({} a frame): {}",
        all / frames,
        (0..SLOT_ORIGINS)
            .map(|o| format!(
                "{} {} ({}, {} a frame)",
                SLOT_NAMES[o],
                by_origin[o],
                percent(by_origin[o], all),
                by_origin[o] / frames
            ))
            .collect::<Vec<_>>()
            .join(", ")
    )];
    let copied: Vec<String> = (0..RECORD_KINDS)
        .filter(|&k| use_of(1, k) > 0)
        .map(|k| format!("{} {}", RECORD_NAMES[k], use_of(1, k)))
        .collect();
    if !copied.is_empty() {
        out.push(format!(
            "  of those copied by the title, by record kind: {}",
            copied.join(", ")
        ));
    }
    if totals {
        out.push(format!(
            "  distinct slots read since the start: {}",
            (0..SLOT_ORIGINS)
                .map(|o| format!("{} {}", SLOT_NAMES[o], v[DESC_DISTINCT + o]))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let writes: Vec<String> = (0..RECORD_KINDS - 1)
        .map(|k| format!("{} {}", RECORD_NAMES[k], v[DESC_WRITES + k]))
        .collect();
    out.push(format!(
        "  slots written by our API: {}, by its copies {}; changes the heap sync found: {}",
        writes.join(", "),
        v[DESC_WRITES + RECORD_KINDS],
        (0..SLOT_ORIGINS)
            .map(|o| format!(
                "{} {}",
                SLOT_NAMES[o],
                v[DESC_WRITES + RECORD_KINDS + 1 + o]
            ))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out
}

fn allocation_lines(v: &[u64], all: bool) -> Vec<String> {
    let mut out = Vec::new();
    for o in 0..ORIGINS {
        let mut cells: Vec<(u64, u64, usize, usize)> = Vec::new();
        for s in 0..SIZES.len() {
            for t in 0..TYPES {
                let at = ALLOC + ((o * SIZES.len() + s) * TYPES + t) * 2;
                if v[at] > 0 {
                    cells.push((v[at + 1], v[at], s, t));
                }
            }
        }
        if cells.is_empty() {
            continue;
        }
        let (n, bytes) = cells.iter().fold((0, 0), |a, c| (a.0 + c.1, a.1 + c.0));
        cells.sort_unstable_by(|a, b| b.cmp(a));
        let shown = if all { cells.len() } else { cells.len().min(4) };
        out.push(format!(
            "allocations, {}: {n} of {}: {}",
            ORIGIN_NAMES[o],
            mb(bytes),
            cells[..shown]
                .iter()
                .map(|c| format!("{} {} {}x {}", SIZES[c.2], TYPE_NAMES[c.3], c.1, mb(c.0)))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        let codes: Vec<String> = (0..16)
            .filter(|&c| v[CODES + (o * 16 + c) * 2] > 0)
            .map(|c| {
                format!(
                    "code {c}: {}x {}",
                    v[CODES + (o * 16 + c) * 2],
                    mb(v[CODES + (o * 16 + c) * 2 + 1])
                )
            })
            .collect();
        if !codes.is_empty() {
            out.push(format!("  XALLOC memory-type codes: {}", codes.join(", ")));
        }
    }
    let bits: Vec<String> = (0..256)
        .filter(|&b| v[GPU_BITS + b * 2] > 0)
        .map(|b| {
            format!(
                "{:#x}: {}x {}",
                b << 11,
                v[GPU_BITS + b * 2],
                mb(v[GPU_BITS + b * 2 + 1])
            )
        })
        .collect();
    if !bits.is_empty() {
        out.push(format!(
            "  XMemVirtualAlloc protections' GPU bits: {}",
            bits.join(", ")
        ));
    }
    out
}

pub(crate) fn traffic_lines(v: &[u64], frames: u64) -> Vec<String> {
    let t = |r: usize, f: T| v[TRAFFIC + r * TF + f as usize];
    let mut out = Vec::new();
    for r in 0..REGION_NAMES.len() {
        let refreshed = t(r, T::RefreshWatched) + t(r, T::RefreshWhole);
        let any = (0..TF).any(|f| v[TRAFFIC + r * TF + f] > 0);
        if !any {
            continue;
        }
        out.push(format!(
            "traffic, {}: CPU-written bytes refreshed {} ({} watched, {} whole; {} a frame) in {} refreshes; write faults {} and stores {}; GPU-written bytes landed {} ({} a frame) in {} pieces; texture bytes read {} ({} a frame); shadows made {}, let go {}, in place {}",
            REGION_NAMES[r],
            mb(refreshed),
            mb(t(r, T::RefreshWatched)),
            mb(t(r, T::RefreshWhole)),
            mb(refreshed / frames),
            t(r, T::Refreshes),
            t(r, T::Faults),
            t(r, T::Stores),
            mb(t(r, T::Landed)),
            mb(t(r, T::Landed) / frames),
            t(r, T::Lands),
            mb(t(r, T::TextureRead)),
            mb(t(r, T::TextureRead) / frames),
            mb(t(r, T::ShadowMade)),
            mb(t(r, T::ShadowGone)),
            mb(t(r, T::InPlace))
        ));
    }
    let [owned, section, neither] = shares(v);
    let moved = owned + section + neither;
    if moved > 0 {
        out.push(format!(
            "traffic in regions an owned allocator could place up front: {} of {} ({}); in views never remapped under the GPU (a committed pool section): {} ({}); neither: {} ({})",
            mb(owned),
            mb(moved),
            percent(owned, moved),
            mb(section),
            percent(section, moved),
            mb(neither),
            percent(neither, moved)
        ));
    }
    out
}

fn remap_lines(v: &[u64], seconds: f64) -> Vec<String> {
    let mut out = Vec::new();
    for c in 0..REMAPS {
        let at = REMAP_COUNTS + c * 4;
        if v[at] == 0 {
            continue;
        }
        let rate = if seconds > 0.0 {
            format!(" ({:.0} GPU pages a second)", v[at + 3] as f64 / seconds)
        } else {
            String::new()
        };
        out.push(format!(
            "remaps, {}: {} calls, {} pages mapped, {} unmapped, {} of them in pages a GPU resource or shadow used{rate}",
            REMAP_NAMES[c],
            v[at],
            v[at + 1],
            v[at + 2],
            v[at + 3]
        ));
    }
    if v[FREES] > 0 {
        out.push(format!(
            "frees by the title: {}, {} touching pages a GPU resource or shadow used, {} of those in placeable regions",
            v[FREES],
            v[FREES + 1],
            v[FREES + 2]
        ));
    }
    out
}

/// What the numbers say for items 2 and 3, by the design's thresholds.
pub(crate) fn verdict_lines(v: &[u64]) -> Vec<String> {
    let uses: Vec<u64> = (0..SLOT_ORIGINS)
        .map(|o| {
            (0..RECORD_KINDS)
                .map(|k| v[DESC_USES + o * RECORD_KINDS + k])
                .sum()
        })
        .collect();
    let all: u64 = uses.iter().sum();
    let raw = uses[SlotOrigin::RawTitle as usize] + uses[SlotOrigin::RawApi as usize];
    let item2 = if all == 0 {
        "no draws read descriptors yet".to_owned()
    } else if raw * 100 <= all {
        format!(
            "raw descriptors are {} of reads: descriptors carrying their host index look viable",
            percent(raw, all)
        )
    } else {
        format!(
            "raw descriptors are {} of reads: the positional path stays for them",
            percent(raw, all)
        )
    };
    let [owned, section, neither] = shares(v);
    let moved = owned + section + neither;
    let remapped_gpu: u64 = (0..REMAPS).map(|c| v[REMAP_COUNTS + c * 4 + 3]).sum();
    let item3 = if moved == 0 {
        "no shadow traffic yet".to_owned()
    } else {
        format!(
            "{} of shadow traffic an owned allocator could keep in place, {} more a committed pool section; {} stays copied; {remapped_gpu} page remaps hit pages the GPU used",
            percent(owned, moved),
            percent(section, moved),
            percent(neither, moved)
        )
    };
    vec![
        format!("verdict, item 2 (descriptors with a host index): {item2}"),
        format!("verdict, item 3 (owned allocator): {item3}"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::VIEW_TAG;

    #[test]
    fn sizes_fall_in_their_classes() {
        assert_eq!(size_class(0), 0);
        assert_eq!(size_class(0xffff), 0);
        assert_eq!(size_class(0x1_0000), 1);
        assert_eq!(size_class(0xf_ffff), 1);
        assert_eq!(size_class(1 << 20), 2);
        assert_eq!(size_class((16 << 20) - 1), 2);
        assert_eq!(size_class(16 << 20), 3);
        assert_eq!(size_class(256 << 20), 4);
        assert_eq!(size_class(u64::MAX), 4);
    }

    #[test]
    fn memory_types_come_from_the_title_s_bits() {
        assert_eq!(mem_type_of_protect(0x04), MemType::Cached);
        assert_eq!(mem_type_of_protect(0x204), MemType::Uncached);
        assert_eq!(mem_type_of_protect(0x404), MemType::WriteCombined);
        assert_eq!(
            mem_type_of_protect(0x604),
            MemType::WriteCombined,
            "write-combining wins"
        );
        assert_eq!(gpu_bits_of_protect(0x04), 0);
        assert_eq!(gpu_bits_of_protect(0x804), 1);
        assert_eq!(gpu_bits_of_protect(0x4_0004), 0x80);
        assert_eq!(gpu_bits_of_protect(0x8_0004), 0, "past the GPU bits");
        assert_eq!(mem_type_of_heap(1, 0), MemType::GpuOnly);
        assert_eq!(mem_type_of_heap(2, 0), MemType::Upload);
        assert_eq!(mem_type_of_heap(3, 0), MemType::Readback);
        assert_eq!(mem_type_of_heap(4, 1), MemType::GpuOnly);
        assert_eq!(mem_type_of_heap(4, 2), MemType::WriteCombined);
        assert_eq!(mem_type_of_heap(4, 3), MemType::Cached);
        assert_eq!(mem_type_of_heap(4, 0), MemType::Unknown);
        assert_eq!(xalloc_code(3 << 29 | 7 << 24), 3);
        assert_eq!(xalloc_code(1 << 33), 0, "the 32-bit flag is not the type");
    }

    #[test]
    fn slots_are_told_apart_by_their_words_and_who_wrote_them() {
        let record = [VIEW_TAG, 5, 1];
        assert_eq!(classify_slot(record, true), SlotOrigin::Api);
        assert_eq!(classify_slot(record, false), SlotOrigin::Copied);
        assert_eq!(classify_slot([0; 3], false), SlotOrigin::Empty);
        assert_eq!(classify_slot([0; 3], true), SlotOrigin::Empty);
        assert_eq!(classify_slot([0x1234, 0, 0], false), SlotOrigin::RawTitle);
        assert_eq!(classify_slot([0x1234, 0, 0], true), SlotOrigin::RawApi);
        assert_eq!(record_kind([VIEW_TAG, 5, 1]), 0);
        assert_eq!(record_kind([VIEW_TAG, 5, 1 << 30 | 9]), 1);
        assert_eq!(record_kind([VIEW_TAG, 5, 2 << 30]), 2);
        assert_eq!(record_kind([VIEW_TAG, 5, 3 << 30]), 3);
        assert_eq!(record_kind([7, 5, 1]), 3);
        // The fingerprint never reads as "nothing written" and tells
        // neighbouring slots and generations apart.
        assert_ne!(fingerprint(0, [0; 3]), 0);
        assert_ne!(fingerprint(0x1000, record), fingerprint(0x1020, record));
        assert_ne!(
            fingerprint(0x1000, record),
            fingerprint(0x1000, [VIEW_TAG, 5, 2])
        );
    }

    #[test]
    fn a_record_moved_by_the_title_counts_as_copied() {
        let p = Box::new(Probe::new());
        let (a, b) = (0x7_0000_1000u64, 0x7_0000_1020u64);
        let record = [VIEW_TAG, 9, 1];
        p.slot_written(a, record, false);
        // The title copies a's 32 bytes to b by itself.
        assert_eq!(p.slot_used(a, record), SlotOrigin::Api);
        assert_eq!(p.slot_used(b, record), SlotOrigin::Copied);
        assert_eq!(p.slot_used(b, record), SlotOrigin::Copied);
        // Our API's copy to b makes it ours.
        p.slot_written(b, record, true);
        assert_eq!(p.slot_used(b, record), SlotOrigin::Api);
        // The title writes a raw descriptor over a, and clears b.
        assert_eq!(p.slot_used(a, [0xdead, 0xbeef, 0]), SlotOrigin::RawTitle);
        assert_eq!(p.slot_used(b, [0; 3]), SlotOrigin::Empty);
        p.slot_changed(a, [0xdead, 0xbeef, 0]);
        let v = p.snapshot();
        let uses = |o: SlotOrigin| {
            (0..RECORD_KINDS)
                .map(|k| v[DESC_USES + o as usize * RECORD_KINDS + k])
                .sum::<u64>()
        };
        assert_eq!(uses(SlotOrigin::Api), 2);
        assert_eq!(uses(SlotOrigin::Copied), 2);
        assert_eq!(uses(SlotOrigin::RawTitle), 1);
        assert_eq!(uses(SlotOrigin::Empty), 1);
        assert_eq!(v[DESC_DISTINCT + SlotOrigin::Api as usize], 2, "a and b");
        assert_eq!(
            v[DESC_DISTINCT + SlotOrigin::Copied as usize],
            1,
            "b, read twice"
        );
        assert_eq!(v[DESC_WRITES], 1, "one table-entry record written");
        assert_eq!(v[DESC_WRITES + RECORD_KINDS], 1, "one copy");
        assert_eq!(
            v[DESC_WRITES + RECORD_KINDS + 1 + SlotOrigin::RawTitle as usize],
            1
        );
        let text = descriptor_lines(&v, 1, true).join("\n");
        assert!(text.contains("descriptors read by draws: 6"), "{text}");
        assert!(text.contains("raw, by the title 1"), "{text}");
        let verdict = verdict_lines(&v).join("\n");
        assert!(verdict.contains("the positional path stays"), "{verdict}");
    }

    #[test]
    fn uses_on_many_threads_add_up() {
        let p = std::sync::Arc::new(Probe::new());
        let threads: Vec<_> = (0..8u64)
            .map(|t| {
                let p = p.clone();
                std::thread::spawn(move || {
                    for i in 0..1000u64 {
                        p.slot_used(0x10_0000 + (t * 1000 + i) * 32, [VIEW_TAG, 1, 1]);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let v = p.snapshot();
        assert_eq!(
            v[DESC_USES + SlotOrigin::Copied as usize * RECORD_KINDS],
            8000
        );
        assert_eq!(v[DESC_DISTINCT + SlotOrigin::Copied as usize], 8000);
    }

    #[test]
    fn regions_are_known_by_page_and_new_reservations_replace_them() {
        let p = Probe::new();
        let base = 0x2_0000_0000usize;
        assert_eq!(p.region(base), Region::Unknown, "nothing made yet");
        p.allocated(
            Origin::VirtualReserve,
            base,
            8 << 16,
            MemType::Cached,
            false,
        );
        assert_eq!(p.region(base), Region::PrivateReserved);
        assert_eq!(p.region(base + (8 << 16) - 1), Region::PrivateReserved);
        assert_eq!(p.region(base + (8 << 16)), Region::Unknown);
        // A commit inside keeps what the reservation said.
        p.allocated(
            Origin::CommitInReservation,
            base,
            2 << 16,
            MemType::Cached,
            false,
        );
        assert_eq!(p.region(base), Region::PrivateReserved);
        // ... and fills in what nothing said.
        p.allocated(
            Origin::CommitInReservation,
            base + (8 << 16),
            1 << 16,
            MemType::Cached,
            false,
        );
        assert_eq!(p.region(base + (8 << 16)), Region::PrivateReserved);
        // A new reservation at the same address replaces it.
        p.allocated(Origin::Placeholder, base, 4 << 16, MemType::Cached, false);
        assert_eq!(p.region(base), Region::PhysicalViews);
        assert_eq!(p.region(base + (5 << 16)), Region::PrivateReserved);
        // Across a 4 GiB line, and at the top of the address space.
        let line = 0x3_0000_0000usize;
        p.allocated(
            Origin::VirtualCommit,
            line - (1 << 16),
            2 << 16,
            MemType::Cached,
            false,
        );
        assert_eq!(p.region(line - 1), Region::PrivateCommitted);
        assert_eq!(p.region(line), Region::PrivateCommitted);
        p.allocated(
            Origin::XMemAllocHeap,
            (1 << 47) - 16,
            64,
            MemType::Coded,
            false,
        );
        assert_eq!(p.region((1 << 47) - 1), Region::XMemHeap);
        assert_eq!(
            p.region(1 << 47),
            Region::Unknown,
            "past what the directory holds"
        );
        // Memory that is not ours names no region.
        p.allocated(
            Origin::Committed,
            0x4_0000_0000,
            1 << 16,
            MemType::GpuOnly,
            false,
        );
        assert_eq!(p.region(0x4_0000_0000), Region::Unknown);
        p.allocated(
            Origin::Committed,
            0x4_0000_0000,
            1 << 16,
            MemType::GpuOnly,
            true,
        );
        assert_eq!(p.region(0x4_0000_0000), Region::Committed);
        let v = p.snapshot();
        assert_eq!(
            v[alloc_at(Origin::VirtualReserve, 1, MemType::Cached, false)],
            1
        );
        assert_eq!(
            v[alloc_at(Origin::VirtualReserve, 1, MemType::Cached, true)],
            8 << 16
        );
        // 128 KiB and 64 KiB: both of the second class.
        assert_eq!(
            v[alloc_at(Origin::CommitInReservation, 1, MemType::Cached, false)],
            2
        );
        assert_eq!(
            v[alloc_at(Origin::CommitInReservation, 1, MemType::Cached, true)],
            3 << 16
        );
        assert_eq!(
            v[alloc_at(Origin::CommitInReservation, 0, MemType::Cached, false)],
            0
        );
    }

    #[test]
    fn traffic_goes_to_the_region_it_falls_in() {
        let p = Probe::new();
        let private = 0x5_0000_0000usize;
        let views = 0x6_0000_0000usize;
        p.allocated(
            Origin::VirtualCommit,
            private,
            1 << 20,
            MemType::Cached,
            false,
        );
        p.allocated(Origin::Placeholder, views, 1 << 20, MemType::Cached, false);
        p.traffic(private + 100, T::RefreshWatched, 4096);
        p.traffic(private, T::Refreshes, 1);
        p.traffic(views, T::RefreshWhole, 65536);
        p.traffic(views, T::Landed, 1000);
        p.traffic(0x9_0000_0000, T::Faults, 1);
        // A view the GPU used and the title then remapped: its traffic is
        // in the remapped slot from then on.
        let remapped = views + (8 << 16);
        p.mark_gpu(remapped, 1 << 16);
        p.remapped(Remap::Scatter, remapped, 1 << 16, false);
        p.traffic(remapped, T::Landed, 500);
        let v = p.snapshot();
        assert_eq!(v[TRAFFIC + REMAPPED_VIEWS * TF + T::Landed as usize], 500);
        assert_eq!(
            v[traffic_at(Region::PrivateCommitted, T::RefreshWatched)],
            4096
        );
        assert_eq!(v[traffic_at(Region::PhysicalViews, T::RefreshWhole)], 65536);
        assert_eq!(v[traffic_at(Region::PhysicalViews, T::Landed)], 1000);
        assert_eq!(v[traffic_at(Region::Unknown, T::Faults)], 1);
        let text = traffic_lines(&v, 1).join("\n");
        assert!(
            text.contains("traffic in regions an owned allocator could place up front"),
            "{text}"
        );
        // 4096 of 4096 + 65536 + 1000 + 500 owned; 66536 a section; 500 neither.
        assert!(text.contains("(5.76 %)"), "{text}");
        assert!(text.contains("93.54 %"), "{text}");
        assert!(text.contains("neither: 0.0 MB (0.70 %)"), "{text}");
        assert_eq!(share_of(Region::PrivateReserved as usize), Share::Owned);
        assert_eq!(share_of(Region::PhysicalViews as usize), Share::Section);
        assert_eq!(share_of(REMAPPED_VIEWS), Share::Neither);
        assert_eq!(share_of(Region::Unknown as usize), Share::Neither);
        assert!(Region::PrivateCommitted.placeable() && Region::DeviceHeap.placeable());
        assert!(!Region::PhysicalViews.placeable() && !Region::Unknown.placeable());
    }

    #[test]
    fn remaps_and_frees_say_whether_the_gpu_used_the_pages() {
        let p = Probe::new();
        let views = 0x7_0000_0000usize;
        p.allocated(Origin::Placeholder, views, 16 << 16, MemType::Cached, false);
        // A texture over the first four pages.
        p.mark_gpu(views, 4 << 16);
        p.remapped(Remap::Scatter, views + (2 << 16), 4 << 16, false);
        p.remapped(Remap::MapPhysicalPages, views + (8 << 16), 2 << 16, true);
        let v = p.snapshot();
        let at = REMAP_COUNTS + Remap::Scatter as usize * 4;
        assert_eq!(v[at + 1], 4, "pages mapped");
        assert_eq!(v[at + 3], 2, "of which the GPU used two");
        let at = REMAP_COUNTS + Remap::MapPhysicalPages as usize * 4;
        assert_eq!((v[at + 1], v[at + 2], v[at + 3]), (0, 2, 0));
        p.freed(views + (9 << 16), 1 << 16);
        p.freed(views, 0);
        let private = 0x8_0000_0000usize;
        p.allocated(
            Origin::VirtualCommit,
            private,
            1 << 16,
            MemType::Cached,
            false,
        );
        p.mark_gpu(private, 100);
        p.freed(private, 0);
        let v = p.snapshot();
        assert_eq!(&v[FREES..FREES + 3], &[3, 2, 1]);
    }

    #[test]
    fn scopes_count_only_the_outer_call() {
        // Without the switch nothing is counted and no scope is outer.
        if !on() {
            let scope = remap_scope(Remap::Scatter);
            assert!(!scope.outer);
            return;
        }
        let outer = remap_scope(Remap::Scatter);
        let inner = remap_scope(Remap::MapPhysicalPages);
        assert!(outer.outer && !inner.outer);
        drop(inner);
        drop(outer);
        assert!(remap_scope(Remap::MapPhysicalPages).outer);
    }

    #[test]
    fn the_lines_name_what_was_counted() {
        let p = Probe::new();
        p.allocated(
            Origin::VirtualCommit,
            0xa_0000_0000,
            300 << 20,
            MemType::WriteCombined,
            false,
        );
        p.gpu_bits(gpu_bits_of_protect(0x1804), 300 << 20);
        p.coded(Origin::XMemAllocHooked, 3, 4096);
        p.allocated(Origin::XMemAllocHooked, 0, 4096, MemType::Coded, false);
        let v = p.snapshot();
        let text = lines(&v, 10, true).join("\n");
        assert!(text.contains("SUMMARY of the run, 10 frames"), "{text}");
        assert!(text.contains("allocations, XMemVirtualAlloc commit: 1 of 300.0 MB: >=256M write-combined 1x 300.0 MB"), "{text}");
        assert!(
            text.contains("XALLOC memory-type codes: code 3: 1x"),
            "{text}"
        );
        assert!(text.contains("GPU bits: 0x1800: 1x 300.0 MB"), "{text}");
        assert!(text.contains("verdict, item 2"), "{text}");
        assert!(text.contains("no shadow traffic yet"), "{text}");
    }
}
