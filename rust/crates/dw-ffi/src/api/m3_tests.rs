//! The M3 contract surface through the FFI (docs/contracts/m3-engine.md):
//! every stub checks its arguments and the session first, then fails with
//! its domain's typed `NotImplemented` naming the call; the constant calls
//! answer; the error codes match §4. Governance and masternode list/ProTx
//! calls are parked with Dash Core's scope (branches m3/r2-governance,
//! m3/r3-protx).

use std::sync::Arc;

use crate::{
    CoinJoinError, CoinJoinSettings, DashNetwork, Engine, EngineConfig, EngineEvent,
    EngineObserver, MasternodeError, MasternodeKeyRole, MixedCoinsDestination, NetworkSession,
    SessionOptions,
};

const WALLET: &str = "abababababababababababababababababababababababababababababababab";
const HASH: &str = "0101010101010101010101010101010101010101010101010101010101010101";
const COMMON: [&str; 6] = [
    "invalid_argument",
    "network_not_open",
    "wallet_not_found",
    "storage",
    "not_implemented",
    "internal",
];

struct Null;
impl EngineObserver for Null {
    fn on_event(&self, _: EngineEvent) {}
}

struct Fixture {
    _dir: tempfile::TempDir,
    engine: Arc<Engine>,
    session: Arc<NetworkSession>,
    rt: tokio::runtime::Runtime,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(
        EngineConfig {
            data_root: dir.path().to_string_lossy().into_owned(),
            worker_threads: Some(2),
        },
        Arc::new(Null),
    )
    .unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let session = rt
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
            },
        ))
        .unwrap();
    Fixture {
        _dir: dir,
        engine,
        session,
        rt,
    }
}

/// The call failed with `code`, or with `not_implemented` naming `call`.
macro_rules! assert_code {
    ($result:expr, not_implemented: $call:expr) => {{
        match $result {
            Ok(_) => panic!("expected not_implemented {}, got Ok", $call),
            Err(e) => {
                assert_eq!(e.code(), "not_implemented", "{e}");
                assert!(e.to_string().ends_with($call), "{e} should name {}", $call);
            }
        }
    }};
    ($result:expr, $code:expr) => {{
        match $result {
            Ok(_) => panic!("expected {}, got Ok", $code),
            Err(e) => assert_eq!(e.code(), $code, "{e}"),
        }
    }};
}

#[test]
fn constants_answer_without_a_session() {
    let limits = crate::coinjoin_limits();
    assert_eq!(limits.min_mixing_balance, 140_001);
    assert_eq!(limits.denominations.len(), 5);
    assert_eq!(limits.defaults.rounds, 4);
    assert_eq!(limits.defaults.target_amount_dash, 1_000);
    assert!(limits.defaults.enabled);
}

#[test]
fn test_qt_046_coinjoin_calls_answer_and_check_their_wallet() {
    let f = fixture();
    let s = &f.session;
    let rt = &f.rt;
    // Options: dash-qt defaults until changed, validated, applied live.
    assert_eq!(
        s.coinjoin_settings().unwrap(),
        crate::coinjoin_limits().defaults
    );
    let bad = CoinJoinSettings {
        rounds: 1,
        ..crate::coinjoin_limits().defaults
    };
    assert_code!(
        rt.block_on(s.set_coinjoin_settings(bad)),
        "invalid_argument"
    );
    let changed = CoinJoinSettings {
        rounds: 8,
        multi_session: true,
        ..crate::coinjoin_limits().defaults
    };
    rt.block_on(s.set_coinjoin_settings(changed)).unwrap();
    assert_eq!(s.coinjoin_settings().unwrap(), changed);

    // Per-wallet calls parse the id, then need the wallet.
    assert_code!(s.coinjoin_status("zz".into()), "invalid_argument");
    assert_code!(s.coinjoin_status(WALLET.into()), "wallet_not_found");
    assert_code!(
        rt.block_on(s.start_mixing(WALLET.into())),
        "wallet_not_found"
    );
    assert_code!(
        rt.block_on(s.stop_mixing(WALLET.into())),
        "wallet_not_found"
    );
    assert_code!(
        rt.block_on(s.set_coinjoin_salt(WALLET.into(), "AB".repeat(32))),
        "invalid_argument"
    );
    assert_code!(
        rt.block_on(s.set_coinjoin_salt(WALLET.into(), HASH.into())),
        "wallet_not_found"
    );
    assert_code!(
        rt.block_on(s.coinjoin_salt(WALLET.into())),
        "wallet_not_found"
    );
    assert_code!(
        rt.block_on(s.generate_coinjoin_salt(WALLET.into())),
        "wallet_not_found"
    );
    // The recovery scan needs a running SPV client.
    assert_code!(
        rt.block_on(s.coinjoin_recovery_scan(WALLET.into())),
        "coinjoin.spv_not_running"
    );
    assert_code!(
        rt.block_on(s.mixed_coins_sweep_plan(WALLET.into(), MixedCoinsDestination::Wallet)),
        "wallet_not_found"
    );
    // The shielded destination is M4 work: its own call name.
    assert_code!(
        rt.block_on(s.mixed_coins_sweep_plan(WALLET.into(), MixedCoinsDestination::Shielded)),
        not_implemented: "NetworkSession.mixed_coins_sweep_plan.shielded"
    );
    assert_code!(
        rt.block_on(s.move_mixed_coins(WALLET.into(), MixedCoinsDestination::Shielded, "g".into())),
        not_implemented: "NetworkSession.move_mixed_coins.shielded"
    );

    // QT-144 Network sub-tab: full-node values are absent, never zero.
    let stats = s.network_stats().unwrap();
    assert!(stats.credit_pool.is_none() && stats.instantsend.is_none());
    assert!(stats.masternodes.is_none() && stats.best_chainlock.is_none());
}

#[test]
fn keychain_calls_check_their_arguments() {
    let f = fixture();
    let s = &f.session;
    let rt = &f.rt;
    assert_code!(
        rt.block_on(s.masternode_keys(WALLET.into(), MasternodeKeyRole::Operator, 0, 101)),
        "invalid_argument"
    );
    assert_code!(
        rt.block_on(s.masternode_keys("zz".into(), MasternodeKeyRole::Owner, 0, 10)),
        "invalid_argument"
    );
    let vault = s.vault();
    assert_code!(
        rt.block_on(vault.reveal_masternode_key(
            "zz".into(),
            MasternodeKeyRole::Owner,
            0,
            "g".into()
        )),
        "invalid_argument"
    );
}

#[test]
fn m3_stubs_report_a_closed_session() {
    let f = fixture();
    assert!(
        f.rt.block_on(f.engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
    assert_code!(f.session.coinjoin_status(WALLET.into()), "network_not_open");
    assert_code!(f.session.network_stats(), "network_not_open");
}

/// The domain codes of `codes` (common codes dropped), sorted.
fn domain(codes: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut v: Vec<String> = codes
        .into_iter()
        .filter(|c| !COMMON.contains(&c.as_str()))
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// The codes listed in the §4 row of m3-engine.md whose first cell is
/// `cell`, sorted.
fn contract_row(cell: &str) -> Vec<String> {
    let contract = include_str!("../../../../../docs/contracts/m3-engine.md");
    let prefix = format!("| {cell} |");
    let row = contract
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no {cell} row in m3-engine.md §4"));
    let mut listed: Vec<String> = row
        .split('`')
        .skip(3)
        .step_by(2)
        .map(String::from)
        .collect();
    listed.sort_unstable();
    listed
}

#[test]
fn m3_error_codes_match_the_contract() {
    let d = String::new;

    let coinjoin = domain(
        [
            CoinJoinError::Disabled,
            CoinJoinError::WatchOnly,
            CoinJoinError::InsufficientFunds { min_duffs: 0 },
            CoinJoinError::VaultLocked,
            CoinJoinError::GrantInvalid,
            CoinJoinError::NothingToMove,
            CoinJoinError::SpvNotRunning,
            CoinJoinError::NoPeers,
            CoinJoinError::BroadcastRejected { reason: d() },
        ]
        .iter()
        .map(CoinJoinError::code),
    );
    assert_eq!(coinjoin, contract_row("`CoinJoinError`"));

    let masternode = domain(
        [
            MasternodeError::WatchOnly,
            MasternodeError::VaultLocked,
            MasternodeError::GrantInvalid,
        ]
        .iter()
        .map(MasternodeError::code),
    );
    assert_eq!(masternode, contract_row("`MasternodeError`"));
}

/// Engine failures reach the host with their M3 codes.
#[test]
fn engine_failures_map_to_their_codes() {
    use dw_engine::{CoinJoinFailure, EngineError, MasternodeFailure};
    let cj: CoinJoinError =
        EngineError::from(CoinJoinFailure::InsufficientFunds { min_duffs: 140_001 }).into();
    assert_eq!(cj.code(), "coinjoin.insufficient_funds");
    let mn: MasternodeError = EngineError::from(MasternodeFailure::WatchOnly).into();
    assert_eq!(mn.code(), "masternode.watch_only");
    let locked: MasternodeError = EngineError::Vault(dw_vault::VaultError::Locked).into();
    assert_eq!(locked.code(), "masternode.vault_locked");
}
