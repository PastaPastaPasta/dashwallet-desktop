# Cost of the `shielded` feature (E0-12)

`dw-engine` has a non-default `shielded` feature. It enables `platform-wallet/shielded` and
`platform-wallet-storage/shielded` (Orchard, Halo 2, the commitment tree). This page records what it costs, so
the decision to ship it in the default build (X2) can be taken on numbers. Platform is pinned at
`bc41f1bc233dec4607d387101c1d9c2f111019b2`; the feature adds 40 packages to `Cargo.lock` (30 compiled crates
on top of 517) among them `halo2_proofs`, `halo2_gadgets`, `orchard` (dashpay fork) and
`grovedb-commitment-tree`.

**Summary.**

| | Dev profile | Release profile |
|---|---|---|
| Clean build, CPU work (user-space instructions) | **+5.9 %** (7.78 → 8.23 × 10¹²) | **+6.6 %** (6.34 → 6.76 × 10¹²) |
| `dwcli` size, unstripped | +1.07 MB (+0.8 %) | +0.40 MB (+0.7 %) |
| `dwcli` size, stripped | +0.35 MB (+0.6 %) | +0.23 MB (+0.5 %) |
| Prover warm-up (cold process) | median 3.2 s | median 3.5 s |

Wall-clock build time **could not be resolved**: with the box at load 15–80, the feature build was as often
faster than the baseline as slower (tables below). Read the instruction counts for the build cost.
The feature costs about 6 % more CPU work and about 30 more crates, and the warm-up is a few seconds, not the
~30 s the upstream docs quote (see "Prover warm-up").

## Machine and method

agentbox: 32 vCPU (Intel Xeon E5-2687W v4 @ 3.00 GHz), shared with other agent threads, load average
15–80 throughout. `uptime` was logged before and after every measurement (raw log kept with the scripts; the
load averages are in the tables). Toolchain `rustc 1.98.1`, `CARGO_BUILD_JOBS=12`, mold linker (from
`~/.cargo/config.toml`).

- **Fresh target dir per build**, inside the worktree and never shared, deleted afterwards.
- **`RUSTC_WRAPPER=` (empty): sccache is bypassed.** The agentbox default is `rustc-wrapper = "sccache"`, which
  would turn a "clean" build into cache hits. Checked with `cargo build -v`: the rustc invocations no longer go
  through `sccache`. (C dependencies still go through ccache, as for any build on this box.)
- Command, from `rust/`, with `/usr/bin/time` for wall, user and sys time:

  ```sh
  cargo build -p dwcli --locked [--release] [--features dw-engine/shielded]
  ```

  `dwcli` is the workspace's only shipped binary that links `dw-engine`. Feature off is the baseline; the
  default feature graph with the change is identical to `main` (`cargo tree --workspace -e features`, compared
  after normalising the checkout path).
- **Order.** Repetitions interleave off and on, and flip the order on every repetition, so a drift in load
  does not land on one variant.
- **Repetitions: 2 per cell**, not 3. Each clean build took 8–10 minutes; at load 50 a third repetition would
  have taken over 90 minutes, so the run was cut after two. The third dev/off build was started and killed;
  its numbers are not used.
- **Instruction counts.** Wall time was too noisy, so one more clean build per cell was run under
  `perf stat -e instructions:u` (user-space instructions of cargo and all its children). `perf_event_paranoid`
  is 2, which allows user-space counting. Instruction counts do not depend on load; one repetition per cell.

## Clean build time

Wall seconds, sorted by repetition; "load" is the 1-minute load average before → after the build.

| Profile | Feature | Rep 1 wall (load) | Rep 2 wall (load) | Median | Min–max | User CPU (median) | Crates compiled |
|---|---|---|---|---|---|---|---|
| dev | off | 535 s (59.5 → 35.5) | 621 s (44.6 → 53.1) | 578 s | 535–621 | 4017 s | 517 |
| dev | on | 508 s (35.5 → 22.3) | 461 s (15.6 → 80.8) | 485 s | 461–508 | 3672 s | 547 |
| release | off | 619 s (60.7 → 48.2) | 511 s (58.6 → 45.0) | 565 s | 511–619 | 3120 s | 517 |
| release | on | 590 s (48.2 → 29.9) | 591 s (53.1 → 55.8) | 591 s | 590–591 | 3198 s | 547 |

With two repetitions the median is the mean of the two, so min–max is the honest spread. The spread within a
cell (up to 90 s) is larger than any difference between cells, and the dev profile shows the feature build
*faster* than the baseline. That is load, not the feature. Peak RSS was 3.0–3.2 GiB in every build
(`maxrss` of the cargo process tree's largest member); the feature adds about 0.1 GiB in release.

Load-independent measure, user-space instructions of one clean build:

| Profile | Off | On | Delta | Wall (load before → after) |
|---|---|---|---|---|
| dev | 7.777 × 10¹² | 8.234 × 10¹² | **+5.9 %** | 633 s (40.6 → 47.9) / 550 s (47.8 → 32.9) |
| release | 6.340 × 10¹² | 6.760 × 10¹² | **+6.6 %** | 416 s (32.9 → 24.3) / 490 s (24.3 → 27.2) |

The dev profile compiles dependencies with `opt-level = 2` (`rust/Cargo.toml`), so dev and release costs are
close; the extra crates are mostly dependency code in both. A cold CI runner builds the whole graph (about
25–60 minutes, `docs/ci.md`); the feature adds about 6 % to that. Warm CI caches are untouched, because only
the nightly builds the feature.

## Binary size

`dwcli`, bytes, identical between repetitions (±800 B). Stripped with `strip` (all symbols).

| Profile | Off | On | Delta | Off, stripped | On, stripped | Delta |
|---|---|---|---|---|---|---|
| dev | 134,991,288 | 136,060,000 | +1,068,712 (+0.8 %) | 54,292,392 | 54,637,448 | +345,056 (+0.6 %) |
| release (thin LTO) | 59,169,120 | 59,568,296 | +399,176 (+0.7 %) | 42,363,184 | 42,593,648 | +230,464 (+0.5 %) |

The delta is small because `dwcli` only reaches the shielded code that `platform-wallet` and the storage
crate call today (the feature only compiles it in; nothing in `dw-engine` calls it yet). Re-measure when X2
wires the shielded flows: LTO keeps what is reachable, so the real delta grows with the code that uses the
pool. The size of the target directory grows by 70–90 MB (dev 2.73 → 2.80 GB; release 2.41 → 2.50 GB).

## Prover warm-up

The prover is `CachedOrchardProver` (`platform_wallet::wallet::shielded::prover`). Its `warm_up()` builds the
Halo 2 proving key once per process (`OnceLock`) and is what the first shielded proof would otherwise pay for;
it needs no network and no wallet. `rust/crates/dw-engine/examples/shielded_warmup.rs` times it:

```sh
cargo build -p dw-engine --locked [--release] --features shielded --example shielded_warmup
target/<debug|release>/examples/shielded_warmup     # prints warm_up_ms=…
```

This measures **prover initialisation, which is the warm-up**; a full first proof needs a funded identity and
a platform connection, so it was not measured. Proof generation after warm-up is a separate cost, not recorded
here.

Each run is a cold process (the cache is per process), 10 runs in all; the second `warm_up()` call in the same
process returns in under a microsecond.

| Profile | Runs | Median | Min–max | Load (1 min) before the runs | User CPU (median) | Peak RSS |
|---|---|---|---|---|---|---|
| dev (deps at opt-level 2) | 4 | 3.23 s | 2.97–3.46 s | 66, 63, 48, 47 | 7.6 s | 37 MB |
| release | 6 | 3.51 s | 2.92–3.78 s | 16, 16, 16, 61, 58, 56 | 8.6 s | 36 MB |

Release is not faster than dev here, probably because the proving key is built in dependency code that the dev
profile already optimises (not checked). User CPU is about 2.5× the wall time, so Halo 2 uses several threads. On a machine with fewer cores the wall time is longer (not measured; a pinned
single-core run was attempted and abandoned). The upstream doc comment says "~30 seconds"; on this box it is
about 3 s. Call `warm_up()` on a background thread at start-up, as the upstream comment says, so the first
shielded operation does not wait.

## Reproducing

The nightly's `shielded` job (`.github/workflows/nightly.yml`, `docs/ci.md`) builds `dwcli` with the feature,
lints `dw-engine` with it and runs the warm-up example on every run, so the feature stays green; it does not
record timings. To repeat the measurement here, use the commands above with a fresh `CARGO_TARGET_DIR` and
`RUSTC_WRAPPER=` set empty, and record `uptime` with each run.
