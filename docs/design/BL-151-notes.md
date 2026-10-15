# BL-151 notes: rustls advisories

Status: **stopped after step 3** (lockfile bump done, CI step not added), see D-3.

| Row | Decision |
|---|---|
| D-1 | `cargo update -p rustls` only reached 0.23.43 (0.23.46 needs rustls-webpki >= 0.103.15, which `-p rustls` does not pull). Used `--precise 0.23.46`; this also moved rustls-webpki 0.103.13 -> 0.103.15. No other lock entry changed, no Cargo.toml edit. |
| D-2 | After the bump, `cargo deny check advisories` (0.20.2) reports no rustls advisory. Three others remain: RUSTSEC-2026-0204 (crossbeam-epoch 0.9.18, via rayon in dash-spv), RUSTSEC-2026-0258 (h2 0.4.15, via tonic/hyper in dash-sdk), RUSTSEC-2024-0370 (proc-macro-error 1.0.4, build-time proc macro under the Linux gtk stack, no upgrade exists). |
| D-3 | crossbeam-epoch and h2 are linked into the runtime binaries, so no `deny.toml` ignore was added (task rule). Both have lockfile-only fixes (`cargo update -p crossbeam-epoch` -> 0.9.21, `-p h2` -> 0.4.20, dry-run checked) but those are outside this row's "rustls and direct companions" scope. The `cargo deny` CI step was not added, because it would fail on the first run. Manager decides: widen BL-151 to those two bumps, then add the step with an ignore for RUSTSEC-2024-0370 only (build-time, no fix). |
