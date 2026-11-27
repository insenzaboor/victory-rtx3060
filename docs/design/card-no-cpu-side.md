# Card copies without a CPU side

Status: stage 1 built, behind `VICTORY_CARD_NO_CPU_SIDE=1`, off by default. Not
yet run in the game. Stage 2 (a reference per piece instead of per copy) is a
plan.

## Why

The game process uses 22 to 25 GB of private memory on a 32 GB PC, and the
user's other jobs spike 5 to 8 GB beside it. The memory line of run q15
(`run-q15-layout-probe-20261127-122309.log.err`, 17:33:04) reads:

    copies of windows (CPU side) 1878, shared heaps unused 1849, our heap 1761,
    copies of title buffers (CPU side) 790, ...;
    of the copies 1624 MB have a card side too (buffers 909, windows 714)

A copy in the card's memory (`shadow::cards`) has two sides. Shaders use the
card side. The CPU side (`Card::side`, the buffer `Shadow::data` maps) is a
shared buffer in the PC's memory of the same size. So the same bytes are held
three times: in title memory, in the CPU side, and on the card. Of the copies'
2.67 GB CPU sides, 1.62 GB belong to copies that also have a card side. Those
1.62 GB are what this switch is about.

## What the CPU side is used for

Reading `shadow.rs`, `pool.rs`, `land.rs`, `watch.rs`, `host.rs` (`card_before`,
`card_back`) and `host/send.rs`:

| Use | Where | Needs the CPU side because |
|---|---|---|
| Refresh | `Shadow::update` → `copy_stale` → `take`/`copy` | Title pages written since they were last taken are copied into it, and the range is noted in `Card::dirty`. |
| Upload | `card_dirty`, then `send::take` (`VICTORY_SEND_OWN`, on by default) | The dirty ranges are copied out of it into the submission's own upload memory, and the card copies them from there before the submission's lists run. |
| Dword sends | `take` with `VICTORY_SEND_CHANGED=1` or `changes_only` (arenas by address, `keep_stores`) | Only dwords that differ from it go up. |
| Reference for shader writes | `card_merge`, `card_merge_relay`, `land_card` (`land::piece`) | After the card's hash compare flags a 64 KiB piece, the fetched bytes are compared with it byte by byte. Bytes that differ are the shaders', and the title's bytes stay where the card did not write. |
| Title bytes under flights | `merge_under_writes` | Title changes inside ranges a flight may write are found by comparing title memory with it. |
| Copies on the card | `placed_copy` | The bytes a list copies into the copy are put into it, so that the landing does not take them for a shader's write. |
| Landing echo | `take_landed` | A page that only a landing wrote is compared with it and not sent again. |
| Diagnostics | `refresh_check`, `stale_pages`, `upcheck` | They compare it with title memory or with the card. |
| Eviction | `Drop`, `let_go_idle`, the pool's reclaim | It is released with the copy. Nothing reads it then. |

A copy that no shader is declared to write uses only the first two rows. For
that copy the CPU side is just a staging area between title memory and the
upload memory. Every other row needs a copy with declared writes (`pending`,
`before` hashes, landings), or a switch, or the arenas by address.

## What the switch does (stage 1)

- **Made without a CPU side.** When `lookup` (title buffers) or `loose`
  (windows) puts a copy in the card's memory, no shared buffer is made for it.
  `Shadow::data` is null and `Card::side` is empty. Copies that do not go to
  the card, and the arenas by address, are made as before.
- **Uploads.** A refresh of such a copy notes the range in `Card::dirty` and
  copies nothing (`Shadow::copy`). When `Host::card_before` takes the
  submission's sends, `send::take` reads those ranges from title memory itself,
  through `copy_mapped` (`shadow::title_run`), into the submission's upload
  memory. Pages that are not there read as zeros, and bytes past the title
  buffer, up to the 64 KiB rounding, are zeros. That is what the CPU side held
  there. Short runs go as word pairs, as before. The upload memory is reused
  between submissions as a ring. `VICTORY_CARD_UPLOAD_RING_MB` (128 unless
  set) is how much of it is kept, in place of `VICTORY_SEND_SPARE_MB`'s
  default of 768. An explicit `VICTORY_SEND_SPARE_MB` still wins.
- **Shader writes.** The first time a submission declares a write to such a
  copy, `card_before` gives it its CPU side (`Shadow::card_side_now`). This
  also happens for ranges `crate::picked` looks at, and for copies the
  structure builder may store in by address (`keep_stores` →
  `side_wanted`). It all runs on the submitting thread, before the hashes and
  before the submission's lists. The new CPU side is filled from title memory
  as it is now, with zeros past the buffer. The whole copy is put in that
  submission's sends, and with the relay it is noted in `sent` under that
  send's number. From then on the copy is an ordinary copy with two sides, and
  every row of the table works as it does today.
- **The reference.** For each option the task named:
  - *Reference only for copies with GPU writes.* This is what stage 1 builds.
    It is coarse: a whole copy is either read-only or keeps a full CPU side.
  - *Title memory at the time of the last upload.* That is the CPU side
    itself. Nothing else holds those bytes once title memory moves on, so the
    switch cannot replace the reference with it.
  - *The card's dirty tracking (`CARD_DIRTY_LIST`, `card_dirty`).* This says
    which ranges the CPU sent up. It does not say which bytes the GPU wrote,
    so it cannot be the reference. It only decides which ranges to read from
    title memory.
- **The readback ring.** Copies with declared writes keep today's path: the
  hash compare on the card, then fetches of flagged pieces into readback
  memory (`card_back`, the relay's fetches), then a merge against the CPU
  side. Stage 1 adds no new readback path.

### Why it is correct

For a copy without declared writes, the card side must hold title memory as
the refreshes took it. Today the bytes go title → CPU side (at the refresh) →
upload memory (at `card_before`) → card. With the switch they go title →
upload memory (at `card_before`) → card. Both steps run while one submission
is being prepared, under the submission lock, and the refresh's step comes
first. So the bytes sent are the ones the refresh took, or newer ones the
title wrote after the write watch was asked. Newer bytes are harmless: the
write watch numbers that write after the asking, so the next refresh takes
and sends the page again, the same as today. The sends still go on the queue
after every earlier submission and before this one's lists, so nothing in
flight sees them change. The buffer copies we do on the CPU (`send_copied`)
write title memory first and then note the range, so they are read the same
way.

A copy given its CPU side later starts out with the same sides as a copy that
had one from the start. The CPU side holds title memory and the card side
receives all of it before any declared write runs. That is the state `carded`
already makes for a card buffer placed where another lay. Each later refresh,
merge and landing then runs on a real CPU side.

`merge_under_writes`, `card_merge`, `card_merge_relay`, `land_card`,
`placed_copy`, `take_landed`, `card_held` and the diagnostics each check for a
missing CPU side. They can only see one if making it failed (no memory). In
that case `merge_under_writes` asks for the flights to be settled, the merges
land nothing, and one warning says so. Nothing reads through a null pointer.

The switch turns itself down, with a line saying why, if
`VICTORY_SEND_OWN=0`, `VICTORY_SEND_CHANGED=1`, `VICTORY_PICKED_WRITES` or
`VICTORY_UPCOPY_CHECK=1` is set. All of them compare against, or copy from,
the CPU side of copies that nobody declared written.

With the switch off nothing changes. The buffers are made in the same order
as before. `Card::side` is set when the copy is made. Every path takes the
branch it took before. The upload memory kept stays at 768 MiB.

## How much it saves

Only copies that have a card side are covered. In q15 those hold **1,623 MB**
of CPU sides: buffers 909 MB (64 KiB-rounded staging bytes, which the line
counts as `size`) and windows 714 MB. The arenas by address (about 80 MB:
1,704 MB of card copies less 909 and 714) keep their CPU sides. The other
1,164 MB of window copies have one side only. The card reads those over the
bus, so this switch cannot drop them.

Of the 1,623 MB, what stays is the CPU sides of copies that shaders are
declared to write:

- Windows: run 91 measured "0.0 MB compared in 0 ranges of windows". Expect
  close to all 714 MB to be dropped.
- Title buffers: q15 does not log which copies are written. Its card line
  shows 12,400 pieces of 64 KiB compared per frame, and the hashes are under
  1 MB (`their bytes from before 0 MB`), which bounds nothing useful. The
  174 MB world heap at `0x3fa0000` is written. If the written set is that
  heap plus the other copies of 20 to 50 MB (about 175 MB more), about
  550 MB of 909 is dropped. If it is the heap alone, about 735 MB.

**Expected saving: about 1.25 to 1.45 GB of private memory** (about half of
the 2.67 GB the q15 line shows for copies' CPU sides). The new memory line
(below) measures it. Copies built in the world-load burst never place their
CPU sides in the shared heaps, so "shared heaps unused" should not grow from
them either.

## Risks

- **A shader that writes a copy without a declared range.** The design
  already assumes this does not happen (`cards`, `keeps`): such a write stays
  on the card until a declared range lands its piece. With the switch, if
  such a write lands on a copy before its first declared write, the whole-copy
  send made when its CPU side is created overwrites it. Before the switch it
  survived until a refresh of that range. `VICTORY_PICKED_WRITES`, which hunts
  for these writes, turns the switch down.
- **Spikes when a CPU side is made.** For a written copy, the first declared
  write allocates its CPU side (as before, only later) and sends it up whole.
  The 174 MB heap takes about 256 MiB of upload memory for that one
  submission. This happens once, at world load. The ring is not strictly
  fixed: a submission that sends more than the ring holds still takes more
  memory for as long as it runs. A strictly fixed ring would split one
  submission's sends over waits.
- **CPU time.** Reads from title memory replace the memcpy from the CPU side,
  and the refresh no longer copies at all, so the submitting thread should do
  less. Reads go through `copy_mapped`, one per dirty run, joined as
  `card_dirty` joins them. Short runs read title memory once per run.
- **Diagnostics are blind to sideless copies.** `VICTORY_REFRESH_CHECK` and
  `stale_pages` skip them. `VICTORY_UPCOPY_CHECK` turns the switch down.
- **Not run.** Only unit tests and `cargo check` back this so far. A game run
  with the switch, compared with one without, is the test.

## What to look for in a game run

With the card's line (`host copies in the card's memory: ...`, every 30 s in q15):

    host copies in the card's memory without a CPU side (VICTORY_CARD_NO_CPU_SIDE=1):
    N of them, M MB of CPU sides not held; K given their CPU side later for shader
    writes (S MB); since the start ... MB sent up from title memory in ... runs;
    upload ring kept 128 MB

Every 30 s with the memory line (diagnostics builds), after
`memory, ours by kind`. Its "copies of ... (CPU side)" kinds now leave out the
bytes not held:

    memory, card copies without a CPU side (VICTORY_CARD_NO_CPU_SIDE=1): M MB of CPU
    sides not held, N copies (title buffers ... in ... MB, windows ... in ... MB); K
    copies given their CPU side later for shader writes (S MB)

At start: `VICTORY_CARD_NO_CPU_SIDE=1: copies in the card's memory have no CPU
side ...`, or `... is turned down: <why>`. The first 16 copies given a CPU side,
then every power of two, are logged as `a copy in the card's memory is given its
CPU side for shader writes`.

## Stage 2 (plan)

Keep a reference per 64 KiB piece instead of per copy: the pieces that
declared ranges hold, made when a range is first declared, from title memory
and sent up as stage 1 does for the whole copy. The 174 MB heap's declared
union decides whether this is worth doing; stage 1's "given their CPU side
later" bytes are the most it could save.
