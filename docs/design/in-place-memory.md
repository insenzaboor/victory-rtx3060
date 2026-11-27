# Title graphics memory in place

Status: plan. Stage 0 (the probes) is built; stages 1 to 3 are not. Driver
heaps for in-place memory (`VICTORY_UMA_DRIVER`) are built, off by default.
Stage 2b for NVIDIA is decided on paper (run q15) and not built: [Stage 2b:
physical pages in place](#stage-2b-physical-pages-in-place).
Target PC for the numbers below: RTX 3060 12 GB, 32 GB RAM, Resizable BAR off,
GTA VI at about 10 fps, bound by the CPU on the thread that prepares
submissions, and by RAM at 22 to 25 GB.

The ideas come from a study of another compatibility layer
(`research/xcl-study.md`, items H2 and H3). That layer was GPL-2.0-only when
this plan was written, from the study's own-words summary and from our code,
and nothing of it was reused here. It is GPL-3.0-or-later since its commit
`06f90c198`; one piece of it is ported since then, the driver heaps below
([Driver heaps](#driver-heaps-victory_uma_driver), `docs/THIRD_PARTY.md`).

## Why

On the console the CPU and the GPU read and write one memory. We give the GPU
copies instead. A shadow (`shadow.rs`) is a host buffer per title buffer or
window, refreshed before each submission from what the write watch and the
guards say the CPU wrote (`watch.rs`). What shaders write is landed back into
title memory (`land.rs`, `shadow.rs` copy-back). Descriptor slots are records
of ours that are mirrored into host heaps at the same positions
(`descriptor.rs`, `descriptor/host.rs`). Each of these steps has a CPU cost
(noticing writes, moving bytes, waiting) and a RAM cost (the copy).
`docs/findings.md` puts most of the preparing thread's 48 ms (of a 55 ms frame
on the RX 7900 XTX) into these steps: refreshes 10 ms, write-watch asking
5 ms, waiting for descriptor records 6 ms, and 16 MB a frame landed.

Memory the GPU reads where the title wrote it needs none of those steps. Two
changes would remove most of them:

- **Item 3, title memory in place.** Title graphics memory is opened as D3D12
  heaps. Buffers placed in those heaps let the GPU read and write the title's
  own bytes over PCIe. Our allocator must own that memory, because an opened
  heap pins its pages and blocks the title's frees and protection changes
  inside it.
- **Item 2, descriptors that carry their host index.** A slot written through
  our API carries the index of its host descriptor in one global
  shader-visible heap. A shader that reads the slot finds the host view
  wherever the title copied the bytes to, so nothing has to be mirrored.

## What exists already

- `VICTORY_INPLACE` (`inplace.rs`, off by default). Level 1 places windows
  outside buffers, and level 2 places title buffers too, except the big ones
  on the card. Each one gets its own heap, opened on its own 64 KiB pages with
  `OpenExistingHeapFromAddress1` after a `VirtualQuery`. View memory (the
  physical page pool) is turned down. Level 2 must not be combined with
  `VICTORY_LATE_WAIT`.
- Renderer port 1 (`port.rs`, `VICTORY_PORT_CONSTANTS`). The title's 32-bit
  graphics heap is one run of one section. We map it a second time at an
  address of ours and open that view as one heap, so the title's own views
  stay free to change.
- `VICTORY_INPLACE_PROBE` (`inplace.rs`). It tries each kind of opening on
  memory of its own. On the software adapter it found these rules: a part of a
  private allocation opens, and the GPU then sees CPU writes made with no call
  in between. GPU writes show at the title's address. A committed section
  opens without locking anything and outlives its views. A reserved section
  (how the pool is made today) is refused. While a heap opened from an address
  stands, `VirtualFree` and `VirtualProtect` inside it fail with error 87.
  Tile mappings into such a heap remove the device. A heap is at most
  4095 MiB.

## Stage 0: the probes (built)

`VICTORY_PROBE_LAYOUT=1` turns them on (`crates/victory-gpu/src/layout_probe.rs`).
Lines prefixed `layout probe:` are written every 30 s with the last window and
the totals, and a `SUMMARY` is written when the title exits or terminates or
the window is closed. Off, each hook costs one relaxed load. On, the hooks do
atomic adds (descriptor uses go on 16 per-thread stripes). The probe also
allocates a byte per 64 KiB page, made per 4 GiB of address space as pages
are touched, plus 18 MiB of slot tables.

What it counts, and where:

| Question | Counted | Hook |
|---|---|---|
| Descriptor origin at use | each slot a draw reads through a table, as: a record our API wrote at that slot / a record of ours the title moved there as bytes / raw bytes put by our API's copy / raw bytes the title wrote / empty; by record kind (table entry, constant view, placed view); distinct slots per class | `descriptor/host.rs` `HostHeap::mirror` (for draws) |
| Our API's writes | records by kind; slots written by descriptor copies (a 32-bit fingerprint of slot and words per slot) | `descriptor.rs` `record`, `copy_simple` |
| Title byte writes | what the heap sync found changed, by the same classes | `descriptor.rs` `sync_heaps` |
| Allocation origin | calls and bytes by origin × size class (<64K, to 1M, to 16M, to 256M, more) × memory type (protection cache bits, D3D12 heap type, XALLOC code); XALLOC codes and console GPU-access bits as histograms | `xmem.rs` `XMemVirtualAlloc`, `XMemAlloc`, `XMemAllocDefault`, `XMemAllocatePhysicalPages`; `resource.rs` `create_heap`, `create_committed`, `create_at_address`, `create_placed`; `descriptor.rs` `create_heap` |
| Region of each page | origin of the region, whether a GPU resource or shadow lay in it, whether it was remapped after that | the allocation hooks, `Shadow::new`, remaps |
| CPU-written bytes | refresh bytes (watched / whole), refresh calls, guard faults, stores made by the fault handler, texture bytes read for refreshes | `shadow.rs` `update_free`; `watch.rs` fault handler; `texture.rs` `refresh` |
| GPU-written bytes | bytes landed back and pieces | `land.rs` `piece_at`; `shadow.rs` `put_changed` |
| RAM of copies | shadow bytes made, let go, in place | `Shadow::new`, `Drop`, `Shadow::placed` |
| Remaps | per call (XMemMapPhysicalPages, Scatter, CopyPageMappings; outermost call only): calls, pages mapped, unmapped, of those the pages a GPU resource or shadow used | `xmem.rs` |
| Frees | title `VirtualFree`s, those touching GPU pages, those in placeable regions | `xmem.rs` `title_virtual_free` |

Traffic is summed three ways. **Owned** is private memory, the XMem heaps and
our own memory, which an owned allocator could place up front. **Section** is
views of physical pages never remapped after the GPU used them, which one
committed pool section could hold. **Neither** is remapped views and memory of
unknown origin. The summary ends with a verdict line for each item.

**The next run.** Use the default switches plus `VICTORY_PROBE_LAYOUT=1`, and
optionally `VICTORY_INPLACE_PROBE=1` to get the driver's answers on the 3060.
Play 5 to 10 minutes covering boot, the world, driving and a cut scene. Run
once more with the probe off to check its cost: within 3 % fps.

**Decision thresholds.**

| Reading | Then |
|---|---|
| raw (title + API) ≤ 1 % of descriptor reads | item 2 as designed (stage 3) |
| raw > 1 %, a few distinct slots | stage 3 with the positional fallback for those slots |
| raw > 10 % or growing distinct raw slots | stop item 2: the title builds its own descriptors |
| Owned share ≥ 50 % of moved bytes | stage 1, then 2 |
| Owned share < 25 %, Section share large | go to stage 2b (pool section) early; stage 1 only for the private part |
| remapped GPU pages ≥ 1,000 a second in Section regions | placed buffers in the pool must follow remaps (stage 2b re-places); measure the re-place cost first |
| frees touching placeable GPU pages > 0 per minute | the owned allocator's free path is hot; stage 1 needs the release-before-free order (below) |

## Stage 1: `VICTORY_INPLACE` for whole allocation classes

Today every window and buffer is a heap of its own, decided when the shadow is
made, after a `VirtualQuery`. Instead, a whole class of allocations is opened
once, when it is allocated.

- New level `VICTORY_INPLACE=3`: everything level 2 does, plus every
  `XMemVirtualAlloc` that reserves and commits at once (origin
  "XMemVirtualAlloc commit") with GPU access bits, up to `VICTORY_INPLACE_MB`.
  It is opened whole (`OpenExistingHeapFromAddress`, `From::Region`) right
  after the call, with one placed buffer over all of it. A title buffer or
  window in it is then that buffer at an offset. No per-range heap is made,
  and no `VirtualQuery` is asked per shadow.
- Reserve-then-commit regions are opened per committed part on first GPU use,
  in 64 MiB parts, never per window.
- Port 1's 32-bit heap is folded under the same level, so one switch says
  "views opened in place".
- Descriptor heaps the title places in such memory become GPU-readable as a
  side effect, which stage 3 needs.

**Files.** `crates/victory-hle/src/modules/xmem.rs`: tell the GPU side about
committed graphics allocations, and call `title_memory_freeing` before the
title's `VirtualFree`/`VirtualProtect` on them, retrying while a heap is still
being let go. `crates/victory-gpu/src/inplace.rs`: a table of opened regions,
`place_for` looking there first, and a new `level` value.
`crates/victory-gpu/src/shadow.rs`: in-place shadows as offsets into a region
buffer. `crates/victory-gpu/src/watch.rs`: no guards or write-watch asking
inside opened regions. `crates/victory-gpu/src/host.rs`: refuse level 3 when
`VICTORY_LATE_WAIT` is on. `crates/victory-gpu/src/port.rs`: share the
region table.

**Risks.**
- *Frees and protection.* Inside an opened heap the title's
  `VirtualFree`/`VirtualProtect` fail with error 87. The runtime must let the
  heap go first, and the heap releases its pages only some time after its last
  reference, so the call has to retry with a bounded wait. The title's fence
  waits protect the GPU's last reads, as long as our fences signal only after
  the flights end (the default, not `VICTORY_LATE_WAIT`).
- *Pinning.* Opening locks every page into RAM (256 MiB untouched: 70 ms and
  256 MB of working set), so a sparsely used allocation costs RAM it never
  used. The probe's size classes show how much that is. Cap it with
  `VICTORY_INPLACE_MB`.
- *PCIe reads.* See [Cross-cutting risks](#cross-cutting-risks). Big buffers
  stay on the card, as level 2 already does (`cards()`).
- *GPU visibility.* The start-up self-test runs on the first region opened.
  If it fails, level 3 is dropped for the run.

**Success criterion.** In a 5-minute world run with `VICTORY_INPLACE=3`, the
probe's row for "private, committed at once" shows refreshed and landed bytes
down ≥ 90 % against level 0, and its shadow bytes are reported as in place.
The preparing thread's `a copy's refresh` time per frame falls by about the
same share of what that row carried. There are no device removals, no new
`not opened in place` warnings after the first eight, and frame captures are
unchanged.

**Fallback.** Levels 0, 1 and 2 are unchanged. Any allocation that will not
open, or is over the cap, keeps the copy path, which already handles memory
that is not in place.

## Stage 2: an owned allocator, D3D12 heaps opened once

Stage 1 retrofits the title's own allocations. Stage 2 makes them ours.

- **Windows.** At start-up, before the title's first allocation, reserve and
  commit a few windows of at most 4095 MiB each. One goes below 4 GiB for
  32-bit graphics memory. The others go in [4 GiB, 2^40) at 4 GiB-aligned
  bases, which keeps them under the BVH builder's 40-bit limit
  (`xmem.rs` `address_range`). Open each window once (`From::Region`) and
  place one buffer over all of it. A title byte's host GPU address is then the
  window buffer's address plus the byte's offset. Root views, buffer views and
  shadows of arena memory become offsets, and no per-range resource is made.
  Size the windows from the previous run's peak of routed allocations (written
  to the cache folder at exit), capped by `VICTORY_ARENA_MB`.
- **Routing.** `XMemVirtualAlloc` with GPU access bits (the probe's GPU-bits
  histogram says which) and graphics flags, `XMemAllocDefault` (both paths),
  `CreateHeap` and committed-resource memory of ours, and descriptor heaps all
  come from the windows. A 64 KiB-granular free list per window is used, and
  no allocation straddles a window.
- **The title's view of its memory.** A reservation gets window space at once.
  Commit is a no-op apart from zeroing pages that were used before. Decommit
  and release give the space back to our free list and never to Windows. The
  title's `VirtualProtect` inside a window is not passed to Windows (it would
  fail). `XMemVirtualQuery`/`VirtualQuery` answer from our table of the
  title's allocations, not from Windows.
- **Stage 2b, the page pool (optional, decided by the probe's Section
  share).** Make the pool's section `SEC_COMMIT` and open it once with
  `OpenExistingHeapFromFileMapping`. That locks nothing (256 MiB: 0.2 ms) and
  survives the title's views being mapped and unmapped. A title view that is
  one run of the section (`inplace::Cover::Run`) is a buffer at the section
  offset. When the title remaps pages the GPU uses, the buffer is placed again
  at the new offset (`title_memory_view` already reports every mapping).
  Scattered views stay copied. On NVIDIA this is replaced by [Stage 2b:
  physical pages in place](#stage-2b-physical-pages-in-place), which weighs
  it against the alternatives with run q15's numbers; it stays the design
  for cards without the import penalty.
- **Downstream.** For arena ranges, `shadow.rs` makes no copy, `watch.rs`
  neither guards nor asks, and `land.rs` has nothing to land. The flights in
  `host.rs` treat them as coherent: a later submission sees GPU writes in
  queue order, and the title's CPU sees them after the fence we already
  signal.

**Files.** `crates/victory-hle/src/modules/xmem.rs` (routing, the
sub-allocator, the title's view of its allocations, protection and query
calls), `crates/victory-gpu/src/inplace.rs` (windows, the self-test, the pool
heap), `crates/victory-gpu/src/resource.rs` (heaps, committed and
at-address resources in windows), `crates/victory-gpu/src/descriptor.rs`
(descriptor heap memory from the arena), `crates/victory-gpu/src/shadow.rs`,
`watch.rs`, `land.rs`, `memory.rs`, `crates/victory-gpu/src/host.rs` (flight
rules for coherent ranges).

**Risks.**
- *RAM and commit.* The windows are committed and pinned up front. That costs
  the whole arena in working set, which must be weighed against the shadow
  bytes it saves (the probe's "shadows made" per region). A `SEC_COMMIT` pool
  charges 6+ GiB of commit at once. `xmem.rs` already notes error 1450 on busy
  hosts, so stage 2b needs commit headroom: check `K32GetPerformanceInfo` and
  refuse with a clear line.
- *Address constraints.* 32-bit, 40-bit and alignment requests must all be met
  from the windows (`address_range`). A request the windows cannot meet takes
  the old path.
- *Emulated memory semantics.* The title must not be able to tell arena memory
  apart: zeroed commits, queries and protection. Unknown flags go to the old
  path.
- *PCIe reads, visibility, remaps*: see below.

**Success criterion.** With `VICTORY_ARENA=1` on the 3060:
- the probe's Owned-share bytes (refreshed, landed, texture reads excepted)
  fall to ≤ 5 % of their stage-0 value;
- the preparing thread gains at least the Owned share × (refresh + write watch
  + title bytes under shader writes) from the timeline, with a goal of
  ≥ 10 ms a frame and 10 fps → ≥ 12;
- working set goes down by at least 1 GB at the same scene;
- no device removal in 30 minutes;
- the self-test passes on the 3060 and on WARP.

**Fallback.** `VICTORY_ARENA=0` (the default until the criterion is met) is
today's path. A full arena sends the next allocation down the old path; both
paths coexist because everything downstream still handles non-arena memory.
If the self-test fails before the title's first allocation, the arena is off
for the run.

## Stage 3: descriptors that carry their host index

Our slots already hold records of our own (`VIEW_TAG`, entry, generation;
`descriptor.rs`), not console descriptors. A slot that holds bytes that are no
record of ours is made null today. A record uses 12 of a view slot's 32 bytes.

- **Encoding.** Word 3 of a record gets the index of the view in one global
  shader-visible heap, allocated when the table entry is made (views of
  resource objects) or when a placed view's shape and address are first
  recorded. Word 4 gets a check value (the index mixed with the tag), so a
  shader can tell a valid index from bytes that only look like one.
  Constant-buffer views named whole carry their address already. With stage 2
  the shader reads them from the window buffer at that offset, and without it
  they take the positional path.
- **Shaders.** The translated code loads words 3 and 4 from descriptor memory
  and checks them. If they are valid it indexes `ResourceDescriptorHeap`
  (Shader Model 6.6, which the 3060 has). Otherwise it uses today's positional
  index. The descriptor memory must be GPU-readable live: arena memory
  (stage 2), or a title-placed heap in memory opened in stage 1.
- **CPU.** Draws no longer check records of tables whose slots are indexed,
  and the heap sync no longer mirrors them. Host descriptors are made at view
  creation. Texture discovery moves from heap scans to view creation.
- **Lifetime.** When a resource is released its index is freed only after the
  flights that could read it end (the retire lists in `host.rs`). Until then
  the entry keeps the old view, and the generation in the record keeps CPU
  lookups honest.

**Files.** `crates/victory-gpu/src/descriptor.rs` (record words 3 and 4, index
allocation and freeing), `crates/victory-gpu/src/descriptor/host.rs` (the
global heap, with the positional heaps kept for the fallback),
`crates/victory-gpu/src/draw.rs` (skip record checks for indexed tables),
`crates/victory-shader/src/emit/lower.rs` and
`crates/victory-shader/src/resource.rs` (load, check, fallback),
`crates/victory-gpu/src/root.rs` and `signature.rs` (heap-indexed root
signatures), `crates/victory-gpu/src/sampler.rs` (deduplicated samplers in a
global sampler heap, 2048 at most), `crates/victory-gpu/src/shader_cache.rs`
(key bump).

**Risks.** One dependent load and a compare per resource access in shaders.
A translator version bump means every pipeline is rebuilt once (67-90 s
loads). Raw slots need the fallback (stage 0 says how many). Descriptor
memory must be GPU-visible. Reusing an index too early shows a wrong
texture for a frame.

**Success criterion.** Slots mirrored per frame (heap sync and draw mirrors)
down ≥ 90 %. The timeline's "the heaps' records" phases down by ≥ 5 ms a
frame. No differences in frame captures. Probe classes "API" plus "copied"
read through the index path.

**Fallback.** `VICTORY_DESC_INDEX=0`: the translator emits positional indexing
only (a different shader-cache key), and the runtime mirrors as today. Every
indexed access also carries the positional fallback, so a bad slot degrades
to today's behaviour, not to garbage.

## Driver heaps (`VICTORY_UMA_DRIVER`)

Built, off by default; ported from XCL (`f2248f068`, `f78e05bcc`;
`crates/victory-gpu/src/uma_driver.rs`).

**Why.** A heap opened on memory the process has (any of the three openings
above) is an import, and on NVIDIA the card reads imported pages slowly: XCL
measured GPU loads 8-24x and atomics 13-22x slower than from a driver
`CUSTOM` heap of write-back pages in the PC's memory (pool L0), and a whole
frame's replay at 1,427 ms of GPU time imported against 172 ms. Our run q21
fits: port 1 and in-place reads off gave 22.9 ms per 1,000 draws, on 27.0.

**What.** A title allocation that `XMemVirtualAlloc` reserves and commits at
once, read-write, at an address of ours, with the console's GPU access bits,
is made a driver heap at that same address before the call returns: its
written pages are copied out (none yet), it is released, every other free
stretch it would fit in is reserved for a moment (all but a margin of half
its size, 256 MiB at most, at each stretch's start), the heap is made
(`CUSTOM`, write-back, L0, `ALLOW_ONLY_BUFFERS | ALLOW_WRITE_WATCH`) and one
buffer over it mapped, and the grooming let go. If the mapping is not at the
address, or `GetWriteWatch` cannot read it, the memory is made again there;
if even that fails (another thread took the hole), the allocation is made
anew elsewhere: no title thread has the address yet. XCL does the same once,
before the title runs, for its coherent window; here it is per allocation,
because the title's memory is its own allocations. While a remake grooms,
our own mappings of title memory and the title's allocations wait on a lock
(`uma_driver::gate`). The software adapter keeps the address space of a heap
let go and maps the next heap of that size there, where the grooming cannot
reach: the remake then keeps the memory as it was. Whether NVIDIA's driver
does the same after the title frees a driver heap, the in-place line's
"mapped elsewhere" count will say.

**Who reads them.** `inplace::place_for` (`VICTORY_INPLACE=1` windows, `=2`
title buffers too) places a buffer in the driver heap, without
`ALLOW_CROSS_ADAPTER`, before it would import; nothing else changes for that
memory (`crate::watch` keeps its write watch, which the heap allows).
Renderer port 1 does not: its 32-bit heap is views of a section (physical
pages the title maps), which a driver heap cannot be, so it stays imported.
Making it a driver heap would mean xmem backing those physical pages with
the heap instead of the section (the reservation is mapped once, in order:
`xmem::arrange`), with every view and write path of those pages following.
That is the plan of [Stage 2b](#stage-2b-physical-pages-in-place).
Descriptor memory (stage 3) becomes GPU-readable in place only if the title
allocates it that way; nothing reads it yet.

**The title's calls on it.** A release of the whole lets the heap go (the
GPU's placed buffers keep it alive until they go). A decommit zeroes the
pages, and a commit there again is a no-op. A partial release fails, as
Windows' would.

**Limits.** `VICTORY_UMA_DRIVER_MIN_MB` (64): smaller allocations are left
alone, which keeps the grooming's margin at 32 MiB or more for mappings made
in that moment by threads that do not take the lock (the drivers'). The most
in driver heaps at once, `VICTORY_UMA_DRIVER_MB` (1024), and the RAM and
commit to leave free, `VICTORY_UMA_DRIVER_KEEP_MB` (2048): a driver heap is
resident at once, all of it, where the title's committed memory costs RAM
only as it is touched. On the 3060 with Resizable BAR off the heap is still
the PC's memory read over PCIe, about 25 GB/s at best: what is gained is the
import's penalty, not the link; big, often-read buffers still belong on the
card (`cards()`).

**Self-test.** At start-up (`uma_driver::before_title`, before the title's
TLS callbacks and `DllMain`s, which makes the host device then), on 64 MiB
of its own remade by the same steps: steps 1 to 4 below, the write watch
seeing a CPU write, a `VirtualProtect` read-only and back (the guards of
`crate::watch`), then the card's copy rate out of the driver heap and out of
the same memory imported, logged only. Any failure is logged at error level
and leaves the switch off for the run. With `VICTORY_INPLACE=0` it is off
too (nothing would read the heaps).

**A run.** `VICTORY_UMA_DRIVER=1 VICTORY_INPLACE=1` (or `=2`). The in-place
line every ten seconds says how many allocations became driver heaps, how
many buffers were placed in them, and why the others were left alone (size,
no GPU access bits, budget, RAM, the driver).

## Stage 2b: physical pages in place

Status: design, nothing built. Written from run q15 (RTX 3060, the default
switches with `VICTORY_PROBE_LAYOUT=1 VICTORY_INPLACE_PROBE=1`, port 1 off,
6,976 frames; log `run-q15-layout-probe-20261127-122309.log.err`) and runs
q16 and q21.

### What the 58 % is

The probe's "Section" share is traffic in views of physical pages that were
never remapped after the GPU used them. Its three parts, per frame:

| Part | Per frame | Since the start | Of the share |
|---|---|---|---|
| CPU-written bytes refreshed | 22.0 MB in 73 refreshes (~300 KB each) | 153,258 MB | 47 % |
| GPU-written bytes landed | 9.4 MB in 294 pieces (~32 KB each) | 65,710 MB | 20 % |
| Texture bytes read for refreshes | 15.5 MB | 107,957 MB | 33 % |

Write faults in these views: 1,624,984 (233 a frame). Shadows made there:
5,808 MB, let go 3,407 MB.

What else q15 says:

- **A third of the share cannot be in place.** Texture host copies are
  detiled; no option below keeps them in title memory. In-place memory can
  take at most the 31.4 MB a frame of buffers and windows: 67 % of the 58 %,
  39 % of all traffic.
- **The sections.** XMemAllocatePhysicalPages made four: 3,977, 297, 220 and
  185 MB (the 32-bit heap). Each is under the 4,095 MiB a heap may be.
- **The views.** 85,696 views (9,517 MB) of 4,679 MB of pages: 85,690 of
  them are one 64 KiB page, and the longest is the whole 3,977 MB section.
  Every page is shown about twice: the title keeps a view of its whole pool
  and maps single pages of it elsewhere as well (Scatter, and
  CopyPageMappings into the GPU's tiled heap). These are aliases that stand
  at the same time, not moves.
- **The layout of the copies** (census at 150 s). Title buffers in views:
  one run of a section 111 (140 MB), scattered pages 24 (384 MB). Windows:
  one run 383 (120 MB), scattered 1,840 (572 MB), with unmapped pages 1,596
  (1,398 MB). By bytes, about a tenth of the view memory under copies is one
  run of a section, which is what one placed buffer can show. With port 1
  off, `arrange` did not run, so the 32-bit heap's 174 MB buffer, which
  spans its five pieces, counted as scattered.
- **The 3060's openings** (q15's in-place probe). A part of private memory
  and a committed section open as the same heap: custom, write-back, L0,
  flags 0x421 (shared, cross-adapter, atomics). Opening 64 MB of a committed
  section that was never touched raised the working set by 65 MB and the
  card's shared memory by 64 MB, so on NVIDIA an opened section is resident
  whole at once (on WARP it was not). The card copied 64 MB out of a
  private import, a section import and a committed CUSTOM L0 buffer in
  20.5, 20.6 and 20.6 ms (about 3.2 GB/s). The copy engine does not show
  the import penalty.
- **An import of hot memory already lost on this card.** q16 (port 1 and
  `VICTORY_INPLACE=1`): 27.0 ms per 1,000 draws in the walk. q21 (both off):
  22.9. q15 (off): 21.4. Port 1 removes the 32-bit heap's copies, and the
  frame was still slower.
- **The time.** On the critical chain per frame (q15 timeline): a copy's
  refresh 4.1 ms, a texture's refresh 4.1 ms, a landing's job 1.9 ms, and
  `GetWriteWatch` 1.8 ms. Views never remapped carry 54 % of buffer refresh
  bytes (22.0 of 40.8 MB a frame), 51 % of landed bytes and most texture
  bytes. So the buffer copies of that memory are worth about 3.2 ms of the
  chain (0.54 × 4.1 + 0.51 × 1.9), plus
  their share of "work spread over threads". The texture refresh (4.1 ms)
  is a separate item.
- **RAM.** The walk's minimum free RAM was 2.53 GB. Runs q21, q23 and q23b
  were killed by the RAM monitor. Any option that pins memory the title has
  not touched fails on this PC.

### The options

**(a) The pool's sections opened with `OpenExistingHeapFromFileMapping`.**
xmem makes the physical-page sections `SEC_COMMIT`, as it already does under
Wine, and each is opened once as a heap. A title buffer or window that is
one run of a section (`inplace::Cover::Run`) becomes a buffer placed at the
section offset. Port 1 places in the section heap instead of opening its own
view.

- *Feasibility.* The call works on the 3060 (q15). It gives the same heap as
  `OpenExistingHeapFromAddress` (flags 0x421). Microsoft documents both calls
  as making a special-purpose diagnostic heap in system memory, one that
  survives device removal (breadcrumbs), and says nothing about speed. Both
  reach WDDM as an allocation over memory the process already has
  (`D3DDDI_ALLOCATIONINFO2`'s union of `hSection` and `pSystemMem`), so the
  kernel driver sees the same kind of allocation either way. We know of no
  NVIDIA statement or benchmark on how fast shaders read either one. XCL
  measured `FromAddress` only. Expected: the same penalty. Only `Run` ranges
  fit; scattered views cannot be one buffer, and tile mappings into such a
  heap remove the device.
- *Share of the 58 %.* The run part of the 31.4 MB a frame: the 32-bit heap
  (made a run by `arrange`) plus title buffers inside the 3,977 MB pool view.
  About 15-35 %. Nothing of the textures.
- *RAM.* The commit charge is about as today: the 3,977 MB pool view is
  committed whole when it is mapped (`Section::commit_shown`). But opening a
  section makes every page of it resident, also the ones never touched,
  which is up to 4.7 GB on a PC that the RAM monitor kills at about 1.2 GB
  free. Shadows of placed ranges are saved.
- *Risk.* High on NVIDIA: the import penalty lands on every GPU read of the
  memory placed there (q16's loss, over more memory), and the pinning comes
  on top.
- *Effort.* `xmem.rs` ~60 lines, `inplace.rs` ~150 (section heaps, placing
  `Run` ranges), `port.rs` ~30, the `uma_driver.rs` self-test ~40: about 300
  lines in 4 files.
- *Decider.* Step 1's self-test line: shader reads out of a file-mapping
  import against a driver heap. Slower by 2x or more, and (a) is off on
  NVIDIA. On cards without a known penalty (AMD) (a) stays stage 2's plan.

**(b) A driver heap as the pool, with the title's views emulated.** The GPU
never needs the title's address, only heap + offset; port 1 and `inplace`
already work that way. So the question is only whether a driver heap's
bytes can be the bytes the CPU sees through the section views. On Windows
and D3D12 they cannot:

1. A driver heap (`CreateHeap`, committed resources) has one CPU mapping, of
   the whole heap, at an address the driver picks (`Map`). No Windows call
   maps those pages anywhere else: `MapViewOfFile3` needs a section,
   `MapUserPhysicalPages` needs AWE frames, and `VirtualAlloc` cannot name
   pages. `CreateSharedHandle` on a shared heap returns a handle to a
   graphics-kernel shared allocation, not a section. Opening that handle
   again maps the whole heap a second time, again at an address the driver
   picks. That can be groomed once per heap, never as a 64 KiB view at the
   title's address, and a shared heap may itself take the slow path.
2. AWE (`AllocateUserPhysicalPages`, `MapUserPhysicalPagesScatter`) is the
   closest thing to the console: frames mapped at addresses, fast, with no
   address-space lock per view. But it needs `SeLockMemoryPrivilege` (a
   policy grant and a new logon) and locks all 4.7 GB. `MEM_PHYSICAL`
   ranges can be neither write-watched nor protected, so no guards would
   find writes in what stays copied. And the GPU would still need an import
   of those pages, the slow path again.
3. Reaching the GPU by offset is the right model, and it changes nothing
   above: the heap's bytes must be the CPU's bytes. With a driver heap they
   are not, and every CPU write has to be copied over. That is (c) or (d3)
   below, copies keyed by physical page. Placing scattered views by offset
   would need a page table in every translated buffer access. Rejected.

One form of (b) works: **a page that has only one view can live in a
driver heap at that view's address**, as XCL's pages live at their home.
The pages of a reservation that one allocation maps once, in address order,
and that are never shown anywhere else are such pages. The 32-bit heap is
one: one XMemAllocatePhysicalPages, five XMemMapPhysicalPages in one 185 MB
reservation, never tile-mapped (`port.rs`), and with `arrange` slice *i* is
the page at offset *i*. So the reservation becomes a driver heap the size of
the section, made by `uma_driver`'s remake at the reservation's address at
the first map, and the section's pages are that heap (call it **(b')**).

- *Feasibility.* The remake is the one `uma_driver` already does (grooming
  so the driver's `Map` lands in the hole), applied to a reservation of
  placeholders before any title code can touch it: placeholders fault until
  mapped, so there are no bytes to lose. xmem gives the section a home:
  `Section::window` returns home + index × 64 KiB, so DirectStorage's writes
  by page number land in the heap. A map at home is bookkeeping only. It
  needs the remake to land at the address on NVIDIA (q24's self-test says
  whether it does).
- *Share of the 58 %.* The 32-bit heap's part of the 31.4 MB a frame. q15
  does not split it out (step 0 does). Port 1's first runs measured 17 MB a
  frame sent up there in ~600 pieces; if that still holds, it is 20-40 % of
  the 58 %. It also takes away the guards' faults on those pages: the heap
  is write-watched (`ALLOW_WRITE_WATCH`).
- *RAM.* 185 MB resident at once. The title fills its heap anyway, so the
  net cost is about zero. At level 2 the card copy (~180 MB of VRAM) goes
  too. It fits `VICTORY_UMA_DRIVER_MB` (1,024).
- *Risk.* Medium:
  - The GPU reads that memory over PCIe out of the PC's memory instead of a
    copy on the card. The import penalty is gone, the link is not (q15
    copied at 3.2 GB/s).
  - A page of that section mapped elsewhere (an alias) cannot share the
    heap. None is expected; it is counted and logged as an error.
  - The title's queries, protections and frees in the range.
  - If the reservation cannot be restored after its release, the title
    already holds the address, so the process stops (`Lost::Stop`, as XCL
    does for its window).
- *Effort.* `xmem.rs` ~180 lines, `uma_driver.rs` ~200 (the reservation
  remake ~110, the self-test ~90), `inplace.rs` ~60 (`Rig::load_ms`),
  `port.rs` ~15: about 450 lines in 4 files, plus WARP tests.
- *Decider.* The walk's ms per 1,000 draws with `VICTORY_PORT_DRIVER=2`
  against q15 (21.4) and the port-1 import (q16, 27.0), on the same route.

**(c) Keep the copies, make them cheaper.**
CPU-to-GPU refreshes of view memory already go by dirty bits, not compares.
They use the 64 KiB guards (`watch.rs`) because views cannot be
write-watched: the kernel refuses them, and port 6 stopped asking (~300
refused askings a frame). Compares are left only where shaders' writes are
found (hashes on the card, `land.rs`), and in textures. So that views are
never remapped under the GPU buys no new dirty bits. What it allows:

- *4 KiB guards.* In private memory the kernel's 4 KiB parts cut the bytes
  copied by about 4x (29 of 114 MB, run cpu-2), which would take views' 22
  MB to 6-11 MB a frame. But a block's guard then faults up to 16 times
  instead of once. Today's 233 faults a frame are already costly: at 350 a
  frame they cost 58-62 ms of all threads' time (run best-1, the guards'
  lock). It is a loss.
- *Copies by physical page.* A page shown at n addresses is refreshed once.
  The alias factor is about 2 (9.5 GB of views on 4.7 GB of pages), but it
  sits mostly in remapped memory, the "neither" class.
- *Share and gain.* 10-20 % of the 58 % in bytes, and about 1 ms of the 3.2
  ms that view-buffer copies take on the chain. RAM: none.
- *Risk.* Low.
- *Effort.* 4 KiB guards ~100 lines (`watch.rs`); copies by page ~600
  (`shadow.rs`, `watch.rs`, `land.rs`).
- *Decider.* A run with 4 KiB view guards: faults a frame against
  "a copy's refresh" ms on the chain.

**(d) Other routes.**

- *d1, XCL's physical pages* (`crates/xcl/src/hle/kernelx.rs`, xcl/main). A
  page is in one place at a time. Its home is unified memory (the coherent
  window, a driver heap on NVIDIA, or a sparse window), and mapping it
  elsewhere moves its 64 KiB there and gives the old place back. That turns
  copies on write into copies on map: q15 mapped 649,000 pages (141,113 +
  170,863 + 337,023), about 42 GB over the run or 6 MB a frame, less than
  today's 31 MB of buffer copies. But it takes the page away from its old
  address, and GTA VI keeps both: the pool view and the single-page aliases
  stand together. It is not credible for this title unless the probe shows
  that the old address is never touched after an alias, which it cannot
  tell today.
- *d2, a recompiler-style hook of the title's allocator.* For the 32-bit
  heap the routine is known (image +0x1e86440). Replacing its physical
  pages with committed memory would make it private memory, which
  `uma_driver` already turns into a driver heap. That is (b')'s result, but
  tied to one binary and broken by the next title update, while (b') reads
  the same pattern from xmem's calls. The streaming pool's allocator is
  unknown, and its aliases are the whole point of it. No.
- *d3, one card copy per physical page, with tile mappings.* A DEFAULT-heap
  copy of each GPU-used page, and the title's GPU addresses as reserved
  buffers tile-mapped onto it: CopyPageMappings becomes `CopyTileMappings`,
  page maps become `UpdateTileMappings`, CPU writes are copied per page
  (guards). Aliases are copied once, and remaps need no new shadows (11.3 GB
  of shadows were made in remapped views: the "neither" class). GPU reads
  stay in VRAM. The copies stay. Weeks of work in `shadow.rs`, `watch.rs`,
  `land.rs`, `resource.rs` and xmem. This is the candidate for the pool after
  (b') (a stage 2c), for the scattered and remapped memory.

| Option | Works on Windows/D3D12/NVIDIA | Of the 58 % | RAM | Risk | Effort | Decided by |
|---|---|---|---|---|---|---|
| (a) section imports | yes, at the import's speed (expected slow) | 15-35 % | pins untouched pool pages, up to 4.7 GB | high | ~300 lines, 4 files | self-test: file-mapping import vs driver heap, shader reads |
| (b) driver pool, emulated views | no: a driver heap cannot be the section's pages | - | - | - | - | - |
| (b') 32-bit heap's pages in a driver heap | yes, if the remake lands (q24) | 20-40 % (step 0) | +185 MB, about net zero; -180 MB VRAM | medium | ~450 lines, 4 files | walk ms/1k draws, level 2 vs q15 and q16 |
| (c) cheaper copies | yes | 10-20 % of bytes, ~1 ms | none | low | 100-600 lines | 4 KiB guards: faults vs refresh ms |
| (d1) XCL's moves | breaks GTA VI's aliases | (all, if no aliases) | none | very high | large | no single run |
| (d3) per-page card copies | needs tile mappings, standard on DEFAULT heaps | copies stay, deduplicated | VRAM per GPU-used page | medium | weeks | later |

### Recommendation

On NVIDIA, stage 2b is **(b')**: the 32-bit heap's physical pages live in a
driver heap at their reservation, and port 1 places in it. It is the only
option that gives physical-page memory both in place and without the import
penalty. It reuses what is built (`uma_driver`'s remake, `arrange`, port 1's
placing), it costs no RAM this PC lacks, and its failure modes are logged
and limited to one 185 MB region. (a) is set aside on NVIDIA unless step 1
shows file-mapping imports reading at least half as fast as a driver heap.
Even then it pins pool pages the RAM monitor cannot spare. (b) in its
general form is impossible. (c) is worth about a millisecond. (d3) is the
road for the streaming pool afterwards. The textures (a third of the share,
4.1 ms of the chain) need their own item whichever option is picked.

### The plan

Every step is a value of one switch, read once in each crate as `port_on`
is: `VICTORY_PORT_DRIVER`, 0 (unset) off and exactly today. Each step is
A/B'd as pairs of runs on the autopilot's route against 0. The memory is
made before the world loads, so an A/B inside one run is not possible.

0. **No code: size it.** Run q15's switches plus `VICTORY_PORT_CONSTANTS=1`.
   With port 1 the 32-bit heap is in place, and the probe does not count
   traffic in place, so the Section bytes (refreshed + landed) that drop
   against q15 are the 32-bit heap's share, and (b')'s ceiling. Under about
   5 MB a frame, stop at step 1 and take (c) and d3 instead. Read q24 at the
   same time (below).
1. **The switch and the measurement.** `VICTORY_PORT_DRIVER=1|2` needs
   `VICTORY_PORT_CONSTANTS=1` (for `arrange`) and runs
   `uma_driver::before_title`'s self-test even with `VICTORY_INPLACE=0`.
   Without the self-test passing it is off, logged at error level. Nothing
   else changes yet. The self-test grows two lines (sketch below):
   - the card's copy rate out of a 16 MB `SEC_COMMIT` section opened with
     `OpenExistingHeapFromFileMapping`, beside the driver heap's and the
     address import's;
   - the time shaders take for dependent scattered 16-byte loads over 16 MB
     out of card memory (DEFAULT), the driver heap, the address import and
     the file-mapping import.

   Files: `uma_driver.rs` (+90), `inplace.rs` (`Rig::load_ms`, +60),
   `port.rs` and `xmem.rs` (the switch, +10). Done when one run logs the
   four load times.

   *Built.* Every self-test line now begins `UMA self-test:`. The card's
   steps are doubled from 16 until a run takes 5 ms; the other memories
   double up to the card's steps and stop at 5 ms too (a run is at most
   about twice the one before, so a very slow import never nears a device
   timeout), and are scaled to the card's steps (marked `~`). The lines say
   each time, the ratios against card memory and the driver heap, and
   whether the file-mapping import reaches half the driver heap's speed
   (option (a)'s decider). A refused opening is said as such; a device
   removed meanwhile fails the test. On WARP all four read alike.
2. **Level 1: the pages at home.** In `XMemMapPhysicalPages`, after
   `arrange`, the first map into a `LOW_RESERVATIONS` reservation whose
   section is as big as the reservation and wholly fresh calls a new
   `uma_driver::title_reservation(base, bytes)`, with the remake's lock held
   as writer. That call walks the reservation's placeholders (their
   boundaries), releases them, grooms, makes the heap (`CUSTOM`,
   write-back, L0, `ALLOW_ONLY_BUFFERS | ALLOW_WRITE_WATCH`), places and maps
   one buffer, and checks the address and the write watch. On failure it
   reserves the range as a placeholder again, split as it was; if even that
   fails, the process stops (`Lost::Stop`). The region goes into
   `uma_driver`'s table (`holds`, `place_for`). Then the section gets a home
   (a `OnceLock<usize>` beside `Section::window`, every commit bit set):
   - `Section::window` returns home + index × 64 KiB (the DirectStorage
     writes, `write_physical`, `page_window`);
   - a map of a slice at its home address makes no view: `Physical::views`
     notes it as homed, `title_memory_remapped` is called and
     `title_memory_view` is not, so `watch.rs` treats the range as
     write-watched memory, not as a view;
   - an unmap at home forgets the note, and the bytes stay, as a console
     page keeps them;
   - a slice of a homed section mapped anywhere else is an alias: an error
     line, a count on port 1's line, and a plain section view (its bytes are
     then not the heap's; the acceptance needs none);
   - the title's `VirtualFree` there goes to `uma_driver::title_free`, and
     its placeholder splits and joins there succeed doing nothing.

   Copies work as before, now over write-watched memory with 4 KiB parts.
   Files: `xmem.rs` (+180), `uma_driver.rs` (+110). Done when the log says
   the 32-bit heap is a driver heap at its address, with no aliases, the
   probe's view faults drop, and the walk is no worse than level 0.
3. **Level 2: port 1 in it.** `port::place_in` asks
   `uma_driver::place_for(start, len)` before `open_region`, so the title's
   buffers and windows there are buffers in the driver heap with no
   `ALLOW_CROSS_ADAPTER`. They are counted on port 1's line as "in a driver
   heap". `port.rs` +15. Done when, against level 0 on the same route, the
   probe's Section bytes fall by step 0's share and the walk's ms per 1,000
   draws is at most q15's 21.4 (q16's import: 27.0), with no device removal
   in 30 minutes and frame captures unchanged.
4. **Hardening and tests.** WARP tests in a process of their own (WARP keeps
   the address space of a heap let go, `uma_driver::tests::alone`):
   - a reservation split into five placeholders is remade, maps at home
     make no views, a write by page number shows in the heap and to the GPU
     through `place_for`, an alias is counted, a release lets the heap go;
   - the restore puts the placeholders back as they were.

   The census and the layout probe learn the homed range: it is counted as
   "in place", not as a view.
5. **Defaults and what comes next.** Level 2 goes on by default on NVIDIA
   once two route runs show the gain and a 30-minute run shows no aliases.
   A probe counter for reservations that one allocation maps once, in order,
   and never shows elsewhere (bytes and traffic) says whether (b') has
   another target. The pool's scattered and remapped memory waits for d3.

**Self-test sketch (step 1, not built).**

```hlsl
// Dependent scattered loads: latency-bound, as XCL's slow programs were.
#define RS "SRV(t0), UAV(u0), RootConstants(num32BitConstants=2, b0)"
ByteAddressBuffer src : register(t0);
RWByteAddressBuffer dst : register(u0);
cbuffer c : register(b0) { uint mask; uint steps; };  // mask: bytes / 16 - 1
[RootSignature(RS)]
[numthreads(64, 1, 1)]
void main(uint id : SV_DispatchThreadID) {
    uint at = id * 2654435761u, sum = 0;
    for (uint i = 0; i < steps; i++) {
        uint4 v = src.Load4((at & mask) << 4);
        sum += v.x ^ v.w;
        at = at * 1664525u + 1013904223u + v.y;  // the next load waits for this one
    }
    if (sum == 0x12345678u) dst.Store(0, sum);   // keeps the loads
}
```

```rust
// inplace::Rig: best of three, wall time around a fenced run; the dispatch
// is sized so that card memory takes 5 ms or more.
pub(crate) fn load_ms(&mut self, src: &ID3D12Resource, bytes: u64) -> windows::core::Result<f64>;

// uma_driver::self_test, after today's copy rates (RATE = 16 MB):
let section = CreateFileMappingW(INVALID_HANDLE_VALUE, null(), PAGE_READWRITE | SEC_COMMIT, 0, RATE as u32, null());
let heap = inplace::open(device, inplace::From::Section(section as usize))?;
let mapped = inplace::place(device, &heap, 0, RATE as u64)?; // cross-adapter
// lines: "copies 16 MB out of a file-mapping import at {} GB/s";
// "shaders read 16 MB in dependent 16-byte loads: card memory {} ms,
//  driver heap {} ms, address import {} ms, file-mapping import {} ms"
```

### What q24's self-test numbers change

q24 runs `VICTORY_UMA_DRIVER=1`. Its self-test logs the card's copy rate out
of the driver heap and out of the same memory imported.

| q24 says | Then |
|---|---|
| The remake failed: mapped elsewhere, or `GetWriteWatch` cannot read the mapping | (b') cannot be built as written on this driver. Stop after step 1 and take (c) and d3. |
| Driver heap and import copy at about the same rate (expected: q15's probe copied 64 MB in 20.5 / 20.6 / 20.6 ms) | Says nothing about shader reads, since the copy engine does not see the penalty. Step 1's load times decide. The plan is unchanged. |
| Driver heap copies much faster than the import | The penalty reaches copies too. (a) is off on NVIDIA for good, (b') has more to gain, and port 1's import should go off by default on NVIDIA. |
| Driver heap copies slower than the import | The driver heap is not what XCL measured on this card: (b') loses its premise. Check step 1's load times before step 2. |
| Either rate under ~3 GB/s | The link or the card's clocks are slow (q15's 3.2 GB/s already sits at the 3 GB/s floor of [the self-test](#the-start-up-self-test), step 5). Reads in the PC's memory of any kind lose to copies on the card, and level 2 is unlikely to win; level 1 (CPU only) still can. Measure the floor with the card awake before enforcing it. |
| q24's walk against q15 (driver heaps for private memory, `VICTORY_INPLACE=1`) | The best early evidence for level 2. If in-place private memory in driver heaps is no faster than its copies, level 2 will not be either. |

## The start-up self-test

Run it on every heap opened in place: each window of stage 2, the first
region of stage 1, and the pool heap of 2b. Use the probe's own queue
(`inplace::Rig`), before the title touches the memory:

1. The CPU writes a pattern at the window's first and last 64 KiB and at one
   page in the middle, with no D3D12 call. The GPU copies those ranges to a
   readback buffer, and they must match. This catches a runtime or driver that
   reports success but gives the GPU memory of its own.
2. The CPU writes a second pattern at the same places and the GPU copies
   again, with no call in between. This catches cached reads across
   submissions.
3. The GPU writes by a copy and by a compute shader through a UAV. After the
   fence the CPU reads at the title's address, and the bytes must match.
4. `VirtualQuery` still reports the window committed and read-write.
5. The GPU copies 64 MiB out of the window and out of a buffer in VRAM, timed.
   The ratio is the PCIe read rate. Log it, and refuse in-place memory below a
   floor (3 GB/s: a narrow or degraded link).

A failure is logged once, at error level, with the step that failed, and the
stage is off for the run. Steps 1 to 3 already exist in
`inplace::probe` (`private`), on memory of its own. The self-test runs them
on the real window.

## Cross-cutting risks

- **PCIe reads with Resizable BAR off (RTX 3060).** Resizable BAR only governs
  how much VRAM the CPU can map (256 MiB without it). It does not limit GPU
  reads of system memory, which run at the link's speed: PCIe 4.0 x16 gives
  about 25 GB/s at best, much less for small scattered reads, with latency in
  microseconds, against 360 GB/s for the card's own memory. So:
  - textures keep host copies on the card (they are detiled anyway, and the
    stages do not change that);
  - big, often-read, rarely-written buffers keep card copies (`cards()`,
    `card_limits`);
  - in place is for buffers the CPU writes every frame (constants, dynamic
    vertex data, readbacks) and the GPU's own writes the CPU reads.

  Without Resizable BAR the opposite design (title memory in VRAM that the CPU
  writes through the BAR) is not available on this PC at all. The self-test's
  step 5 and `inplace::cost` measure the real rate. The study's warning
  stands: the other layer put everything in system memory and its world runs
  at 5 fps on a much faster card.
- **GPU visibility of CPU writes.** Heaps opened on title memory are
  write-back system memory (the probe's "custom, write-back, L0"). The GPU's
  reads are snooped, so CPU writes made before `ExecuteCommandLists` are seen.
  Writes made while a submission runs are not guaranteed. The console title
  only writes what the GPU is not reading, as long as our fences are honest:
  they must signal after the flight ends, which is why late wait and level 2
  or above do not mix. GPU writes are not seen by the write watch, so nothing
  may rely on it for in-place ranges. The self-test checks the driver's side.
- **Remaps.** A heap opened from an address fixes that address's pages. The
  title cannot unmap a view inside it, so views are never opened by the
  title's address: only through our own view (port 1) or the section itself
  (2b). A buffer placed over a view shows the pages that were there when it
  was placed, so after a remap it is stale until it is placed again. The probe
  counts exactly these remaps (pages the GPU used and the title then
  remapped). `CopyPageMappings` into the tiled GPU heap is the likely main
  source, and those regions stay copied unless 2b re-placing is cheap.
- **Tile mappings into an opened heap remove the device.** Never call them.
  Reserved resources keep their own heaps.

## Feasibility before the data

These are estimates from reading the code, to be replaced by the probe's
numbers.

- **Item 2: likely viable (about 75 %).** Raw slots are made null today, and
  the title renders correctly, so raw descriptors that the title builds by
  hand should be close to zero. Byte copies of our records are certain: the
  heap sync and renderer port 2 exist because of them. So most reads should
  be "API" or "copied". The catch is the record kind. Constant-buffer views
  are the most numerous (about 14,000 written a frame), they are named whole,
  and they need stage 2's address path before they stop costing anything.
  Views of resource objects and placed views can use the index on their own.
  The likely gain is the 6 ms of record waits plus most of the 40,000
  mirrors a frame.
- **Item 3: partly viable (about 50 %).** Much GPU memory is views of physical
  pages, not private memory. That includes the 32-bit constant heap (port 1
  already handles it), the tiled GPU heap fed by `CopyPageMappings`, and
  streamed textures. A guess is 20-40 % of moved bytes Owned, 30-50 % Section
  and 10-30 % Neither. Stage 1 alone is then worth a few milliseconds a frame
  (3-6) on the preparing thread. Stages 2 and 2b together might be worth
  10-20 ms, which takes 10 fps to perhaps 12-13 before the card becomes the
  wall. RAM should fall by the shadows of placed memory (1-4 GB), less the
  pinned pages the title never touches.
- **Order of work.** Stage 0 data first. If the Owned share is high, stage 1
  is a cheap first win. Stage 2 is the main project (3-5 weeks). Stage 3
  depends on GPU-readable descriptor memory, so it comes after stage 1
  (title-placed descriptor heaps in opened memory) or stage 2.
