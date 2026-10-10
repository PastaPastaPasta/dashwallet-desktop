# E0-10a trust spike probes

Research tooling for `docs/research/dashpay/trust-spike.md`. Not part of the
`rust/` workspace and not shipped.

- `pin/`: built at platform's pin (`bc41f1bc23`, rust-dashcore `40268cc0`;
  `Cargo.lock` seeded from `rust/Cargo.lock`).
  - `capture`: read-only DAPI fetches (the DPNS and DashPay contracts, and
    with `--identity` an identity), one evonode per query, through a
    `ContextProvider` that forwards to the trusted quorum service and logs
    every (quorum type, quorum hash, core chain-locked height) the SDK asks
    for during proof verification.
  - `spv`: the dash-spv probe at the pin.
- `dev/`: the same dash-spv probe at rust-dashcore `dev` (`c19973ab`).
- `common/spv_probe.rs`: the probe both crates build. It runs a dash-spv
  client with masternode sync on (as `dw-engine` `Session::spv_config` does;
  filters off unless `--filters`), tails the capture log and asks
  `get_quorum_at_height` for every tuple, as platform-wallet's
  `SpvRuntime::get_quorum_public_key` does.
- `analysis/`: `heights.py` resolves each quorum's base-block height (testnet:
  the read-only oracle's `getblockheader`; mainnet: the public Insight API);
  `analyze.py` summarises a run directory; `summary.py` adds the post-day,
  engine-status and RSS figures and writes the JSON committed in
  `docs/research/dashpay/trust-spike-data/`.

```bash
B=/work/scratch/e0-10a; R=$B/run1
(cd pin && CARGO_TARGET_DIR=$B/target-pin cargo build --release)
(cd dev && CARGO_TARGET_DIR=$B/target-dev cargo build --release)
$B/target-pin/release/trust-spike-pin capture --network testnet \
  --out $R/tuples-testnet.jsonl --rounds 780 --interval-secs 120 --identity <base58 id>
$B/target-pin/release/trust-spike-pin spv --network testnet --dir $R/spv-pin-testnet \
  --tuples $R/tuples-testnet.jsonl --out $R/lookups-pin-testnet.jsonl --duration-secs 90000
$B/target-dev/release/trust-spike-dev spv --network testnet --dir $R/spv-dev-testnet \
  --tuples $R/tuples-testnet.jsonl --out $R/lookups-dev-testnet.jsonl --duration-secs 90000
python3 analysis/heights.py testnet $R/tuples-testnet.jsonl $R/heights-testnet.txt
python3 analysis/analyze.py $R --heights testnet=$R/heights-testnet.txt
# summary.py needs both networks' runs, the post-day runs and $R/rss.txt (`<epoch> <unit> <KiB>` lines)
python3 analysis/summary.py $R ../../docs/research/dashpay/trust-spike-data/summary.json
```

Everything is read-only: DAPI queries, the trusted quorum service, public
P2P peers for SPV (never the local oracles: dash-spv sends `getqrinfo` and
`getmnlistdiff`, which are not pure reads there), and pure `getblockheader`
calls on the testnet oracle.
