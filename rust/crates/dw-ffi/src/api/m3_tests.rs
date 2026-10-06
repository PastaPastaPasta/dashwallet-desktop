//! The M3 contract surface through the FFI (docs/contracts/m3-engine.md):
//! every stub checks its arguments and the session first, then fails with
//! its domain's typed `NotImplemented` naming the call; the constant calls
//! answer; the error codes match §4.

use std::sync::Arc;

use crate::{
    CoinJoinError, CoinJoinSettings, CollateralChoice, CollateralRefusal, DashNetwork, Engine,
    EngineConfig, EngineEvent, EngineObserver, FeeSourceChoice, GovernanceError, MasternodeError,
    MasternodeKeyRole, MasternodeQuery, MasternodeType, MasternodeTypeFilter,
    MixedCoinsDestination, NetworkSession, OperatorKeyChoice, OutPoint, ProposalDraft,
    ProposalField, ProposalQuery, ProposalSource, RegistrationRequest, RevocationReason,
    RevokeRequest, SessionOptions, UpdateServiceRequest, VoteOutcome,
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

fn draft() -> ProposalDraft {
    ProposalDraft {
        name: "test".into(),
        url: "https://dash.org".into(),
        payment_address: "yTestAddress".into(),
        payment_amount: 1,
        payment_count: 1,
        first_superblock_height: 1_520,
    }
}

fn registration(wallet: &str) -> RegistrationRequest {
    RegistrationRequest {
        wallet_id: wallet.into(),
        node_type: MasternodeType::Regular,
        collateral: CollateralChoice::FundNew,
        service_addresses: vec![],
        owner_address: None,
        voting_address: None,
        operator_key: OperatorKeyChoice::Generate,
        payout_address: "yPayout".into(),
        operator_reward_x100: 0,
        platform: None,
        fee_source: FeeSourceChoice::Automatic,
    }
}

#[test]
fn constants_answer_without_a_session() {
    let limits = crate::coinjoin_limits();
    assert_eq!(limits.min_mixing_balance, 140_001);
    assert_eq!(limits.denominations.len(), 5);
    assert_eq!(limits.defaults.rounds, 4);
    assert_eq!(limits.defaults.target_amount_dash, 1_000);
    assert!(limits.defaults.enabled);

    let gov = crate::governance_params(DashNetwork::Mainnet);
    assert_eq!(
        (gov.superblock_cycle, gov.maturity_window, gov.min_quorum),
        (16_616, 1_662, 10)
    );
    assert_eq!(gov.proposal_fee, 100_000_000);
    let devnet = crate::governance_params(DashNetwork::Devnet { name: "x".into() });
    assert_eq!(devnet.superblock_cycle, 24);

    let mn = crate::masternode_network_defaults(DashNetwork::Testnet);
    assert_eq!((mn.core_p2p_port, mn.platform_p2p_port), (19_999, 22_000));
    assert_eq!(mn.max_shares, 8);
}

#[test]
fn coinjoin_stubs_check_arguments_then_report_not_implemented() {
    let f = fixture();
    let s = &f.session;
    let rt = &f.rt;
    assert_code!(s.coinjoin_settings(), not_implemented: "NetworkSession.coinjoin_settings");
    let bad = CoinJoinSettings {
        rounds: 1,
        ..crate::coinjoin_limits().defaults
    };
    assert_code!(
        rt.block_on(s.set_coinjoin_settings(bad)),
        "invalid_argument"
    );
    assert_code!(
        rt.block_on(s.set_coinjoin_settings(crate::coinjoin_limits().defaults)),
        not_implemented: "NetworkSession.set_coinjoin_settings"
    );
    assert_code!(s.coinjoin_status("zz".into()), "invalid_argument");
    assert_code!(
        s.coinjoin_status(WALLET.into()),
        not_implemented: "NetworkSession.coinjoin_status"
    );
    assert_code!(
        rt.block_on(s.start_mixing(WALLET.into())),
        not_implemented: "NetworkSession.start_mixing"
    );
    assert_code!(
        rt.block_on(s.stop_mixing(WALLET.into())),
        not_implemented: "NetworkSession.stop_mixing"
    );
    assert_code!(
        rt.block_on(s.set_coinjoin_salt(WALLET.into(), "AB".repeat(32))),
        "invalid_argument"
    );
    assert_code!(
        rt.block_on(s.set_coinjoin_salt(WALLET.into(), HASH.into())),
        not_implemented: "NetworkSession.set_coinjoin_salt"
    );
    assert_code!(
        rt.block_on(s.coinjoin_salt(WALLET.into())),
        not_implemented: "NetworkSession.coinjoin_salt"
    );
    assert_code!(
        rt.block_on(s.generate_coinjoin_salt(WALLET.into())),
        not_implemented: "NetworkSession.generate_coinjoin_salt"
    );
    assert_code!(
        rt.block_on(s.coinjoin_recovery_scan(WALLET.into())),
        not_implemented: "NetworkSession.coinjoin_recovery_scan"
    );
    assert_code!(
        rt.block_on(s.mixed_coins_sweep_plan(WALLET.into(), MixedCoinsDestination::Wallet)),
        not_implemented: "NetworkSession.mixed_coins_sweep_plan"
    );
    // The shielded destination is M4 work: its own call name.
    assert_code!(
        rt.block_on(s.move_mixed_coins(WALLET.into(), MixedCoinsDestination::Shielded, "g".into())),
        not_implemented: "NetworkSession.move_mixed_coins.shielded"
    );
    assert_code!(s.network_stats(), not_implemented: "NetworkSession.network_stats");
}

#[test]
fn governance_stubs_check_arguments_then_report_not_implemented() {
    let f = fixture();
    let s = &f.session;
    let rt = &f.rt;
    assert_code!(
        s.governance_sync_state(),
        not_implemented: "NetworkSession.governance_sync_state"
    );
    assert_code!(
        rt.block_on(s.set_governance_sync_enabled(true)),
        not_implemented: "NetworkSession.set_governance_sync_enabled"
    );
    let mine = |w: &str| ProposalQuery {
        source: ProposalSource::Mine {
            wallet_id: w.into(),
        },
        title_filter: None,
    };
    assert_code!(rt.block_on(s.proposals(mine("zz"))), "invalid_argument");
    assert_code!(
        rt.block_on(s.proposals(mine(WALLET))),
        not_implemented: "NetworkSession.proposals"
    );
    assert_code!(
        rt.block_on(s.proposal_detail("zz".into())),
        "invalid_argument"
    );
    assert_code!(
        rt.block_on(s.proposal_detail(HASH.into())),
        not_implemented: "NetworkSession.proposal_detail"
    );
    assert_code!(
        rt.block_on(s.voting_masternodes(HASH.into(), Some("zz".into()))),
        "invalid_argument"
    );
    assert_code!(
        rt.block_on(s.cast_votes(HASH.into(), VoteOutcome::Yes, vec!["zz".into()], "g".into())),
        "invalid_argument"
    );
    assert_code!(
        rt.block_on(s.cast_votes(HASH.into(), VoteOutcome::No, vec![HASH.into()], "g".into())),
        not_implemented: "NetworkSession.cast_votes"
    );
    assert_code!(s.superblock_dates(12), not_implemented: "NetworkSession.superblock_dates");
    assert_code!(
        s.validate_proposal(draft()),
        not_implemented: "NetworkSession.validate_proposal"
    );
    assert_code!(s.proposal_json(draft()), not_implemented: "NetworkSession.proposal_json");
    assert_code!(
        s.proposal_payload_hex(draft()),
        not_implemented: "NetworkSession.proposal_payload_hex"
    );
    assert_code!(
        rt.block_on(s.create_proposal(WALLET.into(), draft(), "g".into())),
        not_implemented: "NetworkSession.create_proposal"
    );
    assert_code!(
        rt.block_on(s.pending_proposals(WALLET.into())),
        not_implemented: "NetworkSession.pending_proposals"
    );
    assert_code!(
        rt.block_on(s.submit_proposal(WALLET.into(), "zz".into())),
        "invalid_argument"
    );
    assert_code!(
        rt.block_on(s.governance_info()),
        not_implemented: "NetworkSession.governance_info"
    );
    assert_code!(s.governance_clock(), not_implemented: "NetworkSession.governance_clock");
}

#[test]
fn masternode_calls_answer_offline_and_shared_sessions_stay_not_implemented() {
    let f = fixture();
    let s = &f.session;
    let rt = &f.rt;
    // The list before any sync: unavailable, no rows (no wallet, nothing
    // tracked), unknown hashes are not found.
    let state = s.masternode_list_state().unwrap();
    assert!(!state.available);
    assert_eq!(state.total, 0);
    let query = MasternodeQuery {
        type_filter: MasternodeTypeFilter::All,
        text: None,
        owned_only: false,
        hide_banned: false,
    };
    assert!(
        rt.block_on(s.masternodes(query.clone()))
            .unwrap()
            .is_empty()
    );
    assert_code!(
        rt.block_on(s.masternode_detail("zz".into())),
        "invalid_argument"
    );
    assert_code!(
        rt.block_on(s.masternode_detail(HASH.into())),
        "masternode.not_found"
    );
    assert_code!(
        rt.block_on(s.locate_masternodes("1.2.3.4".into())),
        "masternode.list_unavailable"
    );

    // ProTx (protx.rs): arguments, then the wallet.
    assert_code!(
        rt.block_on(s.prepare_registration(registration("zz"), "g".into())),
        "invalid_argument"
    );
    assert_code!(
        rt.block_on(s.prepare_registration(registration(WALLET), "g".into())),
        "wallet_not_found"
    );
    assert_code!(
        rt.block_on(s.collateral_candidates(WALLET.into(), MasternodeType::Evo)),
        "wallet_not_found"
    );
    let update = UpdateServiceRequest {
        pro_tx_hash: "zz".into(),
        service_addresses: vec!["1.2.3.4:19899".into()],
        operator_secret: Some(vec![b'a'; 64]),
        platform: None,
        operator_payout_address: None,
        fee_source: FeeSourceChoice::Automatic,
        fee_wallet_id: WALLET.into(),
    };
    assert_code!(
        rt.block_on(s.prepare_update_service(update, "g".into())),
        "invalid_argument"
    );
    let revoke = RevokeRequest {
        pro_tx_hash: HASH.into(),
        operator_secret: None,
        reason: RevocationReason::ChangeOfKeys,
        fee_source: FeeSourceChoice::Automatic,
        fee_wallet_id: WALLET.into(),
    };
    assert_code!(
        rt.block_on(s.prepare_revoke(revoke, "g".into())),
        "wallet_not_found"
    );
    // v24 shared masternodes: not built yet.
    let too_large = "x".repeat(dw_protx::params::MAX_ENVELOPE_BYTES + 1);
    assert_code!(
        rt.block_on(s.import_shared_message(WALLET.into(), too_large)),
        "masternode.shared_envelope_too_large"
    );
    assert_code!(
        rt.block_on(s.import_shared_message(WALLET.into(), "{}".into())),
        not_implemented: "NetworkSession.import_shared_message"
    );
    assert_code!(
        rt.block_on(s.prepare_dissolve_now(HASH.into(), true, WALLET.into(), "g".into())),
        not_implemented: "NetworkSession.prepare_dissolve_now"
    );

    // Keychain, tracked masternodes, evonode tools (masternode_keys.rs).
    assert_code!(
        rt.block_on(s.masternode_keys(WALLET.into(), MasternodeKeyRole::Operator, 0, 101)),
        "invalid_argument"
    );
    assert_code!(
        rt.block_on(s.masternode_keys(WALLET.into(), MasternodeKeyRole::Operator, 0, 10)),
        "wallet_not_found"
    );
    // IOS-082: tracking works before the list synced, and persists.
    let tracked = rt
        .block_on(s.track_masternode(HASH.into(), Some("rack 3".into())))
        .unwrap();
    assert_eq!(tracked.label.as_deref(), Some("rack 3"));
    assert!(tracked.attached_roles.is_empty());
    assert_eq!(tracked.row.status, crate::MasternodeListStatus::Unknown);
    assert_code!(
        rt.block_on(s.track_masternode(HASH.into(), None)),
        "masternode.already_tracked"
    );
    assert_eq!(rt.block_on(s.tracked_masternodes()).unwrap().len(), 1);
    assert_eq!(rt.block_on(s.masternodes(query)).unwrap().len(), 1);
    assert_code!(
        rt.block_on(s.attach_masternode_key(
            HASH.into(),
            MasternodeKeyRole::Voting,
            b"not a real key".to_vec(),
            "g".into()
        )),
        "masternode.invalid_key"
    );
    assert!(rt.block_on(s.untrack_masternode(HASH.into())).unwrap());
    assert!(!rt.block_on(s.untrack_masternode(HASH.into())).unwrap());
    assert_code!(
        rt.block_on(s.evonode_status(HASH.into())),
        not_implemented: "NetworkSession.evonode_status.platform"
    );
    let vault = s.vault();
    assert_code!(
        rt.block_on(vault.reveal_masternode_key(
            Some(WALLET.into()),
            Some(HASH.into()),
            MasternodeKeyRole::Owner,
            0,
            "g".into()
        )),
        "invalid_argument"
    );
    assert_code!(
        rt.block_on(vault.reveal_masternode_key(
            Some(WALLET.into()),
            None,
            MasternodeKeyRole::Owner,
            0,
            "g".into()
        )),
        "wallet_not_found"
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
    assert_code!(f.session.governance_clock(), "network_not_open");
    assert_code!(f.session.masternode_list_state(), "network_not_open");
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

    let governance = domain(
        [
            GovernanceError::SyncDisabled,
            GovernanceError::NotSynced,
            GovernanceError::ProposalNotFound { hash: d() },
            GovernanceError::InvalidProposal {
                field: ProposalField::Name,
            },
            GovernanceError::NoVotingKeys,
            GovernanceError::VoteTooOften {
                retry_after_secs: 0,
            },
            GovernanceError::InsufficientFunds {
                needed: 0,
                available: 0,
            },
            GovernanceError::CollateralUnconfirmed { confirmations: 0 },
            GovernanceError::ProposalExpired,
            GovernanceError::WatchOnly,
            GovernanceError::VaultLocked,
            GovernanceError::GrantInvalid,
            GovernanceError::NoPeers,
            GovernanceError::BroadcastRejected { reason: d() },
        ]
        .iter()
        .map(GovernanceError::code),
    );
    assert_eq!(governance, contract_row("`GovernanceError`"));

    let masternode = domain(
        [
            MasternodeError::ListUnavailable,
            MasternodeError::NotFound { pro_tx_hash: d() },
            MasternodeError::KeyNotInWallet {
                role: MasternodeKeyRole::Owner,
            },
            MasternodeError::InvalidService { detail: d() },
            MasternodeError::InvalidKey {
                role: MasternodeKeyRole::Operator,
                detail: d(),
            },
            MasternodeError::InvalidPayout { detail: d() },
            MasternodeError::DuplicateAddress { detail: d() },
            MasternodeError::CollateralUnavailable {
                refusal: CollateralRefusal::WrongAmount,
            },
            MasternodeError::InsufficientFunds {
                needed: 0,
                available: 0,
            },
            MasternodeError::OperatorSecretMismatch,
            MasternodeError::OperatorSecretUnconfirmed,
            MasternodeError::CollateralSignatureInvalid,
            MasternodeError::UnsupportedEntry { detail: d() },
            MasternodeError::WatchOnly,
            MasternodeError::VaultLocked,
            MasternodeError::GrantInvalid,
            MasternodeError::NoPeers,
            MasternodeError::BroadcastRejected { reason: d() },
            MasternodeError::SharedEnvelopeInvalid { detail: d() },
            MasternodeError::SharedEnvelopeTooLarge { size_bytes: 0 },
            MasternodeError::SharedNetworkMismatch,
            MasternodeError::SharedSessionNotFound { session_id: d() },
            MasternodeError::SharedInputsRefused { detail: d() },
            MasternodeError::SharedCoinSpent {
                outpoint: OutPoint {
                    txid: HASH.into(),
                    vout: 0,
                },
            },
            MasternodeError::AlreadyTracked { pro_tx_hash: d() },
            MasternodeError::PlatformUnavailable,
        ]
        .iter()
        .map(MasternodeError::code),
    );
    assert_eq!(masternode, contract_row("`MasternodeError`"));
}

/// Engine failures reach the host with their M3 codes.
#[test]
fn engine_failures_map_to_their_codes() {
    use dw_engine::{CoinJoinFailure, EngineError, GovernanceFailure, MasternodeFailure};
    let cj: CoinJoinError =
        EngineError::from(CoinJoinFailure::InsufficientFunds { min_duffs: 140_001 }).into();
    assert_eq!(cj.code(), "coinjoin.insufficient_funds");
    let gov: GovernanceError = EngineError::from(GovernanceFailure::InvalidProposal(
        dw_engine::ProposalField::Url,
    ))
    .into();
    assert!(matches!(
        gov,
        GovernanceError::InvalidProposal {
            field: ProposalField::Url
        }
    ));
    let mn: MasternodeError = EngineError::from(MasternodeFailure::KeyNotInWallet(
        dw_engine::MasternodeKeyRole::Voting,
    ))
    .into();
    assert!(matches!(
        mn,
        MasternodeError::KeyNotInWallet {
            role: MasternodeKeyRole::Voting
        }
    ));
    let locked: MasternodeError = EngineError::Vault(dw_vault::VaultError::Locked).into();
    assert_eq!(locked.code(), "masternode.vault_locked");
}
