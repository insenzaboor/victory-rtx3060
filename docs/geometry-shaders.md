# Geometry shaders: the voxelization pass (2026-11-26)

The 195 containers that fail with "a container without a vertex, pixel, compute or mesh
program" are all `GS_VoxelizeGeneric_*`. What they hold, how often the title draws with them,
and what translating them takes. Nothing is translated yet: the measurements say the pass
never runs, so this is the plan for when it does.

## What they are

Read offline from `shaders.rpf` (every record unpacked, its `DXIL` part's native programs
listed):

| Records | Programs | Pipeline role |
|---|---|---|
| 195 `GS_VoxelizeGeneric_*` | one `Gs` each (wave64, 1,024 bytes of code, 24 or 25 VGPRs) | geometry |
| 195 `VS_VoxelizeGeneric_*` | an `Es` and a `Vs` each (two `XBCH` blobs) | vertex |
| 301 `PS_VoxelizeVisiblity_*` | one `Ps` | pixel (writes the volumes through UAVs) |
| `voxelization:CS_*` (7), `EntityCoarseVoxelization:CS_*` (1) | `Cs` | clears, jump flood, coarse pass |

No other record in the file has an LS, HS, ES or GS program. The bitcode is stripped (a stub),
so only the GFX10 code says what they do.

The GS is the **legacy (not NGG) half of a merged ES+GS stage**, with both rings in LDS and no
`exp`: a copy shader of the console driver, not in the pack, exports what it leaves in LDS.

- Prologue: EXEC from s3 bits 8-15 (one lane a primitive), M0 from s3 bits 16-23 (the GS
  wave id), the GSVS ring base a dword loaded from user data 0-1 (a driver table, `+0xb8`,
  no root parameter holds it) shifted left 8, plus s2.
- Inputs (ESGS ring): vertex k's item at the dword offset in v0 bits 0-15, v0 bits 16-31,
  v1 bits 0-15 (`(v0 << 2) & 0x3fffc`, an SDWA `WORD_1` select). Item dword 4r+c is row r,
  column c of `ISG1`: it reads `SV_Position.xyz` (dwords 0-2) and `TEXCOORD0.xy` (4-5).
- Work: the face normal (cross product, `v_rsq`), its dominant axis (0-5), the triangle
  swizzled so that axis is depth, scaled by a constant-buffer vector (table at user data 8,
  offset 584 dwords, `b6`, bytes 0x40-0x4b) times 1/2^n (n the low 5 bits of the root
  constant `b502`, user data 11), clamped to [-1, 1], z mapped to [0, 1], w = 1.
- Outputs (GSVS ring): vertex n at lane x 240 bytes + n x 80 bytes + 4(4r+c), r and c of
  `OSG1`: position, a uint (the axis), a float4 (-1, -1, 1, 1), a float2 (the texcoord), a
  float3 (the normal), exactly the five `OSG1` rows, which are the five `ISG1` rows of
  `PS_VoxelizeVisiblity`.
- Messages: `GS EMIT` x3, `GS CUT`, `GS_DONE`. `PSV0`: triangle in, triangle strip out, stream
  mask 1, at most 3 vertices. No instancing, no primitive id, no streams 1-3, no stream-out.
- GS stage data (offsets from E = record + 0x54, checked on `GS_VoxelizeGeneric_b300ca2e`):
  E+0x27c GSVS item in dwords (60), E+0x280 1, E+0x284 most vertices (3), E+0x28c vertex in
  dwords (20); the four non-position outputs in 6-byte entries (usage, row, parameter...) at
  E+0x150, their count at E+0x210.

The ES half computes the same thing as the `Vs` program beside it (position transformed,
texcoord), writing a 32-byte item to LDS at lane x 32 + (s3 bits 24-27) << 11 instead of
`exp pos0` / `exp param0`, and ending in `s_barrier` and `s_setpc_b64` into the GS half.

## How often the title draws with them

Never, so far. In `run-fortynine-20261126-135135` (5,300 frames) 3,435 shaders were bound
first (1,050 compute, 773 mesh, 1,091 pixel, 521 vertex), none of them a geometry stage,
and the drop reasons per frame were "being made", the structure builder's stages and two
compute programs; no "refused" pipeline. A pipeline with a geometry stage would show as
`first bind ... stage="geometry"`, and its draws as "refused [vertex ..., pixel ...,
geometry ...]" in the drop report (`HostPipeline::new` refuses it before translating
anything). Of the voxelization family only `CS_EntityCoarseVoxelization` was bound (frame
1,579). Over all 62 run logs (`logs/*.log.err`, 3,449 distinct shaders bound first) the
same: no geometry stage, no `VS_`/`GS_VoxelizeGeneric` or `PS_VoxelizeVisiblity`, no
`voxelization:CS_*` clear or jump-flood program, only `CS_EntityCoarseVoxelization`. The pass
is not running at all; its draws are not being dropped. Dark interiors have another cause.

## Other projects

None of the forks we track (avatarmanz, fxceless, deynes, protonida, spook, ruben, chabrune,
belobug, neptune, upstream, th3, xavi, coolfox) translates a GS or an ES. Neptune has a fix
for `PS_VoxelizeVisiblity` (a lane mask parked in two lanes of a VGPR, `with_shared_slots`);
ours translates that pixel shader without it, which may need checking once the pass runs.

Outside them, MiamiGame/XTP (GPL-3.0, derived from this code base) translates all 390
halves: `docs/analysis/es-gs.md` and `crates/xtp-shader/src/emit/lower/geometry.rs` (79 KB),
with WARP stream-out tests. victory-scarlett's audit also lists the 195 as never bound.
ProjectReCity (Vulkan) maps them the same way.

## Design

A D3D12 geometry shader for the GS, between the translated **`Vs`** program of the vertex
container and the pixel shader. The plain `Vs` already translates and computes what the ES
writes into the ring, by the same `OSG1` rows, so the ES half need not be translated (XTP
translates it; that is a third of its work). The input assembler stays as it is.

1. `native.rs`: read the GS stage data above into a `StageIo::Gs` (most vertices, vertex
   dwords, item dwords, outputs), cross-checked with `PSV0` and `OSG1`; refuse what does not
   agree.
2. Ring plans (new, after XTP's `geometry.rs`): evaluate every LDS address of the GS as
   constant + coefficients x bit fields of v0, v1, the lane (`v_mbcnt`) and uniform scalars.
   An ESGS read is 4 x an offset half + a constant: `loadInput(row, col, vertex)`. A GSVS
   write is stride x lane + a shared uniform base + a constant below the stride: vertex =
   constant / vertex bytes, row and column from the rest. Emits are numbered by the emits
   before them (the same on every path). Refuse: an LDS access the rings do not explain, a
   write after its vertex's emit or twice, an emit that depends on the path, another stream.
3. Lowering (`emit/lower.rs`, GS arm only): skip the set-up the plans list (the EXEC
   prologue from s3, M0, the ring-base load and its arithmetic); keep GSVS writes in values
   until their vertex's emit, then `storeOutput` of each and `emitStream(0)`; `GS CUT` is
   `cutStream(0)`; `GS_DONE` and the barrier nothing. One host invocation is one lane, as for
   the vertex and pixel programs.
4. Resources: `resource::track` must not see the ring-base load (user data 0-1, which no root
   parameter holds): a parameter listing the sites to leave out.
5. `emit/shader.rs`: the geometry kind exists for the rectangle stage (`Geometry`,
   `emitStream`, `cutStream`, `PSV0`); it needs the console's `ISG1`/`OSG1` and the most
   vertices from `PSV0`. `translate` takes a `Gs` program when a container has nothing else.
6. Host (`victory-gpu`): `HostPipeline::new` takes `vertex + geometry + pixel` (not beside a
   mesh stage or the rectangle stage); the geometry stage's root constants and tables with
   `D3D12_SHADER_VISIBILITY_GEOMETRY`; `pipeline.rs`'s `translate` takes the `Gs` program's
   used resources. The draw path binds per stage already.
7. All of it behind a switch, off by default, kept in the shader cache key while on.

Estimate: about 2,000 lines with tests, three to four days: two for the plans and the
lowering (porting XTP's, adapted to our single `lower.rs`), one for the stage data, the
resource tracker and the emitter, one for the host pipeline and WARP stream-out tests
comparing every output dword with what the console program leaves in its ring. Then a game
session that reaches the voxelization pass, which none has so far: that is the reason it is
not done now.
