//! The M2 contract surface through the FFI (docs/contracts/m2-engine.md):
//! every stub checks its arguments and the session first, then fails with
//! its domain's typed `NotImplemented`; the error codes match §4.

use std::sync::Arc;

use crate::{
    BackupError, CompatError, ConsoleError, CoreExportFormat, DashNetwork, DesktopError, Engine,
    EngineConfig, EngineEvent, EngineObserver, FeeMode, GrantPurpose, HistoryFilter, HistorySort,
    ImportOptions, KeyMaterial, NetworkSession, OutPoint, PsbtError, SendError, SessionOptions,
    SyncError, TxActionError, VaultCredential, VaultError, WalletError, WatchOnlyFilter,
    WatchOnlyOptions,
};

const WALLET: &str = "abababababababababababababababababababababababababababababababab";
const TXID: &str = "0101010101010101010101010101010101010101010101010101010101010101";
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

fn filter() -> HistoryFilter {
    HistoryFilter {
        types: vec![],
        categories: vec![],
        statuses: vec![],
        date_from: None,
        date_to: None,
        text: None,
        min_amount: None,
        watch_only: WatchOnlyFilter::All,
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
fn session_stubs_check_arguments_then_report_not_implemented() {
    let f = fixture();
    let s = &f.session;
    let rt = &f.rt;

    // Multiwallet (R1).
    assert_code!(s.wallet_load_states(), not_implemented: "NetworkSession.wallet_load_states");
    assert_code!(
        rt.block_on(s.load_wallet("AB".repeat(32))),
        "invalid_argument"
    );
    assert_code!(
        rt.block_on(s.unload_wallet(WALLET.into())),
        not_implemented: "NetworkSession.unload_wallet"
    );
    assert_code!(
        rt.block_on(s.import_watch_only("tpub".into(), WatchOnlyOptions::default())),
        not_implemented: "NetworkSession.import_watch_only"
    );
    assert_code!(
        rt.block_on(s.account_xpub(WALLET.into(), 0)),
        not_implemented: "NetworkSession.account_xpub"
    );
    assert_code!(
        rt.block_on(f.engine.existing_networks()),
        not_implemented: "Engine.existing_networks"
    );

    // Transaction actions (R1).
    assert_code!(
        rt.block_on(s.abandon_transaction(WALLET.into(), "zz".into())),
        "invalid_argument"
    );
    assert_code!(
        rt.block_on(s.abandon_transaction(WALLET.into(), TXID.into())),
        not_implemented: "NetworkSession.abandon_transaction"
    );
    assert_code!(
        rt.block_on(s.resend_transaction(WALLET.into(), TXID.into())),
        not_implemented: "NetworkSession.resend_transaction"
    );
    assert_code!(
        rt.block_on(s.drop_unconfirmed(None)),
        not_implemented: "NetworkSession.drop_unconfirmed"
    );
    assert_code!(
        rt.block_on(s.export_history_csv(
            WALLET.into(),
            filter(),
            HistorySort::NewestFirst,
            crate::DisplayUnit::Dash,
            vec![],
            0
        )),
        not_implemented: "NetworkSession.export_history_csv"
    );
    assert_code!(
        rt.block_on(s.tx_notices(WALLET.into(), vec![TXID.into(), "bad".into()])),
        "invalid_argument"
    );
    assert_code!(
        rt.block_on(s.tx_detail_extras(WALLET.into(), TXID.into())),
        not_implemented: "NetworkSession.tx_detail_extras"
    );

    // Fees, tools, console (R1).
    assert_code!(s.fee_policy(), not_implemented: "NetworkSession.fee_policy");
    let bad_outpoint = OutPoint {
        txid: "nothex".into(),
        vout: 0,
    };
    assert_code!(
        rt.block_on(s.coin_selection_summary(
            WALLET.into(),
            vec![bad_outpoint],
            vec![],
            FeeMode::Recommended { target_blocks: 6 },
            false
        )),
        "invalid_argument"
    );
    assert_code!(s.node_info(), not_implemented: "NetworkSession.node_info");
    assert_code!(s.warnings(), not_implemented: "NetworkSession.warnings");
    assert_code!(
        rt.block_on(s.ban_peer("127.0.0.1".into(), 3600)),
        not_implemented: "NetworkSession.ban_peer"
    );
    assert_code!(s.rescan_progress(), not_implemented: "NetworkSession.rescan_progress");
    assert_code!(
        rt.block_on(s.reset_chain_data()),
        not_implemented: "NetworkSession.reset_chain_data"
    );
    assert_code!(
        rt.block_on(s.console_execute(None, b"getblockcount".to_vec(), None)),
        not_implemented: "NetworkSession.console_execute"
    );
    assert_code!(crate::console_commands(), not_implemented: "console_commands");
    assert_code!(
        crate::console_redact(b"walletpassphrase x 60".to_vec()),
        not_implemented: "console_redact"
    );

    // Compat, backups, PSBT (R2).
    assert_code!(
        rt.block_on(s.import_dump_wallet("/nonexistent".into(), ImportOptions::default())),
        not_implemented: "NetworkSession.import_dump_wallet"
    );
    assert_code!(
        rt.block_on(s.import_key_material(
            KeyMaterial::HdSeed { seed: vec![0; 32] },
            ImportOptions::default()
        )),
        not_implemented: "NetworkSession.import_key_material"
    );
    assert_code!(
        rt.block_on(s.export_for_core(
            WALLET.into(),
            CoreExportFormat::DumpWallet,
            "/x".into(),
            "g".into()
        )),
        not_implemented: "NetworkSession.export_for_core"
    );
    assert_code!(
        rt.block_on(f.engine.inspect_wallet_file("/x".into())),
        not_implemented: "Engine.inspect_wallet_file"
    );
    assert_code!(
        rt.block_on(s.backup_wallet(WALLET.into(), "/x".into(), None)),
        not_implemented: "NetworkSession.backup_wallet"
    );
    assert_code!(s.backup_policy(), not_implemented: "NetworkSession.backup_policy");
    assert_code!(crate::parse_psbt(b"cHNidP8=".to_vec()), not_implemented: "parse_psbt");
    // No wallet is registered, so the draft `create_unsigned` hangs off is
    // refused first.
    assert!(matches!(
        s.new_tx_draft(WALLET.into()),
        Err(SendError::WalletNotFound { .. })
    ));

    // Vault (S1).
    let vault = s.vault();
    assert_code!(vault.quick_unlock_policy(), not_implemented: "Vault.quick_unlock_policy");
    assert_code!(
        rt.block_on(vault.recover_with_mnemonic("x".into(), vec![], vec![], vec![])),
        "invalid_argument"
    );
    assert_code!(
        rt.block_on(vault.destroy(VaultCredential::Unencrypted)),
        not_implemented: "Vault.destroy"
    );

    // Desktop (S1).
    assert_code!(crate::decode_qr_codes(vec![]), not_implemented: "decode_qr_codes");
    assert_code!(
        rt.block_on(f.engine.export_logs("/x.zip".into(), vec![])),
        not_implemented: "Engine.export_logs"
    );
    assert_eq!(
        crate::desktop_quick_unlock_provider(),
        crate::QuickUnlockProvider::Unavailable
    );
}

#[test]
fn stubs_report_a_closed_session() {
    let f = fixture();
    assert!(
        f.rt.block_on(f.engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
    assert_code!(f.session.wallet_load_states(), "network_not_open");
    assert_code!(f.session.node_info(), "network_not_open");
    assert_code!(
        f.rt.block_on(f.session.drop_unconfirmed(None)),
        "network_not_open"
    );
    assert_code!(f.session.vault().quick_unlock_policy(), "network_not_open");
}

#[test]
fn parse_psbt_refuses_files_over_100_mib() {
    let big = vec![0u8; (crate::MAX_PSBT_BYTES + 1) as usize];
    assert_code!(crate::parse_psbt(big), "psbt.too_large");
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

/// The codes listed in the §4 row of m2-engine.md whose first cell is
/// `cell`, sorted.
fn contract_row(cell: &str) -> Vec<String> {
    let contract = include_str!("../../../../../docs/contracts/m2-engine.md");
    let prefix = format!("| {cell} |");
    let row = contract
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no {cell} row in m2-engine.md §4"));
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
fn m2_error_codes_match_the_contract() {
    let d = String::new;

    let tx_action = domain(
        [
            TxActionError::TxNotFound { txid: d() },
            TxActionError::Refused {
                refusal: crate::TxActionRefusal::Confirmed,
            },
            TxActionError::SpvNotRunning,
            TxActionError::NoPeers,
        ]
        .iter()
        .map(TxActionError::code),
    );
    assert_eq!(tx_action, contract_row("`TxActionError`"));

    let console = domain(
        [
            ConsoleError::ParseError { detail: d() },
            ConsoleError::RpcError {
                code: -1,
                message: d(),
            },
            ConsoleError::NotAvailable { command: d() },
            ConsoleError::AuthorizationRequired {
                purpose: GrantPurpose::SignMessage,
                wallet_id: None,
            },
            ConsoleError::WalletRequired,
        ]
        .iter()
        .map(ConsoleError::code),
    );
    assert_eq!(console, contract_row("`ConsoleError`"));

    let compat = domain(
        [
            CompatError::FileUnreadable { detail: d() },
            CompatError::UnsupportedFormat { detail: d() },
            CompatError::Corrupt { detail: d() },
            CompatError::PassphraseRequired,
            CompatError::WrongPassphrase,
            CompatError::NoHdChain,
            CompatError::NetworkMismatch {
                found: DashNetwork::Testnet,
            },
            CompatError::InvalidKeyMaterial { detail: d() },
            CompatError::AlreadyExists { wallet_id: d() },
            CompatError::NoVault,
            CompatError::VaultLocked,
            CompatError::GrantInvalid,
            CompatError::WatchOnly,
            CompatError::DestinationUnwritable { detail: d() },
        ]
        .iter()
        .map(CompatError::code),
    );
    assert_eq!(compat, contract_row("`CompatError`"));

    let backup = domain(
        [
            BackupError::VaultLocked,
            BackupError::PassphraseRequired,
            BackupError::WrongPassphrase,
            BackupError::Corrupt { detail: d() },
            BackupError::UnsupportedVersion { version: 9 },
            BackupError::NetworkMismatch,
            BackupError::AlreadyExists { wallet_id: d() },
            BackupError::DestinationUnwritable { detail: d() },
        ]
        .iter()
        .map(BackupError::code),
    );
    assert_eq!(backup, contract_row("`BackupError`"));

    let psbt = domain(
        [
            PsbtError::Invalid { detail: d() },
            PsbtError::TooLarge { size_bytes: 0 },
            PsbtError::NetworkMismatch,
            PsbtError::NotComplete,
            PsbtError::FeeRateTooHigh { duffs_per_kb: 0 },
            PsbtError::WatchOnly,
            PsbtError::VaultLocked,
            PsbtError::GrantInvalid,
            PsbtError::GrantExceeded { max_duffs: 0 },
            PsbtError::NoPeers,
            PsbtError::BroadcastRejected { reason: d() },
            PsbtError::BroadcastUnknown { reason: d() },
        ]
        .iter()
        .map(PsbtError::code),
    );
    assert_eq!(psbt, contract_row("`PsbtError`"));

    let desktop = domain(
        [
            DesktopError::Unsupported { feature: d() },
            DesktopError::OsError { detail: d() },
            DesktopError::NoQrCode,
            DesktopError::ImageUnreadable { detail: d() },
        ]
        .iter()
        .map(DesktopError::code),
    );
    assert_eq!(desktop, contract_row("`DesktopError`"));

    // Variants M2 adds to M1 enums.
    let vault = domain(
        [
            VaultError::QuickUnlockLimitExceeded { limit_duffs: 0 },
            VaultError::PassphraseStale,
            VaultError::NotEmpty,
            VaultError::RecoveryMismatch,
        ]
        .iter()
        .map(VaultError::code),
    );
    assert_eq!(vault, contract_row("`VaultError` (M2 additions)"));
    assert_eq!(
        domain([WalletError::InvalidXpub { detail: d() }.code()]),
        contract_row("`WalletError` (M2 additions)")
    );
    let sync = domain(
        [
            SyncError::SpvRunning,
            SyncError::RescanInProgress,
            SyncError::PeerNotFound { address: d() },
        ]
        .iter()
        .map(SyncError::code),
    );
    assert_eq!(sync, contract_row("`SyncError` (M2 additions)"));
}
