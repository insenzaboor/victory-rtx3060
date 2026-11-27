# Title graphics memory in place

Status: plan. Stage 0 (the probes) is built; stages 1 to 3 are not.
Target PC for the numbers below: RTX 3060 12 GB, 32 GB RAM, Resizable BAR off,
GTA VI at about 10 fps, bound by the CPU on the thread that prepares
submissions, and by RAM at 22 to 25 GB.

The ideas come from a study of another compatibility layer
(`research/xcl-study.md`, items H2 and H3). That layer is GPL-2.0-only. This
plan is written from the study's own-words summary and from our code. It
reuses nothing of that project's code, and its files were not read for it.

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
  Scattered views stay copied.
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
