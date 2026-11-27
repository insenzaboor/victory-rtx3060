# Port tracker

_Generated from [`docs/data/ports.json`](data/ports.json) by `scripts/tracker/build.py` · updated 2026-11-26_ · **[Live dashboard](https://insenzaboor.github.io/victory-rtx3060/)** · **[Project board](https://github.com/users/insenzaboor/projects/2)**

![overall](assets/progress/overall.svg)

| Status | Meaning | Count |
|---|---|---|
| 🟢 shipped | On by default, verified in game runs | 23 |
| 🔵 on, untested | On by default, merged, not yet verified in a game run | 11 |
| 🔵 ready to test | Merged behind a switch (off), waiting for a game run | 32 |
| 🟡 in progress | Being ported or built | 0 |
| ⬜ planned | Designed or queued, not started | 9 |
| ⚪ no gain (off) | Measured: no gain or worse on this hardware (kept off) | 13 |
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
| Descriptor heap GPU start cached | 🔵 on, untested | aleksesipenko 41d8a1e |  | Small per-draw saving | [`31e16a63`](https://github.com/insenzaboor/victory-rtx3060/commit/31e16a63) | [#1](https://github.com/insenzaboor/victory-rtx3060/issues/1) |
| Per-thread lookup cache for host copies | 🔵 ready to test | fxceless 32be36d + bc6ee68 | `VICTORY_LOOKUP_CACHE=1` | Claimed -8 to -15 ms/frame on RTX 4070 | [`e1ac4e47`](https://github.com/insenzaboor/victory-rtx3060/commit/e1ac4e47) | [#3](https://github.com/insenzaboor/victory-rtx3060/issues/3) |
| Audio decoded on a worker thread | 🔵 ready to test | ours | `VICTORY_APU_ASYNC=1` | Opus decode was ~5% of title thread time (run 52 profile) | [`c6ed52d8`](https://github.com/insenzaboor/victory-rtx3060/commit/c6ed52d8) | [#4](https://github.com/insenzaboor/victory-rtx3060/issues/4) |
| Residency wait for textures only | 🔵 ready to test | Belobug#346 | `VICTORY_WAIT_RESIDENT=textures` | Buffers skip the residency wait | [`4bd60465`](https://github.com/insenzaboor/victory-rtx3060/commit/4bd60465) | [#29](https://github.com/insenzaboor/victory-rtx3060/issues/29) |
| Joined submissions 16 parts / 96 lists | ⬜ planned | Maykzera (RTX 3060) | `VICTORY_JOIN_PARTS=16, VICTORY_JOIN_LISTS=96` | 1% low +18%, P99 -15% on an RTX 3060 |  | [#5](https://github.com/insenzaboor/victory-rtx3060/issues/5) |
| In-place memory stages 1-3 (one heap per allocation, owned allocator, self-indexing descriptors) | ⬜ planned | ours (docs/design/in-place-memory.md) |  | Estimate 3-6 ms/frame (stage 1), 10-20 ms with stage 2, 1-4 GB RAM; waits on probe numbers | [`ceae18c0`](https://github.com/insenzaboor/victory-rtx3060/commit/ceae18c0) | [#50](https://github.com/insenzaboor/victory-rtx3060/issues/50) |
| Page mapping threads: 8 (default) vs 1 | ⚪ no gain (off) | ours |  | A/B run 48 with the first minute of streaming excluded: 31.0 vs 32.2 ms per 1k draws (noise); 8 kept. An earlier reading of 30.9 vs 37.7 included streaming phases. |  |  |
| Plain release build (no diagnostics) | ⚪ no gain (off) | cool_fox / CGBJ |  | q1-release vs q1-diag: 31.2 vs 31.6 ms per 1k draws (noise); loads fine (89 s). Use it to play, not for speed. | [`ca28c339`](https://github.com/insenzaboor/victory-rtx3060/commit/ca28c339) | [#2](https://github.com/insenzaboor/victory-rtx3060/issues/2) |
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
| Ray-tracing memory counting leak fixed | 🔵 on, untested | Belobug e9428969 | `VICTORY_BY_ADDRESS_COUNT (on)` | Areas stayed off the card after one did not fit | [`078fec6a`](https://github.com/insenzaboor/victory-rtx3060/commit/078fec6a) | [#24](https://github.com/insenzaboor/victory-rtx3060/issues/24) |
| Idle times for staging/windows, read-buffer cap | 🔵 ready to test | cool_fox cf3f615 | `VICTORY_STAGING_IDLE_MS, VICTORY_WINDOW_IDLE_MS, VICTORY_READ_KEEP_MB` | Run q5 with 10 s idle kept windows longer (+1.1 GB): must be set BELOW our 2-5 s defaults; retest in q5b at 1.5 s | [`a01ca023`](https://github.com/insenzaboor/victory-rtx3060/commit/a01ca023) | [#7](https://github.com/insenzaboor/victory-rtx3060/issues/7) |
| Caps on spare scratch and send memory | 🔵 ready to test | cool_fox 39318a8 | `VICTORY_SCRATCH_SPARE_MB, VICTORY_SEND_SPARE_MB` | Up to ~1.8 GB of unused upload memory | [`6bc60929`](https://github.com/insenzaboor/victory-rtx3060/commit/6bc60929) | [#8](https://github.com/insenzaboor/victory-rtx3060/issues/8) |
| Shared pool of indirect scratch buffers | 🔵 ready to test | Belobug e9428969 | `VICTORY_BATCH_POOL=1` | Less VRAM for idle lists | [`e8d846ca`](https://github.com/insenzaboor/victory-rtx3060/commit/e8d846ca) | [#25](https://github.com/insenzaboor/victory-rtx3060/issues/25) |
| Spare texture blocks scale with free VRAM | 🔵 ready to test | avatarmanz#94 / PR #119 | `VICTORY_SPARES_HEADROOM=1` | Late textures near the 12 GB budget | [`2f65cd92`](https://github.com/insenzaboor/victory-rtx3060/commit/2f65cd92) | [#31](https://github.com/insenzaboor/victory-rtx3060/issues/31) |
| Texture size cap under VRAM pressure | 🔵 ready to test | pr0vId3nce c392501 | `VICTORY_TEXTURE_MAX=<N>` | Halves at 92% VRAM, quarters at 97% | [`38e1a8f4`](https://github.com/insenzaboor/victory-rtx3060/commit/38e1a8f4) | [#44](https://github.com/insenzaboor/victory-rtx3060/issues/44) |
| Pool pieces packed into the fullest heap so empty heaps are freed | 🔵 ready to test | ours (memory accounting, run b1) | `VICTORY_POOL_PACK=1` | Expected -0.4 to -1.2 GB (0.3-1.45 GB of heap space sat unused) | [`566084a4`](https://github.com/insenzaboor/victory-rtx3060/commit/566084a4) | [#51](https://github.com/insenzaboor/victory-rtx3060/issues/51) |
| Idle host-list floor only while 8 GiB is free | 🔵 ready to test | ours (memory accounting, run b1) | `VICTORY_LIST_LEAN=1` | Expected -160 to -250 MB; risk of hitches when lists are remade | [`19f8ba47`](https://github.com/insenzaboor/victory-rtx3060/commit/19f8ba47) | [#52](https://github.com/insenzaboor/victory-rtx3060/issues/52) |
| Host lists borrow preludes from a shared pool | 🔵 ready to test | ours (memory accounting, run b1) | `VICTORY_PRELUDE_POOL=1` | Live preludes ~1,400 -> in-flight + 64: expected -0.4 to -0.8 GB | [`8a9cfc11`](https://github.com/insenzaboor/victory-rtx3060/commit/8a9cfc11) | [#53](https://github.com/insenzaboor/victory-rtx3060/issues/53) |
| Cap idle host lists (5.8-10.9 GB grew while lists were made) | ⬜ planned | ours | `VICTORY_ALLOC_TRIM` | Lead from run 51/52 memory lines |  | [#9](https://github.com/insenzaboor/victory-rtx3060/issues/9) |
| Card buffer cap 2 GB | ⬜ planned | grimygunnz / Rubenpannen | `VICTORY_CARD_BUFFERS_MB=2048` | Upstream 327 -> 61 ms on a 3070 Ti; our runs 38/38b hit the load hang, retest |  | [#10](https://github.com/insenzaboor/victory-rtx3060/issues/10) |
| Drop the CPU side of card copies (transient upload ring) | ⬜ planned | ours (memory accounting, run b1) |  | Biggest duplicate we own: about 1.5-2.5 GB; large shadow.rs change |  | [#54](https://github.com/insenzaboor/victory-rtx3060/issues/54) |
| Texture mip levels get VRAM on demand | ⚪ no gain (off) | ours (a26859fc) / used by CGBJ | `VICTORY_LEVELS_ON_DEMAND=1` | Run q5: host textures 3.4 -> 6.4 GB of RAM on this PC (Resizable BAR off); killed by the RAM monitor. Off. |  | [#6](https://github.com/insenzaboor/victory-rtx3060/issues/6) |

## Picture correctness

![rendering](assets/progress/rendering.svg)

| Item | Status | Source | Switch | Effect | Commit | Issue |
|---|---|---|---|---|---|---|
| Released heaps forget slots; textures without room retried | 🟢 shipped | Th3C0D3R e4245f3c |  | Correctness fixes from community pull requests | [`d11c364c`](https://github.com/insenzaboor/victory-rtx3060/commit/d11c364c) |  |
| Title's own upscaler instead of FSR4's fallback | 🔵 on, untested | chabrune 53057554 | `VICTORY_UPSCALER=6` | FSR4's fallback turns dithering into dots (hair, shadow edges) |  | [#11](https://github.com/insenzaboor/victory-rtx3060/issues/11) |
| In-place indirect arguments prepared alone, right before they run | 🔵 on, untested | avatarmanz 631400f..7ab13ed | `VICTORY_PORT_CONSTANTS_ORDER (on)` | Upstream: trees and palms no longer drop out. Bisect run b2 (off) was ruined by paging; kept on. | [`d005b299`](https://github.com/insenzaboor/victory-rtx3060/commit/d005b299) | [#12](https://github.com/insenzaboor/victory-rtx3060/issues/12) |
| Texture views clamped to the levels their copy has | 🔵 on, untested | Belobug e9428969 | `VICTORY_VIEW_BASE_FIX (on), VICTORY_KEPT_ROOM_LATER (on)` | Views of copies starting at a later mip; arrays too | [`3afba992`](https://github.com/insenzaboor/victory-rtx3060/commit/3afba992) | [#23](https://github.com/insenzaboor/victory-rtx3060/issues/23) |
| Descriptor slots given back after split submits | 🔵 on, untested | fxceless af23c652 (PR #1320) | `VICTORY_SPLIT_TABLES (on)` | Slot ring ran out on NVIDIA (262,144): flicker, missing geometry (avatarmanz#35) | [`26b3d211`](https://github.com/insenzaboor/victory-rtx3060/commit/26b3d211) | [#28](https://github.com/insenzaboor/victory-rtx3060/issues/28) |
| Skin-bake draws wait for their pipeline | 🔵 on, untested | GromX0 9dfbe7c | `VICTORY_BAKE_WAIT (on)` | A cold start could bake skin as nothing: missing faces, necks, arms | [`f2e785ae`](https://github.com/insenzaboor/victory-rtx3060/commit/f2e785ae) | [#38](https://github.com/insenzaboor/victory-rtx3060/issues/38) |
| Sampler filter value 2 decoded as point, not anisotropic linear | 🔵 on, untested | ours (found studying XCL; no XCL code) | `VICTORY_SAMPLER_ANISO_POINT (on)` | Point-sampled lookups and noise were blended under anisotropy: edge halos, possibly hair dots | [`5c9d31f0`](https://github.com/insenzaboor/victory-rtx3060/commit/5c9d31f0) | [#46](https://github.com/insenzaboor/victory-rtx3060/issues/46) |
| 10-bit float colour formats 116/117 mapped (RGBA16F) | 🔵 on, untested | ours (XCL study lead) |  | Were sized but never mapped; widened on upload | [`55eee34e`](https://github.com/insenzaboor/victory-rtx3060/commit/55eee34e) | [#48](https://github.com/insenzaboor/victory-rtx3060/issues/48) |
| Descriptor tables released when their work finishes | 🔵 ready to test | ours (ae6b0bab) / CGBJ default | `VICTORY_EARLY_TABLES=1` | Refused tables 2,200/frame -> 0; dropped draws come back |  | [#13](https://github.com/insenzaboor/victory-rtx3060/issues/13) |
| One oversized indirect record no longer empties the rest | 🔵 ready to test | CGBJ default | `VICTORY_INDIRECT_REST=0` | Picture completeness |  | [#14](https://github.com/insenzaboor/victory-rtx3060/issues/14) |
| Title textures uploaded in the submission prelude | 🔵 ready to test | Belobug 5d6bd841 | `VICTORY_PRELUDE_UPLOADS=1 (now off by default)` | Bisect: costs ~3 ms per 1k draws (~9%) on the submit thread (q1-diag 31.6, b3 off 28.8, all four off 27.6). Flicker benefit judged in queue q7. | [`a606b9cb`](https://github.com/insenzaboor/victory-rtx3060/commit/a606b9cb) | [#15](https://github.com/insenzaboor/victory-rtx3060/issues/15) |
| Ambient light follows the sky; reflection cap and least light | 🔵 ready to test | Belobug main (705a2d09, e48eb83e, c3bd5612) | `VICTORY_AMBIENT=sky, VICTORY_AMBIENT_REFLECT=0.5` | Interiors too dark or too bright; glossy-floor glaze indoors | [`7ac7d719`](https://github.com/insenzaboor/victory-rtx3060/commit/7ac7d719) | [#16](https://github.com/insenzaboor/victory-rtx3060/issues/16) |
| Changing textures rechecked every frame | 🔵 ready to test | ApexIV da3b0c2 | `VICTORY_TEXTURE_CHANGING=1` | Videos/screens no longer 12-40 frames late | [`6b50c44d`](https://github.com/insenzaboor/victory-rtx3060/commit/6b50c44d) | [#17](https://github.com/insenzaboor/victory-rtx3060/issues/17) |
| View minimum level of detail as its first mip | 🔵 ready to test | Belobug 5d7bd16d | `VICTORY_LOD_CLAMP=1` | Logs the first such views either way | [`cf933365`](https://github.com/insenzaboor/victory-rtx3060/commit/cf933365) | [#26](https://github.com/insenzaboor/victory-rtx3060/issues/26) |
| Light-probe ray programs run (violet vegetation) | 🔵 ready to test | Op-lite 50bd954 | `VICTORY_ACTIVATE_RAYS=1, VICTORY_RT_RESOLVE=1` | Skipped probes kept leftover memory: violet palms and bushes (queue q13) | [`50e3cffa`](https://github.com/insenzaboor/victory-rtx3060/commit/50e3cffa) | [#40](https://github.com/insenzaboor/victory-rtx3060/issues/40) |
| Constant-buffer tails past the title size zeroed | 🔵 ready to test | ours (adanaport 906e643 lead) | `VICTORY_CBV_TAIL_ZERO=1` | Leftover bytes in 256-byte views; candidate for too-bright hair (queue q14) | [`4abe4dc6`](https://github.com/insenzaboor/victory-rtx3060/commit/4abe4dc6) | [#43](https://github.com/insenzaboor/victory-rtx3060/issues/43) |
| Suspect: torn copy-backs with RELAY_COPY + CARD_BUFFERS_MIN_KB | ⬜ planned | fxceless#1268 | `VICTORY_FETCH_CHECK=1` | One-frame black flashes on a 4080; our launcher sets exactly this pair (queue q8) |  | [#34](https://github.com/insenzaboor/victory-rtx3060/issues/34) |
| Suspect: CARD_STEPS=2 flicker and wrong faces | ⬜ planned | fxceless#1268 (2080S, 4070 TiS, 5070 Ti) | `VICTORY_CARD_STEPS=0` | Fixes upstream, costs ~5 fps (queue q9) |  | [#35](https://github.com/insenzaboor/victory-rtx3060/issues/35) |
| Suspect: async submit (THIN=1) road flicker | ⬜ planned | fxceless#912 (5060, 3070, 5070) | `VICTORY_THIN=0` | Stretched polygons, road flicker (queue q10) |  | [#36](https://github.com/insenzaboor/victory-rtx3060/issues/36) |
| Suspect: port 2 misses view records on NVIDIA | ⬜ planned | Belobug#346 (3080 10 GB) | `VICTORY_PORT_DESCRIPTORS=0` | 427,634 missed view records: late textures (queue q11) |  | [#37](https://github.com/insenzaboor/victory-rtx3060/issues/37) |
| Console-precompiled pipelines (device slot 0x218) | ⚪ no gain (off) | chabrune 84899fde | `VICTORY_PIPELINE_PARTS` | Used only in the first 2 s; implementing them made pictures worse upstream | [`43e832c3`](https://github.com/insenzaboor/victory-rtx3060/commit/43e832c3) |  |

## Loads, hangs, crashes

![stability](assets/progress/stability.svg)

| Item | Status | Source | Switch | Effect | Commit | Issue |
|---|---|---|---|---|---|---|
| Title bug: frame tracker present leaks its lock (load hang) | 🟢 shipped | ours | `VICTORY_TRACKER_UNLOCK (on)` | 14 of 17 launches hung once loads got faster; 6-byte byte-checked patch fixes it | [`dcb32bb1`](https://github.com/insenzaboor/victory-rtx3060/commit/dcb32bb1) |  |
| Frame meter counts occluded presents | 🟢 shipped | ours |  | Autopilot no longer mistakes loaded runs for hangs | [`6f014818`](https://github.com/insenzaboor/victory-rtx3060/commit/6f014818) |  |
| Fence sweep for the title's GPU watchdog | 🟢 shipped | ours | `VICTORY_FENCE_SWEEP` | Crashes at title+0x23cf683 gone | [`7c4c76a3`](https://github.com/insenzaboor/victory-rtx3060/commit/7c4c76a3) |  |
| Title always active (focus/minimise) | 🟢 shipped | ours | `VICTORY_ALWAYS_ACTIVE` | Occluded-window hangs | [`1a25173a`](https://github.com/insenzaboor/victory-rtx3060/commit/1a25173a) |  |
| A save folder Windows refused to delete no longer drops settings | 🔵 on, untested | Belobug 705a2d09 |  | The game no longer starts without its settings | [`92da03de`](https://github.com/insenzaboor/victory-rtx3060/commit/92da03de) | [#27](https://github.com/insenzaboor/victory-rtx3060/issues/27) |
| Driver compiler crashes on a pipeline worker caught and skipped | 🔵 ready to test | Belobug e9428969 | `VICTORY_COMPILER_GUARD=1` | A crashing pipeline is listed in warm-bad.txt and skipped from then on | [`2c65587b`](https://github.com/insenzaboor/victory-rtx3060/commit/2c65587b) | [#22](https://github.com/insenzaboor/victory-rtx3060/issues/22) |
| Padding past each card copy | 🔵 ready to test | grimygunnz 7c030137 | `VICTORY_CARD_PAD_KB=64` | Against page-fault device removals | [`a31ba178`](https://github.com/insenzaboor/victory-rtx3060/commit/a31ba178) | [#30](https://github.com/insenzaboor/victory-rtx3060/issues/30) |
| Descriptor handles kept within their block (shader clean-up) | 🔵 ready to test | GromX0 1465c1f | `VICTORY_BLOCK_HANDLES=1` | Fixed AMD compiler crashes; none known on NVIDIA | [`7e549889`](https://github.com/insenzaboor/victory-rtx3060/commit/7e549889) | [#45](https://github.com/insenzaboor/victory-rtx3060/issues/45) |

## Shader translation

![shaders](assets/progress/shaders.svg)

| Item | Status | Source | Switch | Effect | Commit | Issue |
|---|---|---|---|---|---|---|
| VICTORY_BVH_NO_HIT keyed into the shader cache | 🔵 on, untested | ours (found porting Op-lite 50bd954) | `VICTORY_BVH_NO_HIT` | With a warm cache the switch did nothing; now it applies (one cold retranslation) | [`50e3cffa`](https://github.com/insenzaboor/victory-rtx3060/commit/50e3cffa) | [#39](https://github.com/insenzaboor/victory-rtx3060/issues/39) |
| Loose-LDS forms of wave64 programs on 32-lane GPUs | 🔵 ready to test | deynesdev fab7fda | `VICTORY_WAVE_LDS_LOOSE=1` | Offline probe: all 27 wave64-only shaders translate (particles, decal trails, water volumetrics, hair shadows) | [`d157b329`](https://github.com/insenzaboor/victory-rtx3060/commit/d157b329) | [#18](https://github.com/insenzaboor/victory-rtx3060/issues/18) |
| ds_bpermute within halves, 32 KiB LDS kept to | 🔵 ready to test | deynesdev fab7fda | `VICTORY_WAVE32_FIXES=1` | Exact pieces of the same commit | [`d157b329`](https://github.com/insenzaboor/victory-rtx3060/commit/d157b329) | [#19](https://github.com/insenzaboor/victory-rtx3060/issues/19) |
| Split 64-lane masks treated as the mask | 🔵 ready to test | Th3C0D3R #1112 | `VICTORY_MASK_COPIES=1` | Translator correctness | [`2cf371ee`](https://github.com/insenzaboor/victory-rtx3060/commit/2cf371ee) | [#20](https://github.com/insenzaboor/victory-rtx3060/issues/20) |
| v_div_fixup priority order (0/x no longer +-Inf) | 🔵 ready to test | dng-nguyn 472518cb | `VICTORY_DIV_FIXUP=1` | NaN in the picture (2060 / 4070 reports) | [`0d4c1fc4`](https://github.com/insenzaboor/victory-rtx3060/commit/0d4c1fc4) | [#32](https://github.com/insenzaboor/victory-rtx3060/issues/32) |
| NaN scrubbed from colour outputs | 🔵 ready to test | aleksesipenko 59609080 | `VICTORY_NAN_SCRUB=1` | NaN dots and blobs (4070 TiS, 3060 reports); test for the dots on hair | [`5cb3e52a`](https://github.com/insenzaboor/victory-rtx3060/commit/5cb3e52a) | [#33](https://github.com/insenzaboor/victory-rtx3060/issues/33) |
| 16-bit halves through table views keep their bits | 🔵 ready to test | pr0vId3nce aa67414 / fxceless #927 | `VICTORY_HALVES=1` | Face animation buffers read as 0: mouths do not move, face specks (queue q13) | [`e95b92e1`](https://github.com/insenzaboor/victory-rtx3060/commit/e95b92e1) | [#41](https://github.com/insenzaboor/victory-rtx3060/issues/41) |
| SDWA 16-bit sign extension; f16 interpolation modifiers | 🔵 ready to test | pr0vId3nce aa67414 | `VICTORY_SDWA_SEXT=1, VICTORY_INTERP_F16_FIXES=1` | A 16-bit -1 came out as 0x0000FFFF | [`0ff6698d`](https://github.com/insenzaboor/victory-rtx3060/commit/0ff6698d) | [#42](https://github.com/insenzaboor/victory-rtx3060/issues/42) |
| f32 to f16 conversions round to nearest even everywhere | 🔵 ready to test | ours (XCL study lead) | `VICTORY_F16_NEAREST=1` | NVIDIA truncates (0x3c01) where the console rounds (0x3c02); verified on the RTX 3060 | [`7aed3d5a`](https://github.com/insenzaboor/victory-rtx3060/commit/7aed3d5a) | [#47](https://github.com/insenzaboor/victory-rtx3060/issues/47) |
| GS_VoxelizeGeneric (195 geometry shaders) | ⚫ n/a | design: MiamiGame/XTP |  | Never bound in 62 runs; design doc in docs/geometry-shaders.md | [`645819a2`](https://github.com/insenzaboor/victory-rtx3060/commit/645819a2) |  |

## Measurement and automation

![tooling](assets/progress/tooling.svg)

| Item | Status | Source | Switch | Effect | Commit | Issue |
|---|---|---|---|---|---|---|
| Autopilot: menus, roaming walk, screenshots, stuck detection | 🟢 shipped | ours |  | Hands-off measured runs |  |  |
| Runtime A/B switching of switches | 🟢 shipped | ours | `VICTORY_AB` | Both sides measured in the same scene |  |  |
| Timeline traces with critical-path breakdown | 🟢 shipped | ours | `VICTORY_TIMELINE` | Found every bottleneck above |  |  |
| RAM/commit monitor that kills the game before the PC pages to death | 🟢 shipped | ours |  | Remote sessions survived every out-of-memory run |  |  |
| 30 s memory breakdown line (our buckets vs private total) | 🟢 shipped | ours |  | Diagnostics builds: eight biggest buckets, unexplained card-shared memory, list/prelude counts | [`395d3fdc`](https://github.com/insenzaboor/victory-rtx3060/commit/395d3fdc) |  |
| On-screen fps / frame-time panel | 🔵 ready to test | Belobug e9428969 | `VICTORY_PANEL=1` | fps, frame-time graph, VRAM/RAM and per-core CPU, in release builds too | [`fcd42dfd`](https://github.com/insenzaboor/victory-rtx3060/commit/fcd42dfd) | [#21](https://github.com/insenzaboor/victory-rtx3060/issues/21) |
| Layout probes: where descriptors, allocations and copy traffic come from | 🔵 ready to test | ours | `VICTORY_PROBE_LAYOUT=1` | Counts descriptor origins, allocation origins and refresh/fault/landing bytes by region; decides which in-place stage pays | [`540890ee`](https://github.com/insenzaboor/victory-rtx3060/commit/540890ee) | [#49](https://github.com/insenzaboor/victory-rtx3060/issues/49) |
