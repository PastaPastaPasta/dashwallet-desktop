# Desktop backlog rows added from branches

The repository had no backlog file when this one was created (BL-151, D-13), so rows opened by a branch
are kept here until they are moved into the manager's tracker. The `BL-` number is assigned there.
BL-156 is tracked in the node-program BACKLOG (same test, first seen 10-15 as a 1-in-40 Linux race).

| Row | Title | Detail |
|---|---|---|
| BL-156 | macOS-only timing failure in `dw-engine` lease test `r1_f2_an_artifact_seen_sent_during_its_resolution_keeps_its_marker` | `cargo test --workspace` failed on the macOS job of [CI run 38687350378](https://github.com/PastaPastaPasta/dashwallet-desktop/actions/runs/38687350378) (`main` at 43992da6, 2026-10-15): `platform::lease::tests::r1_f2_an_artifact_seen_sent_during_its_resolution_keeps_its_marker` panicked at `crates/dw-engine/src/platform/lease/tests.rs:2979` (`assert!(j.load().1[0].sent)`, after `until(\|\| j.load().1.len() == 1)`), 918 passed and 1 failed in the `dw-engine` lib, which took 1419 s on that runner. The same run's Rust tests (Linux) and Windows (engine) jobs passed. The test stalls the journal for 300 ms of wall clock (`j.stall_ms.store(300, ...)`) on a two-worker `#[tokio::test]` and polls with `until`, so a starved macOS runner can see the Sent row late. Make the test deterministic (a gate or notify instead of a sleep and poll), or give `until` a budget that scales with the runner, and rerun it under load on macOS. Not caused by BL-151: the lockfile bumps there passed the same job on b5435f56. |
