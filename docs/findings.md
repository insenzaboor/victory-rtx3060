# What was learned on the RX 7900 XTX (2026-11-23 to 2026-11-25)

One PC: i9-12900K (8 fast cores, 8 slow ones, 24 threads), 64 GB, Radeon RX 7900 XTX
24 GB, Windows 11, 1440p, Series X profile. Figures are of diagnostics builds with a
screenshot a second. Scenes are not the same from run to run, so read differences
under about 5 % as nothing. What a switch does is in the comment where it is read.

## Where a frame goes

Median of whole runs, same part of the prologue: 280 ms, then 124, 103, 75, now 46-65.
On the water (first boat mission) 46-53 ms.

The title needs 33 ms. Of a 55 ms frame on the water:

- **The thread that prepares submissions: 48 ms.** It is the wall. Steps of the lists
  18 (refreshes of host copies 10), steps done on the card 7, waiting for descriptor
  records 6, asking the write watch 5, copies in the card's memory 4, the title's bytes
  under shader writes 4.
- **The card: about 41 ms busy**, the next wall.
- **List calls: 36,000 a frame, 230 ms of work** spread over eight threads. In a street
  chase 67,000 calls and 430 ms.
- Descriptor records copied as plain bytes: 400,000 slots looked at, 40,000 mirrored a
  frame, most of them empty.
- Landing what shaders wrote: 16 MB a frame put as dwords, on a thread of its own.

## The one idea behind most of it

On the console the CPU and the graphics processor share memory. Here they do not, so
everything one writes and the other reads must be noticed and moved. Every large cost
is one of four: **noticing** writes of the title (write watch, guards), **noticing**
writes of shaders (hashes on the card, compares on the CPU), **moving** them, and
**waiting** until an order is safe. Gains came from making each of them narrower
(dwords, not pages), wider (threads, AVX2) or later (on the card, behind the frame),
in that order of size:

| What | Frame before | after |
|---|---|---|
| Descriptor records of draws' tables left to the draws | 280 ms | about 124 |
| Our hash passes over card copies, 64 threads a piece | card 79 ms | 66 |
| Preparing on a thread of its own, refreshes and records spread | 103 | 75 |
| List calls written down and translated by eight threads (`VICTORY_THIN`) | 75 | 68 |
| Submissions joined, card steps, textures sent ahead | 68 | about 63 |
| Title bytes under shader writes compared with AVX2 on six threads | 14 ms of it | 4-7 |
| Stores into guarded memory looked at first, fewer guards reset | 69 ms of threads' time | about 6 |

(Each figure is from the run in which it was measured; they overlap and do not add up.)

## Faults found, and what they were

- **Boats sank, the sea plane went under.** The ocean's height and displacement maps are
  textures that compute passes write and the title's physics sample on the CPU. They were
  never brought back to title memory. Fixed by Geckomacabre's read-back as merged in
  fxceless/victory-fork f5c03f3b; on by default here (`VICTORY_TEXTURE_BACK=0`: off).
  With it the boat stays up through a whole run; without it sank in every one.
- **A device loss about every 16 minutes**: a page fault inside a texture made in the same
  tenth of a second. A slot showed it before its upload had run.
  `VICTORY_SHOW_AFTER=1`; none in the sessions since (likely fixed, not proven).
- **A rare lost write** (6 of 200 runs of a stress test): a view was mapped between our
  look at its guards and our lock, and views were opened one after another. None in 603
  runs after.
- **Blocky patches**: indirect dispatches cut at 65,535 groups.
- **Square holes and dashed edges in shadows**: pixel shaders that read their quad's
  neighbours with wave reads; on AMD helper lanes take no part in them.
- **Cube reflections too blurred, wrong materials**: DXIL's `Log` is base 2, and
  normalized buffer codes were one step off.
- **Every ray missed**: the dispatch flag that says counts are threads, not groups.

## Measured and not worth it

- Unordered early pipelines for everything (AMD's compiler crashed).
- Series S profile, smaller textures, a second drawing queue, `ASYNC_SUBMIT` off.
- Mapping pages on sixteen threads: no more pages mapped, frames longer.
- Scalar buffer loads four dwords a call (`VICTORY_SHADER_OPT=127`): no gain on this card.
- Wave sizes, fused multiply-adds, fast reciprocals, raw picked buffers, leaving out
  barriers: nothing to 0.1 ms each. Exact RDNA float rules (`VICTORY_RDNA_FLOAT`): 0.8 ms.
- Running the console's shader code natively on the card: the Windows driver takes no raw
  code, the two generations differ, and a Vulkan host layer ends at the same compiler. What
  is left to gain from shader code was guessed at under 1 ms.
- An on-screen panel of numbers (`VICTORY_PANEL`): it helps finding where a frame goes but
  costs some, so it is off and F3 shows it.

## Pipelines the title has ready-made (device methods 0x1c8 and 0x218), 2026-11-26

From chabrune (84899fde, 03552a5b; 96 descriptions of each method written down with
`VICTORY_PIPELINE_DUMP=<folder>` on an RX 7700 XT) and grimygunnz/nvidia-3070-VI
(58a0c92, 229d582); run fortynine here (RTX 3060) logs descriptions of the same lengths.

**What they are.** Pipelines whose programs the console's compiler made ahead of time,
handed over in pieces instead of a shader container: the pieces of the `FSPX` an `ACCC`
part holds. When a method answers that it is not there, the title makes the same
pipeline from containers through the methods we have. So the warnings these two log
("a ready-made pipeline method that is not implemented") are not missing draws; all of
them come in the first two seconds, while the title prepares its pipelines (run
fortynine: 0x1c8 16,384+ calls, 0x218 2,048+).

**0x218, compute**, 12 words: the root signature object (one of ours); four pieces as
pointer and length: the pipeline's registers (0x1c bytes: thread-group size as three
u16 at +0x0c, flags at +0x12, `COMPUTE_PGM_RSRC1/2` at +0x14/+0x18), a note about the
pipeline (0x14 bytes, a hash first), the GFX10 code, the program's metadata (0x403,
size - 0x14, code dwords, then 32-byte records 0x41/0x42/0x43 from +0x18); then three
words of the title's own (two descriptions it is linked with, a hash).

**0x1c8, graphics**, 13 words: the root signature object, a block of registers (0x17c,
0x184 or 0x18c bytes), a note (0x78 to 0xbc bytes, with the input layout: element
count, then semantic names and formats), then code and metadata of the pixel program
(null for depth-only pipelines) and of the vertex program. The register block holds
`SPI_SHADER_PGM_RSRC1/2` of both stages (+0x38, +0x4c), `CB_COLOR_CONTROL` (+0x7c),
`PA_SU_SC_MODE_CNTL` (+0xa0), `DB_DEPTH_CONTROL` and `DB_STENCIL_CONTROL` (+0xb8,
+0xbc), `CB_BLEND0..7_CONTROL` (+0xd0); the render target and depth formats, the sample
count and the topology are not found yet. Not written.

**State.** 0x218 is written behind `VICTORY_PIPELINE_PARTS` (off by default): `1` makes
every pipeline whose pieces read as a compute program (put into a one-part `ACCC`
container the translator already reads), `2` only those whose program translates at
once, leaving the others to the title's containers. With `1` the title used these
pipelines in place of the containers' and the picture was black (chabrune: 1,065
dispatches a frame dropped; grimygunnz: 821 programs that do not translate, 40 ms
frames): the pieces lack what a container's program record says of root parameters,
and the `FSPX` reader takes only what the upscaler's programs need (no root constants,
for one). Nothing is gained on the picture by either value: 662 of 1,019 such programs
have a container program of the same group size and code length (grimygunnz).

## What is next

1. The preparing thread: split what one submission does over two threads in a row; ask
   for the next submission while this one is sent; view the card's own buffers where fills
   copy them now.
2. The card: small buffers' copies in its own memory (`VICTORY_CARD_BUFFERS_MIN_KB=64`,
   after fxceless c36971b2, wins over `VICTORY_CARD_BUFFERS_MIN_MB`; not measured here
   yet), and the passes that run for nothing
   without rays.
3. Stalls: 270-660 s of a session were full of frames over 200 ms, almost all while new
   pipelines were made. `victory warm`, and a quarter of the workers for the slow ones.
4. Outdoor flicker: suspected to be one bad indirect record that uses up its list's
   budget, so that every later draw of it is empty. The log's frame line counts them;
   `VICTORY_INDIRECT_REST=0` is the experiment.
5. Memory, 40-46 GB private: lists' allocators 9 GB, windows 7, uploads 4
   (`VICTORY_MEMORY_MAP=1` lists it).

## The other repositories, compared

Read up to 2026-11-25: Marvisak/VIctory, fxceless/victory-fork, DexNom/VIctory,
Belobug/VIctory-Belobug-AMD, FQY55kai/ZJOU-VICTORY. What was taken is in the history,
each commit naming its source. In short:

- Marvisak's commits hardly apply any more; nearly all of what they fix is here, often
  another way. Taken: nearest normalized codes, d16 views, shader model flags.
- fxceless: the base. Newest taken: texture read-back, small buffers on the card, hashes
  over a buffer's own bytes.
- DexNom: thread counts of ray dispatches, cube gradients, joined sparse ranges.
- Belobug: pipeline workers for screen passes, the ambient stand-in's idea, pipeline and
  constant buffer bookkeeping (romainhedouin).
- ZJOU: render targets not set again; some shader corrections are being looked at.
