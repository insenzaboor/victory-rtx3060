# Third-party code

VIctory is licensed under GPL-3.0-or-later (`LICENSE`). Code taken from other
projects is listed here, with its license and where it is. Each such file
names its origin (repository and commit) in its header. Commits taken whole
from other forks of VIctory name the commit and its author in their message
and are not repeated here.

| Source | License | What | Where |
|---|---|---|---|
| [XCL](https://github.com/xcl-dev/xcl), `crates/xcl/src/hle/d3d12x/uma_driver.rs` at commit `06f90c198` (as written in `f2248f068` by Kalmarv and `f78e05bcc` by Prime1997) | GPL-3.0-or-later (from `06f90c198`, "Docs: relicense XCL to GPL-3.0-or-later"; GPL-2.0-only before it, from which nothing was taken). Copyright the XCL contributors. | Memory remade as a driver `CUSTOM` write-back L0 heap mapped at its own address: the snapshot of written pages, the release, the grooming of free address space so the driver's mapping lands in the hole, the write-watched heap, the restore; and its tests (the failed write watch, the failed restore, the grooming for a window under 1 GiB) | `crates/victory-gpu/src/uma_driver.rs` (`VICTORY_UMA_DRIVER`) |

The license was checked at `06f90c198`: its `LICENSE` is the GPL version 3
text, its `Cargo.toml` says `GPL-3.0-or-later`, and the ported file carries
no header of its own (no other notice to keep).
