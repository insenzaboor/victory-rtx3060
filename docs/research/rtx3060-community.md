# RTX 3060 and nearby NVIDIA cards: what other people found

This note collects reports from other people running VIctory forks on NVIDIA
cards: the RTX 3060 (8 and 12 GB, desktop and laptop), the 3060 Ti, 3070,
3080, 2060 and 2070, and other 12 GB and 32-lane cards when the cause is the
same. Each report is checked against this tree (`av-3060`, branch `av-3060`).
It was gathered on 2026-11-26 from:

- the issues of Marvisak/VIctory, fxceless/victory-fork, avatarmanz/VIctory-7900XTX
  and Belobug/VIctory-Belobug-AMD (`gh issue list --search "3060 OR nvidia OR rtx"`);
- the READMEs, launch scripts and recent commits of the NVIDIA forks:
  Maykzera/victory-rtx3060, spatsariya/victory-RTX3060, canalmaloqueru-design/VIctory-3060,
  FluffyCyanNova/VIctory-RTX3060-Intel12100F, grimygunnz/nvidia-3070-VI,
  aleksesipenko/VIctory-RTX, nossemrew/VIctory-RTX3070, Rainbow-SPY/VIctory-for-RTX3070Ti-Laptop,
  dng-nguyn/victory-rtx2060-dlssg and ruaavell/VIctory---NVIDIA-RTX-2060
  (HypeLobbiesHD/VIctory-RTX3060 is empty);
- the repositories tagged 3060/3070/2060 in `research/scout.md`.

The GitHub-wide search for "3060" turned up no other relevant issues.
Maykzera/victory-rtx3060, aleksesipenko/VIctory-RTX and grimygunnz/nvidia-3070-VI
have no issues, and deynesdev/Protonida has issues turned off.

Our machine is an RTX 3060 12 GB with a Ryzen 5900X, 32 GB of RAM and
Resizable BAR off. It runs at 10-11 fps in the world with
`scripts/play-rtx3060.ps1` (av-pub). The glitches we see are flicker, late
textures, dark interiors and dots on hair.

What the status column means:

- **have**: already in this tree. The switch or code was found with `git grep`.
- **ported**: added by this note's commits, listed under "Ported here".
- **n/a**: not applicable to us.
- **open**: missing, and still worth doing.

## Ported here (all off by default, except the first)

| Commit | Switch | From | What it does |
|---|---|---|---|
| `26b3d211` | `VICTORY_SPLIT_TABLES` (on; `=0` restores the old path) | fxceless [`af23c652`](https://github.com/fxceless/victory-fork/commit/af23c6529ddc242107d2f2b09be268606e963685) (PR #1320, merged there) | `submit_split` now gives back the descriptor-table slots of the lists it runs at once, as `submit_cut` already did. Without this, NVIDIA's 262,144-slot dynamic ring runs out, tables are refused and draws drop. |
| `4bd60465` | `VICTORY_WAIT_RESIDENT=textures` | Belobug [#346](https://github.com/Belobug/VIctory-Belobug-AMD/issues/346) | Committed buffers skip the MakeResident wait, but texture memory still waits. Belobug measured 212,000 waits (60.8 s) going down to about 1,000. The faces stay clean, unlike with `=0`. |
| `a31ba178` | `VICTORY_CARD_PAD_KB=<KiB>` | grimygunnz [`7c030137`](https://github.com/grimygunnz/nvidia-3070-VI/commit/7c0301377fea85bdb447a49a3744d918be559482) | Adds room after each title buffer's copy in VRAM. Reads past the end then no longer hit unmapped card memory, which had caused MMU faults and 0x887A0006 in 4 of 19 runs on a 3070 Ti. Upstream uses 64. |
| `2f65cd92` | `VICTORY_SPARES_HEADROOM=1` | avatarmanz [#94](https://github.com/avatarmanz/VIctory-7900XTX/issues/94) / [PR #119](https://github.com/avatarmanz/VIctory-7900XTX/pull/119) | Scales how many empty texture blocks are kept by the VRAM left below 85 %, instead of always keeping 64 blocks (4 GiB). On 12 GB at 7 of 10.8 GiB that is 17 blocks. |
| `0d4c1fc4` | `VICTORY_DIV_FIXUP=1` | dng-nguyn [`472518cb`](https://github.com/dng-nguyn/victory-rtx2060-dlssg/commit/472518cbc421dafe3e1800f20f05c9fcf487b016) (also FluffyCyanNova b44/0010) | Puts `v_div_fixup_f32` back in the hardware's priority order, so `0 / x` and `x / Inf` give ±0 again instead of ±Infinity. Part of the shader cache key while on. |
| `5cb3e52a` | `VICTORY_NAN_SCRUB=1` | aleksesipenko [`59609080`](https://github.com/aleksesipenko/VIctory-RTX/commit/59609080f30801a9489fafee590086eb03bb8905) (from trustno1-coder 77e13a9) | Writes 0 instead of NaN or ±Infinity in pixel colour exports and f32 image stores, tested in integer arithmetic. A workaround against NaN dots, not a fix. Part of the shader cache key while on. |

How they were checked: `cargo check -p victory-gpu -j 2` passes, and
`cargo test -p victory-gpu --lib empty_blocks` covers the spares rule. The
game was not run. The two shader switches change translations, so turning
either one on makes a fresh shader cache for that run (cold start).

## Reports and status

### Flicker, black flashes, torn geometry

| Report | Card | Symptom | Cause found | Fix | Us |
|---|---|---|---|---|---|
| [fx#1268](https://github.com/fxceless/victory-fork/issues/1268) | 4080 | One-frame black "shard" flashes with `CARD_BUFFERS_MIN_KB=64`, about 18 a minute | With `RELAY_COPY=1`, the copy queue fetches small dynamic buffers while a later flight is still refilling them, so torn bytes are uploaded again (`host/torn.rs`) | `VICTORY_FETCH_CHECK=1` (0.9-3.5 torn pieces a frame caught, 0 of 60 frames corrupted) | **have**, off. Our launcher sets `RELAY_COPY=1` and `MIN_KB=64`, exactly the failing pair: try it |
| [fx#1268](https://github.com/fxceless/victory-fork/issues/1268) (2080S / 4070 TiS / 5070 Ti comments) | several | Triangles and texture flicker on the boat; faces wrong | `CARD_STEPS=2` and `TEXTURE_NO_WAIT=1` | `CARD_STEPS=0` (about 5 fps slower, clean); `TEXTURE_NO_WAIT=0` (faces right, and faster there) | **have** (switches). We run both at the risky values: A/B them |
| [fx#912](https://github.com/fxceless/victory-fork/issues/912) | 5060 / 3070 / 5070 | Road flicker and stretched polygons in town | `ASYNC_SUBMIT` | turn it off; `LATE_WAIT=2` against flashes | **have**. Here `VICTORY_THIN=1` turns the async path on too (`queue/behind.rs:804`): A/B `THIN=0` |
| [av#35](https://github.com/avatarmanz/VIctory-7900XTX/issues/35) | 3060 12 GB, 3070 Ti, 3080, 4090, 5080 | Flicker, missing heads, smear; 1.5-2.5k draws dropped a frame | 1M-view cap leaves 262,144 dynamic slots, held until the lists are recycled | release slots at landing; `bbcdb0a` TITLE_REACH | **have** (`ae6b0bab`, queue.rs `submit_cut`, `VICTORY_DYNAMIC_SLOTS_CAPPED`), plus `submit_split`: **ported** (`VICTORY_SPLIT_TABLES`). Check the log's "tables asked, N refused" line |
| [av#378](https://github.com/avatarmanz/VIctory-7900XTX/issues/378) | 2070S | Artifacts, black-looking textures, no gain | renderer ports 6 and 7 together (port 7 suspected) | none yet | **have** port 6 on, port 7 off. A/B `PORT_GUARD=0` |
| [fx#1319](https://github.com/fxceless/victory-fork/issues/1319) | 4070S | Black blocks and white haze against a clear sky; rainbow dots on NPCs | `AMBIENT_AIR=1.5` with BVH miss; render scale 0.5; wave32 shadow bugs | `AMBIENT_AIR` 0.8 or less, render scale 0.667 or more | **have** (we do not set `AMBIENT_AIR`) |

### Late or missing textures

| Report | Card | Symptom | Cause found | Fix | Us |
|---|---|---|---|---|---|
| [fx#776](https://github.com/fxceless/victory-fork/issues/776) | 3080 Ti / 4070 12 GB, 3060 | Dark walls, black surfaces; device removed in 1 run of 3 | On 12 GB, textures are starved at 90 % of the budget; `CARD_WINDOWS_MB=2048` takes their room | `LEVELS_ON_DEMAND=1` (frees 1-3 GB), `CARD_WINDOWS_MB` 512 or less, page file 32 GB or more | **have**, but only in `-Experimental`. We use 2304 MB of windows: A/B 1024 |
| [fx#879](https://github.com/fxceless/victory-fork/issues/879) | **3060 12 GB**, 5800X | 280 texture passes blocked for want of VRAM, 10.4 of 11.2 GB used | buffer and window copies competing with mip 0 | `LEVELS_ON_DEMAND=1` + `LEAN_WINDOWS=1` (9.1 GB, 0 blocked); `TEXTURE_DROP_MIPS=1`; keep `CARD_FETCH_INFLIGHT` off (7-9x slower on the 3060) | `LEVELS_ON_DEMAND` **have**; `LEAN_WINDOWS` and `TEXTURE_DROP_MIPS` **open** |
| [Belobug#346](https://github.com/Belobug/VIctory-Belobug-AMD/issues/346) | 3080 10 GB, 32 GB | `PORT_DESCRIPTORS=1` missed **427,634 view records** (about 145 a frame) on NVIDIA; textures took 14-20 s to appear | the memcpy hook of renderer port 2 marks 0 of about 1,813 copies a frame there; the 16-page fallback catches the rest late | `PORT_DESCRIPTORS=0` (`PORT_JOURNAL=1` costs +1.3 ms); `EARLY_TEXTURES_MS=50`, `TEXTURE_LOOK_MS=500` | **have** (switches). We ship `PORT_DESCRIPTORS=1`: **A/B `=0` first** |
| [av#94](https://github.com/avatarmanz/VIctory-7900XTX/issues/94) | 8 and 16 GB cards | Over the VRAM budget, or room held back from textures | `spares()` keeps 4 GiB of empty blocks whenever use is under 85 % | spares scaled by headroom | **ported** (`VICTORY_SPARES_HEADROOM=1`) |
| [Belobug#333](https://github.com/Belobug/VIctory-Belobug-AMD/issues/333) | 3090, 32 GB | Slow frames under commit pressure | VRAM counts against the commit limit one for one; residency pressure turns on and stays on | `FALLBACK_HEAPS=1` (532 to 370 ms); bigger page file | **have** (`RESIDENCY=1` pressure holds texture prefetch below 4 GiB of free commit): use a large fixed page file |
| [av#124](https://github.com/avatarmanz/VIctory-7900XTX/issues/124) | 3070 8 GB | Black geometry; "no host list for a state prelude" after 4-5 min | VRAM overcommitted by `TEXTURE_NO_WAIT`, `CARD_STEPS`, `WIDE_BACK`; allocation fails with 0x8007000E | set those to 0 | **have** (switches). We run all three on |
| [av#75](https://github.com/avatarmanz/VIctory-7900XTX/issues/75) | 3080 10 GB | Skin missing, washed-out frames, pop-in on first launch | draws dropped while their pipelines are still being made | relaunch; `PIPELINE_BOOK=1` | **have** (`PIPELINE_BOOK=1` set) |
| [fx#1211](https://github.com/fxceless/victory-fork/issues/1211) | 4060 8 GB | Black faces and skin | tint-palette textures packed within 4 KiB of each other count as clashing under `Rule::Start` (`HEAD=4096`), so their views go null | fork PR #923: the head is the smaller of 4 KiB and the texture's own size | **open** (`descriptor/host.rs` still uses `HEAD=4096`; the PR must be fetched) |
| [fx#1340](https://github.com/fxceless/victory-fork/issues/1340) | 3070 Ti (Linux) | Xid 31, 0x887A0005 | `TEXTURE_BACK` makes a committed read-back buffer per copy (966k in 21 min) | read-back pool (fork `b3694a26`, 117 lines) | n/a (we do not set `TEXTURE_BACK`) |

### Dark interiors and lighting

| Report | Card | Symptom | Cause found | Fix | Us |
|---|---|---|---|---|---|
| [fx#1235](https://github.com/fxceless/victory-fork/issues/1235) | 5080, 3060, 5070 Ti, 3080 | Interiors almost black; water dark; lips still | no ray-traced bounce light; 27 compute programs refused on 32 lanes (particles, decals, water volumetrics, **hair shadow masks**); lip sync from 16-bit buffer accesses (fork PR #927) | ambient stand-in; `WAVE_WIDE_ONLY` / loose LDS for some programs; #927 for lips | ambient **have** (`AMBIENT=0.8`, `AMBIENT=sky`, `AMBIENT_REFLECT`); loose LDS **have** (`-Experimental`); #927 not verified |
| [Belobug#11](https://github.com/Belobug/VIctory-Belobug-AMD/issues/11) | 4070, 5090D, 5070 | Dark indirect light | no ray-traced GI | `VICTORY_AMBIENT` | **have** |
| [Belobug#346](https://github.com/Belobug/VIctory-Belobug-AMD/issues/346) | 3080 10 GB | 16x16 probe halos around hair and clothing; brown indoor fog; backlit faces too dark | untraced probe blocks bleed up to 4x over the smooth estimate | local `AMBIENT_MOST=0` and a raised dark-face floor; `AMBIENT=0.95`, `AMBIENT_SHADE=0.7,0.6`, `AMBIENT_AIR=1.4` | `AMBIENT_SHADE` and `AIR` **have**; `MOST` is likely what `AMBIENT_REFLECT` (Belobug main) covers, **not verified** |
| [fx#1268](https://github.com/fxceless/victory-fork/issues/1268) (5070 Ti comment) | 5070 Ti, 4070 TiS | Room lamp light missing, rooms dark | `VICTORY_SKIP_WC_BACK=1` (fork #868) | gate by size (`SKIP_WC_MIN_KB=1024`) or leave it off | n/a (we lack `SKIP_WC_*`) |
| [fx#1076](https://github.com/fxceless/victory-fork/issues/1076) | **3060 12 GB** (x2) | 3-4 fps; black shade and faces; launch-to-launch glitches (blocky sun outline, black wall decals) | upstream removes the device on a B4G4R4A4 RTV / B5G5R5A1 UAV; the fork's stand-in formats avoid it | stand-ins; `QUEUE2`, `INDIRECT_GATHER`, `CARD_HASH_WIDE` (#1014) | stand-ins **have** (`resource.rs` `target_format`); the others **open** |
| Maykzera [`c05b44b3`](https://github.com/Maykzera/victory-rtx3060/commit/c05b44b34a52a22092e3fddcbd1d2e850561d61b) | 3060 12 GB | D3D12 error 646 in the light stand-in | Radiance/Air passes bind the image UAV as the depth SRV table | bind depth only for Resolve, and only when its size matches | **open** (validation-only error; our ambient code has moved, so it needs a hand fit) |
| grimygunnz [`f63f51cd`](https://github.com/grimygunnz/nvidia-3070-VI/commit/f63f51cdd126be7f64ad7041ba9ca3c0b76bcbe5) | 3070 Ti | Banding in dark rooms | the stand-in's output is too smooth | 2 lines of per-pixel grain in the ambient HLSL | **open** (cosmetic) |

### Dots, speckles, NaN

| Report | Card | Symptom | Cause found | Fix | Us |
|---|---|---|---|---|---|
| [fx#57](https://github.com/fxceless/victory-fork/issues/57) | **3060 12 GB**, 7700K | Glowing or flying hair strands, NPCs with dark "clouds" round their heads | `269b802f878c24e1` (`CS_InterpolateStrandStyle`) skipped through a copied "3060 Ti" skip list; loop limit 32 | keep it out of `VICTORY_SKIP_PIPELINES`; loop limit 256 or more | n/a (our launcher skips nothing; the loop limit is 2^20, diagnostics builds only) |
| dng-nguyn [`472518cb`](https://github.com/dng-nguyn/victory-rtx2060-dlssg/commit/472518cbc421dafe3e1800f20f05c9fcf487b016) | 4070 12 GB / 2060 | `0 / x` came out as ±Infinity, so NaN reached the picture | `v_div_fixup_f32` priority wrong (a NaN quotient above `make_zero`) | ISA priority order | **ported** (`VICTORY_DIV_FIXUP=1`) |
| aleksesipenko [`59609080`](https://github.com/aleksesipenko/VIctory-RTX/commit/59609080f30801a9489fafee590086eb03bb8905), Maykzera launcher | 4070 TiS, **3060 12 GB** | Black blobs, white-outs, dots | NaN and infinite values left in colour outputs | NaN scrub | **ported** (`VICTORY_NAN_SCRUB=1`) |
| ours (launcher comment) | 3060 | Dots on hair and shadow edges | FSR4's fallback upscaler | `VICTORY_UPSCALER=6` (the title's own) | **have** |
| [Belobug#346](https://github.com/Belobug/VIctory-Belobug-AMD/issues/346) | 3080 10 GB | Rainbow "confetti" on faces and arms | `WAIT_RESIDENT=0`: the decoration bake sampled unmapped texture blocks | `WAIT_RESIDENT=textures` | **ported** (we run the default, which already waits) |
| [Belobug#216](https://github.com/Belobug/VIctory-Belobug-AMD/issues/216) | 4060 Laptop, 780M | Console lane ids collide in 2-D/3-D compute groups on wave32 hosts | `waveReadLaneFirst(flat)` rounded to 32; NVIDIA's 8x8 waves are not consecutive runs | disjoint thread ids (gomeaw `7eda5e36`, after Marvisak#2126 `411ed48`; about 178 lines + 278 of tests) | **open** (too large for this pass; best lead for depth-pyramid, tiled-lighting and shadow-mask speckles) |
| [Marvisak#726](https://github.com/Marvisak/VIctory/issues/726) / [#745](https://github.com/Marvisak/VIctory/issues/745) | "NVIDIA" (in fact Intel iGPUs) | `v_med3_f32` with NaN returns the wrong side | the driver folds `fcmp uno` | integer NaN test | **have** (`is_nan_f32`, used by `med3`) |
| dng-nguyn [`225d3d73`](https://github.com/dng-nguyn/victory-rtx2060-dlssg/commit/225d3d73ef78259569e297c57eb3d4fefa08c61b) | 4070 | 16-bit compares read the high half | VOP3 op_sel honoured on 16-bit `v_cmp` (the author says RDNA2 ignores it) | ignore op_sel on non-class compares | **open** (low: compilers do not emit op_sel there for GFX10) |

### Device removed, hangs, page faults (TDR)

| Report | Card | Symptom | Cause found | Fix | Us |
|---|---|---|---|---|---|
| [Marvisak#3330](https://github.com/Marvisak/VIctory/issues/3330) | **3060 12 GB** | DEVICE_HUNG entering the world | view heap over NVIDIA's 1M cap (1,245,184 asked); about 90 wave64 programs refused; Wind `00167d58cc5a78d7` over the TDR | cap the heap; skip that hash | **have** (capped heap, null slot) |
| [Marvisak#2220](https://github.com/Marvisak/VIctory/issues/2220), [av#182](https://github.com/avatarmanz/VIctory-7900XTX/issues/182) | 3060L, 3060 12 GB, 3070, 3080 | 60-394 compute pipelines refused (E_INVALIDARG): missing lighting and hair passes, hangs | `WaveSize(64)` on wave32-only NVIDIA | wave-wide fallback, loose LDS | **have** (wave-wide default, `WAVE_LDS_LOOSE`/`WAVE32_FIXES` behind `-Experimental`). Grep the log for "host pipeline refused" |
| [Marvisak#2258](https://github.com/Marvisak/VIctory/issues/2258) / [#2391](https://github.com/Marvisak/VIctory/issues/2391) / [#1952](https://github.com/Marvisak/VIctory/issues/1952) | 3060, 4060L, 4080 | 0x887A0001 / device removed | B5G5R5A1 with UAV; B4G4R4A4 RTV (format 115) | stand-in formats; no typed UAV in a format the card cannot store (Marvisak `ea488a9f`, #2462) | stand-ins **have**; the `typed_store` check is **open** (not hit on the 3060 per fx#1076) |
| grimygunnz [`7c030137`](https://github.com/grimygunnz/nvidia-3070-VI/commit/7c0301377fea85bdb447a49a3744d918be559482) | 3070 Ti 8 GB | MMU faults in shaders, 0x887A0006 | reads past a buffer copy in VRAM | 64 KiB of padding | **ported** (`VICTORY_CARD_PAD_KB`) |
| [Belobug#49](https://github.com/Belobug/VIctory-Belobug-AMD/issues/49) | 3060 Laptop 12 GB | TDR 0x887A0006 | NaN constant buffer in two dispatches | `SKIP_DISPATCH=cabb87599722e59a,2561af9577c47289` | **open** (no such switch; `2561af95` is also why aleksesipenko made `WAVE_LOCAL` strict: we have `VICTORY_WAVE_LOCAL=idiom`) |
| [fx#1338](https://github.com/fxceless/victory-fork/issues/1338) | **3060 12 GB** | about 20 ms of card time a frame; `=0` hangs 2 loads of 4 | `ExecuteIndirect` with the spread signature is slow on NVIDIA | fork PR #1352 `VICTORY_SPREAD_LEAN=1` (-12.8 ms GPU on a 3060) | **open** (spread **have**, on; LEAN is more than 60 lines) |
| [Belobug#13](https://github.com/Belobug/VIctory-Belobug-AMD/issues/13) | 5070 Ti, 4060 Ti | List close fails with E_INVALIDARG; black world | barrier "before" state not legalized | `legal_state` | **have** (`HostList::transition`) |
| [Marvisak#3344](https://github.com/Marvisak/VIctory/issues/3344) | 3060 | 5 failing tests on the capped-heap path | tests assume the full heap | Marvisak #2461, #3283 | n/a (different structure here) |

### Frame rate on the same class of card

| Report | Card | Result | Settings / change | Us |
|---|---|---|---|---|
| Maykzera/victory-rtx3060 | **3060 12 GB**, 5600X, 32 GB | 14-17 fps open world (Series S, scale 0.5); limited by the submit thread at ~58 ms | `RENDER_SCALE=0.5`, `DYNAMIC_SLOTS=524288`, `CARD_BUFFERS_MB=1536`, `JOIN_PARTS=16`, `JOIN_LISTS=96`, `PIPELINE_WAIT_MS=10000` | join-parts-16 planned (ports.json); fx#1268 warns that a 10 s pipeline wait froze for 12.7 s |
| [Belobug#346](https://github.com/Belobug/VIctory-Belobug-AMD/issues/346) | 3080 10 GB, 7800X3D | 3-5 fps up to 21-30 indoors | pooled `shadow::exact()` buffers with a 4-frame quarantine (code not published as a commit); `CARD_BUFFERS_MIN_KB=64` + `MB=2560`; `PIPELINE_WAIT_MS=15`, `WORKERS=4` | MIN_KB=64 **have**; `SHARED_HEAPS`/`CARD_HEAPS` cover part of the pooling; MB is 1024 in our launcher |
| [av#367](https://github.com/avatarmanz/VIctory-7900XTX/issues/367), [av#343](https://github.com/avatarmanz/VIctory-7900XTX/issues/343), [av#213](https://github.com/avatarmanz/VIctory-7900XTX/issues/213) | 2070S 8 GB, 3080, 4070 12 GB | 353 down to 180 ms; 4.6 up to 18 fps | own-bytes pool + `MIN_KB=64` + `MB=1024-1536`; `VIEW_BYTES=0`; ports 1/5 make the GPU wait | switches **have**; A/B `CARD_BUFFERS_MB=1536` |
| [av#371](https://github.com/avatarmanz/VIctory-7900XTX/issues/371) | 2070S 8 GB | 40-80 s of MakeResident waits | one MakeResident per buffer, mostly from `shadow::exact` | own-bytes pool | **have** (`SHARED_HEAPS=16` in the launcher; `UPLOAD_HEAPS` off); `WAIT_RESIDENT=textures` **ported** |
| [fx#1283](https://github.com/fxceless/victory-fork/issues/1283) | 3060 12 GB, i3 | 10x slowdown with the full 16-port stack | one of fork PRs #783-#791 (unresolved) | none | n/a |
| [fx#1109](https://github.com/fxceless/victory-fork/issues/1109), [fx#1076](https://github.com/fxceless/victory-fork/issues/1076), [Marvisak#2697](https://github.com/Marvisak/VIctory/issues/2697) | 3060 8 / 12 GB | 2-5 fps | older trees | none | n/a (we are past this) |

## What to try in the next run, by glitch

Change one switch at a time with `play-rtx3060.ps1 -More @{...}`. The first
item in each list is the most likely to help.

**Flicker**
1. `VICTORY_FETCH_CHECK=1`. fx#1268 found that our exact pair, `RELAY_COPY=1`
   with `CARD_BUFFERS_MIN_KB=64`, gives one-frame black flashes, and this
   switch removes them. Watch the frame-rate line for torn pieces caught.
2. `VICTORY_CARD_STEPS=0`. Triangles and flicker went away only with this. It
   costs about 5 fps.
3. `VICTORY_THIN=0`. `THIN` also turns on the async submit path that fx#912
   ties to flicker.
4. `VICTORY_SPLIT_TABLES` is now on. If the log's "tables asked, N refused"
   line still shows refusals, try `VICTORY_DYNAMIC_SLOTS_CAPPED=393216`. It
   takes those slots from the title's in-place range (the log line says how
   many stay), so check that no views come out null.

**Late textures**
1. `VICTORY_PORT_DESCRIPTORS=0`. On an NVIDIA card, port 2 missed 427k view
   records (Belobug#346).
2. `VICTORY_LEVELS_ON_DEMAND=1`, outside `-Experimental` too (fx#776, fx#879).
3. `VICTORY_SPARES_HEADROOM=1` (new).
4. `VICTORY_CARD_WINDOWS_MB=1024` instead of 2304, to give the textures
   room on 12 GB.
5. `VICTORY_TEXTURE_LOOK_MS=500` with `VICTORY_EARLY_TEXTURES_MS=50`.
6. Use a fixed page file of 32 GB or more, so that the residency pressure
   rule does not hold texture prefetch back.

**Dark interiors**
1. `VICTORY_AMBIENT=sky` with `VICTORY_AMBIENT_REFLECT=0.5`, or `-RayTracing`.
2. `VICTORY_AMBIENT=0.95` with `VICTORY_AMBIENT_SHADE=0.7,0.6` (Belobug#346's set).
3. Check that the log shows no "host pipeline refused" for the lighting
   programs (`2cc4c197`, `f133e35e`).
4. `TEXTURE_NO_WAIT=0`: faces and dark surfaces came out right with it on
   two other cards.

**Dots on hair**
1. `VICTORY_NAN_SCRUB=1` (new). If the dots go, a NaN is the cause.
2. `VICTORY_DIV_FIXUP=1` (new). This is the real fix if the NaN comes from
   `0 / x`. Try it alone, then with the scrub.
3. Keep `VICTORY_UPSCALER=6` and keep `VICTORY_SKIP_PIPELINES` empty
   (`269b802f878c24e1` is the hair strand pass).
4. If neither helps, the next lead is Belobug#216 (lane ids on wave32, open)
   and the refused hair shadow-mask programs (fx#1235). Grep the log for
   "host pipeline refused".

The two shader switches start a new shader cache, so judge the second run,
not the cold one.

## Open, worth porting next

- fork PR #923: `Rule::Start` head = min(4 KiB, texture size). About 10 lines,
  fixes black faces, and must be fetched from the PR.
- fxceless `a463dcb1` (PR #1336): `alias_of` holds the texture under the lock.
  This closes a use-after-free that can give wrong textures. It needs
  `add_ref_alive` and `Held`, about 45 lines.
- fxceless `46da5eec` (PR #1353): `catch_unwind` around `Pending::make`. Check
  first whether `VICTORY_COMPILER_GUARD` already covers it.
- gomeaw `7eda5e36`: disjoint thread ids for tiled wave32 groups (large).
- fork PR #1352 `VICTORY_SPREAD_LEAN` (-12.8 ms GPU on a 3060), and
  `VICTORY_CARD_HASH_WIDE` (#1014).
- Maykzera `d1fe0bd0`: send the card compare list on its own (D3D12 error 538).
- Rainbow-SPY `e04c0781` (Marvisak #2143): a program's constant table starts
  after its code, so NVIDIA does not reserve per-thread memory for it.
