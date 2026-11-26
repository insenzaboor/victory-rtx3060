# Port tracker

_Generated from [`docs/data/ports.json`](data/ports.json) by `scripts/tracker/build.py` · updated 2026-11-26_ · **[Live dashboard](https://insenzaboor.github.io/victory-rtx3060/)**

![overall](assets/progress/overall.svg)

| Status | Meaning | Count |
|---|---|---|
| 🟢 shipped | On by default, verified in game runs | 23 |
| 🔵 on, untested | On by default, merged, not yet verified in a game run | 3 |
| 🔵 ready to test | Merged behind a switch (off), waiting for a game run | 12 |
| 🟡 in progress | Being ported or built | 3 |
| ⬜ planned | Designed or queued, not started | 3 |
| ⚪ no gain (off) | Measured: no gain or worse on this hardware (kept off) | 10 |
| ⚫ n/a | Does not apply to NVIDIA / this title (documented why) | 2 |

## Frame time / CPU

![performance](assets/progress/performance.svg)

| Item | Status | Source | Switch | Effect | Commit | Issue |
|---|---|---|---|---|---|---|
| Title windows and small buffers in VRAM | 🟢 shipped | ours | `VICTORY_CARD_WINDOWS_MB=2304, VICTORY_CARD_BUFFERS_MB=1024` | Card no longer reads title memory over PCIe: ~300 -> ~130 ms (run 9 -> 10); 2304 MB moved ~0.75 GB off RAM (run 36) |  |  |
| Host buffers placed in heaps made resident once | 🟢 shipped | ours | `VICTORY_SHARED_HEAPS=16` | 102,900 committed buffers + MakeResident per run -> ~105/s; 16 MB heaps cut fragmentation waste 2.8 -> 0.9 GB | [`d147bd9a`](https://github.com/insenzaboor/victory-rtx3060/commit/d147bd9a) |  |
| Card buffers placed in heaps (reused pieces resent whole) | 🟢 shipped | ours | `VICTORY_CARD_HEAPS=64` | Part of run 36: 153 -> 138 ms at 3k draws | [`0d7ac684`](https://github.com/insenzaboor/victory-rtx3060/commit/0d7ac684) |  |
| card_dirty looks only at copies noted as changed | 🟢 shipped | ours | `VICTORY_CARD_DIRTY_LIST=1` | Removes an atomic upgrade + swap per copy per submission (was 12% of the submit thread) | [`5caad0cb`](https://github.com/insenzaboor/victory-rtx3060/commit/5caad0cb) |  |
| Structured views stay on their buffer's copy | 🟢 shipped | deynesdev 175ebc49 | `VICTORY_STRUCT_IN_PLACE=1` | Avoids ~19k extra copy buffers; renders correctly (run 37) | [`32d87b9a`](https://github.com/insenzaboor/victory-rtx3060/commit/32d87b9a) |  |
| Renderer port 1: constants heap read in place | 🟢 shipped | avatarmanz | `VICTORY_PORT_CONSTANTS=1` | No uploads or fills of the 174 MB constants heap |  |  |
| Renderer port 2: descriptor stores report their slots | 🟢 shipped | avatarmanz | `VICTORY_PORT_DESCRIPTORS=1` | Replaces a ~600,000-slot scan a frame |  |  |
| Renderer port 6: shared guard lock, per-block claims | 🟢 shipped | avatarmanz | `VICTORY_PORT_GUARD=1` | Title threads' waits 29 -> ~2 ms a frame (upstream) |  |  |
| Non-blocking logger | 🟢 shipped | ours |  | Logger lock stalls removed from frame chains | [`e72f733d`](https://github.com/insenzaboor/victory-rtx3060/commit/e72f733d) |  |
| Title thread affinity widened to all cores | 🟢 shipped | ours |  | Console masks pinned title workers to 4 cores; run 24 best of its day | [`76faf5f2`](https://github.com/insenzaboor/victory-rtx3060/commit/76faf5f2) |  |
| BCPack decoder 1.56x | 🟢 shipped | ours |  | 27 -> 43 MB/s per thread, bit-exact | [`15eea45d`](https://github.com/insenzaboor/victory-rtx3060/commit/15eea45d) |  |
| Lock-free fast path for refreshes that copy nothing | 🟢 shipped | XaviFortes fdfb306d |  | ~15k of ~16k refreshes a frame copy nothing; part of run 48/49 gains | [`4884d2a7`](https://github.com/insenzaboor/victory-rtx3060/commit/4884d2a7) |  |
| Page mapping on 8 threads (kept, measured) | 🟢 shipped | ours |  | A/B run 48: 30.9 vs 37.7 ms per 1k draws with 1 thread |  |  |
| Descriptor heap GPU start cached | 🔵 on, untested | aleksesipenko 41d8a1e |  | Small per-draw saving | [`31e16a63`](https://github.com/insenzaboor/victory-rtx3060/commit/31e16a63) | [#1](https://github.com/insenzaboor/victory-rtx3060/issues/1) |
| Plain release build (no diagnostics) | 🔵 ready to test | cool_fox / CGBJ |  | Builds clean; autopilot keys now work in release builds | [`ca28c339`](https://github.com/insenzaboor/victory-rtx3060/commit/ca28c339) | [#2](https://github.com/insenzaboor/victory-rtx3060/issues/2) |
| Per-thread lookup cache for host copies | 🔵 ready to test | fxceless 32be36d + bc6ee68 | `VICTORY_LOOKUP_CACHE=1` | Claimed -8 to -15 ms/frame on RTX 4070 | [`e1ac4e47`](https://github.com/insenzaboor/victory-rtx3060/commit/e1ac4e47) | [#3](https://github.com/insenzaboor/victory-rtx3060/issues/3) |
| Audio decoded on a worker thread | 🔵 ready to test | ours | `VICTORY_APU_ASYNC=1` | Opus decode was ~5% of title thread time (run 52 profile) | [`c6ed52d8`](https://github.com/insenzaboor/victory-rtx3060/commit/c6ed52d8) | [#4](https://github.com/insenzaboor/victory-rtx3060/issues/4) |
| Joined submissions 16 parts / 96 lists | ⬜ planned | Maykzera (RTX 3060) | `VICTORY_JOIN_PARTS=16, VICTORY_JOIN_LISTS=96` | 1% low +18%, P99 -15% on an RTX 3060 |  | [#5](https://github.com/insenzaboor/victory-rtx3060/issues/5) |
| Renderer port 7: heap journal | ⚪ no gain (off) | avatarmanz | `VICTORY_PORT_JOURNAL` | A/B run 49: 24.4 vs 24.8 ms per 1k draws |  |  |
| Settle room | ⚪ no gain (off) | fxceless | `VICTORY_SETTLE_ROOM` | A/B run 49: no difference |  |  |
| Placed order | ⚪ no gain (off) | Vice-Tay | `VICTORY_PLACED_ORDER` | A/B run 48: 39.0 vs 37.7 ms per 1k draws (worse) |  |  |
| Draw stages share fresh tables | ⚪ no gain (off) | fxceless 5d659e0c | `VICTORY_TABLE_STAGES` | A/B run 37: no difference | [`5e8cea34`](https://github.com/insenzaboor/victory-rtx3060/commit/5e8cea34) |  |
| Stride shadow cache + coalesced unguard | ⚪ no gain (off) | XaviFortes 61477a81 / 8abd3671 | `VICTORY_STRIDE_SHADOW, VICTORY_UNGUARD_RUNS` | Run 50: 26.0 vs 25.2 ms per 1k draws |  |  |
| 8 descriptor view threads | ⚪ no gain (off) | ours | `VICTORY_VIEW_THREADS=8` | Run 51: 28.4 vs 25.2 ms per 1k draws |  |  |
| Dynamic descriptor slots in striped locks | ⚪ no gain (off) | ours | `VICTORY_DYNAMIC_STRIPES` | Loads 147-151 s instead of ~80 s | [`f164ca3b`](https://github.com/insenzaboor/victory-rtx3060/commit/f164ca3b) |  |
| Join parts (Spook) | ⚪ no gain (off) | SPOOKEXE | `VICTORY_JOIN_PARTS` | A/B run 33: noisy, no clear gain |  |  |
| Skip ray-tracing passes | ⚪ no gain (off) | LoverCatys / deynesdev | `VICTORY_SKIP_RT` | Run 31: ~206 ms (worse) |  |  |
| Renderer port 5: constant buffers read in place | ⚫ n/a | avatarmanz | `VICTORY_PORT_CBV` | Needs a 4.2M-view heap; NVIDIA caps heaps at 1M |  |  |

## RAM and VRAM

![memory](assets/progress/memory.svg)

| Item | Status | Source | Switch | Effect | Commit | Issue |
|---|---|---|---|---|---|---|
| Residency budgets (pareto-residency) | 🟢 shipped | cool_fox | `VICTORY_RESIDENCY=1` | Admission control against out-of-memory |  |  |
| Texture mip levels get VRAM on demand | 🔵 ready to test | ours (a26859fc) / used by CGBJ | `VICTORY_LEVELS_ON_DEMAND=1` | -3.1 GB VRAM allocated on an RTX 4070 |  | [#6](https://github.com/insenzaboor/victory-rtx3060/issues/6) |
| Idle times for staging/windows, read-buffer cap | 🔵 ready to test | cool_fox cf3f615 | `VICTORY_STAGING_IDLE_MS, VICTORY_WINDOW_IDLE_MS, VICTORY_READ_KEEP_MB` | Upstream: up to 40-46 GB kept alive in long runs | [`a01ca023`](https://github.com/insenzaboor/victory-rtx3060/commit/a01ca023) | [#7](https://github.com/insenzaboor/victory-rtx3060/issues/7) |
| Caps on spare scratch and send memory | 🔵 ready to test | cool_fox 39318a8 | `VICTORY_SCRATCH_SPARE_MB, VICTORY_SEND_SPARE_MB` | Up to ~1.8 GB of unused upload memory | [`6bc60929`](https://github.com/insenzaboor/victory-rtx3060/commit/6bc60929) | [#8](https://github.com/insenzaboor/victory-rtx3060/issues/8) |
| Cap idle host lists (5.8-10.9 GB grew while lists were made) | ⬜ planned | ours | `VICTORY_ALLOC_TRIM` | Lead from run 51/52 memory lines |  | [#9](https://github.com/insenzaboor/victory-rtx3060/issues/9) |
| Card buffer cap 2 GB | ⬜ planned | grimygunnz / Rubenpannen | `VICTORY_CARD_BUFFERS_MB=2048` | Upstream 327 -> 61 ms on a 3070 Ti; our runs 38/38b hit the load hang, retest |  | [#10](https://github.com/insenzaboor/victory-rtx3060/issues/10) |

## Picture correctness

![rendering](assets/progress/rendering.svg)

| Item | Status | Source | Switch | Effect | Commit | Issue |
|---|---|---|---|---|---|---|
| Released heaps forget slots; textures without room retried | 🟢 shipped | Th3C0D3R e4245f3c |  | Correctness fixes from community pull requests | [`d11c364c`](https://github.com/insenzaboor/victory-rtx3060/commit/d11c364c) |  |
| Title's own upscaler instead of FSR4's fallback | 🔵 on, untested | chabrune 53057554 | `VICTORY_UPSCALER=6` | FSR4's fallback turns dithering into dots (hair, shadow edges) |  | [#11](https://github.com/insenzaboor/victory-rtx3060/issues/11) |
| In-place indirect arguments prepared alone, right before they run | 🔵 on, untested | avatarmanz 631400f..7ab13ed | `VICTORY_PORT_CONSTANTS_ORDER (on)` | Trees and palms no longer drop out (upstream runs pop-14..17) | [`d005b299`](https://github.com/insenzaboor/victory-rtx3060/commit/d005b299) | [#12](https://github.com/insenzaboor/victory-rtx3060/issues/12) |
| Descriptor tables released when their work finishes | 🔵 ready to test | ours (ae6b0bab) / CGBJ default | `VICTORY_EARLY_TABLES=1` | Refused tables 2,200/frame -> 0; dropped draws come back |  | [#13](https://github.com/insenzaboor/victory-rtx3060/issues/13) |
| One oversized indirect record no longer empties the rest | 🔵 ready to test | CGBJ default | `VICTORY_INDIRECT_REST=0` | Picture completeness |  | [#14](https://github.com/insenzaboor/victory-rtx3060/issues/14) |
| Changing textures rechecked every frame | 🔵 ready to test | ApexIV da3b0c2 | `VICTORY_TEXTURE_CHANGING=1` | Videos/screens no longer 12-40 frames late | [`6b50c44d`](https://github.com/insenzaboor/victory-rtx3060/commit/6b50c44d) | [#17](https://github.com/insenzaboor/victory-rtx3060/issues/17) |
| Title textures uploaded in the submission prelude | 🟡 in progress | Belobug 5d6bd841 |  | Fixes frame-order flickering (Belobug) |  | [#15](https://github.com/insenzaboor/victory-rtx3060/issues/15) |
| Ambient light follows the sky; least reflection light | 🟡 in progress | Belobug main (705a2d09, e48eb83e, c3bd5612) |  | Interiors too dark/bright |  | [#16](https://github.com/insenzaboor/victory-rtx3060/issues/16) |
| Console-precompiled pipelines (device slot 0x218) | ⚪ no gain (off) | chabrune 84899fde | `VICTORY_PIPELINE_PARTS` | Used only in the first 2 s; implementing them made pictures worse upstream | [`43e832c3`](https://github.com/insenzaboor/victory-rtx3060/commit/43e832c3) |  |

## Loads, hangs, crashes

![stability](assets/progress/stability.svg)

| Item | Status | Source | Switch | Effect | Commit | Issue |
|---|---|---|---|---|---|---|
| Title bug: frame tracker present leaks its lock (load hang) | 🟢 shipped | ours | `VICTORY_TRACKER_UNLOCK (on)` | 14 of 17 launches hung once loads got faster; 6-byte byte-checked patch fixes it | [`dcb32bb1`](https://github.com/insenzaboor/victory-rtx3060/commit/dcb32bb1) |  |
| Frame meter counts occluded presents | 🟢 shipped | ours |  | Autopilot no longer mistakes loaded runs for hangs | [`6f014818`](https://github.com/insenzaboor/victory-rtx3060/commit/6f014818) |  |
| Fence sweep for the title's GPU watchdog | 🟢 shipped | ours | `VICTORY_FENCE_SWEEP` | Crashes at title+0x23cf683 gone | [`7c4c76a3`](https://github.com/insenzaboor/victory-rtx3060/commit/7c4c76a3) |  |
| Title always active (focus/minimise) | 🟢 shipped | ours | `VICTORY_ALWAYS_ACTIVE` | Occluded-window hangs | [`1a25173a`](https://github.com/insenzaboor/victory-rtx3060/commit/1a25173a) |  |

## Shader translation

![shaders](assets/progress/shaders.svg)

| Item | Status | Source | Switch | Effect | Commit | Issue |
|---|---|---|---|---|---|---|
| Loose-LDS forms of wave64 programs on 32-lane GPUs | 🔵 ready to test | deynesdev fab7fda | `VICTORY_WAVE_LDS_LOOSE=1` | Offline probe: all 27 wave64-only shaders translate (particles, decal trails, water volumetrics, hair shadows) | [`d157b329`](https://github.com/insenzaboor/victory-rtx3060/commit/d157b329) | [#18](https://github.com/insenzaboor/victory-rtx3060/issues/18) |
| ds_bpermute within halves, 32 KiB LDS kept to | 🔵 ready to test | deynesdev fab7fda | `VICTORY_WAVE32_FIXES=1` | Exact pieces of the same commit | [`d157b329`](https://github.com/insenzaboor/victory-rtx3060/commit/d157b329) | [#19](https://github.com/insenzaboor/victory-rtx3060/issues/19) |
| Split 64-lane masks treated as the mask | 🔵 ready to test | Th3C0D3R #1112 | `VICTORY_MASK_COPIES=1` | Translator correctness | [`2cf371ee`](https://github.com/insenzaboor/victory-rtx3060/commit/2cf371ee) | [#20](https://github.com/insenzaboor/victory-rtx3060/issues/20) |
| GS_VoxelizeGeneric (195 geometry shaders) | ⚫ n/a | design: MiamiGame/XTP |  | Never bound in 62 runs; design doc in docs/geometry-shaders.md | [`645819a2`](https://github.com/insenzaboor/victory-rtx3060/commit/645819a2) |  |

## Measurement and automation

![tooling](assets/progress/tooling.svg)

| Item | Status | Source | Switch | Effect | Commit | Issue |
|---|---|---|---|---|---|---|
| Autopilot: menus, roaming walk, screenshots, stuck detection | 🟢 shipped | ours |  | Hands-off measured runs |  |  |
| Runtime A/B switching of switches | 🟢 shipped | ours | `VICTORY_AB` | Both sides measured in the same scene |  |  |
| Timeline traces with critical-path breakdown | 🟢 shipped | ours | `VICTORY_TIMELINE` | Found every bottleneck above |  |  |
| RAM/commit monitor that kills the game before the PC pages to death | 🟢 shipped | ours |  | Remote sessions survived every out-of-memory run |  |  |
| On-screen fps / frame-time panel | 🟡 in progress | Belobug e9428969 | `VICTORY_PANEL` |  |  | [#21](https://github.com/insenzaboor/victory-rtx3060/issues/21) |
