//! The governance console commands (MG-02 §6): Core v24's result shapes
//! from hand-built facade records, and the argument and disabled errors of
//! an offline session (governance sync is off in a fresh one).

use std::sync::Arc;

use dw_console::governance::{
    ListSignal, ListType, count_json, count_text, current_votes_json, get_json,
    governance_info_json, list_entry_json, list_json,
};
use dw_console::{ConsoleContext, ConsoleFailure, Json};
use dw_engine::governance::{
    CurrentVoteRow, GovernanceClock, GovernanceInfo, ProposalCounts, ProposalDetail, ProposalRow,
    ProposalStatus, VoteOutcome, VoteTally, WeightRange,
};
use dw_engine::{DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, SessionOptions};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};

const DATA: &str = "{\"type\":1,\"name\":\"beer\",\"payment_amount\":10}";

/// Display-order hash `first` followed by 31 bytes of `fill`, ending in
/// `last` (hash order in Core's map is by the reversed bytes).
fn hash(first: u8, last: u8) -> String {
    format!("{first:02x}{}{last:02x}", "00".repeat(30))
}

fn range(estimate: u32) -> WeightRange {
    WeightRange {
        min: estimate,
        max: estimate,
        estimate,
    }
}

fn detail(hash: String, yes: u32, no: u32, abstain: u32) -> ProposalDetail {
    ProposalDetail {
        row: ProposalRow {
            hash,
            name: "beer".into(),
            url: "https://example.org".into(),
            payment_address: "yXyz".into(),
            payment_amount: 1_000_000_000,
            start_epoch: 1_000,
            end_epoch: 2_000,
            payments_requested: 1,
            status: ProposalStatus::Voting,
            close_call: false,
            tally: VoteTally {
                yes: range(yes),
                no: range(no),
                abstain: range(abstain),
                net_yes_estimate: i64::from(yes) - i64::from(no),
                voters: yes + no + abstain,
                ranged_votes: 0,
            },
            funded_height: None,
        },
        collateral_txid: "cc".repeat(32),
        created_at: 1_700_000_000,
        data_json: DATA.into(),
    }
}

fn keys(v: &Json) -> Vec<&str> {
    match v {
        Json::Obj(f) => f.iter().map(|(k, _)| k.as_str()).collect(),
        other => panic!("not an object: {other:?}"),
    }
}

fn info() -> GovernanceInfo {
    GovernanceInfo {
        clock: GovernanceClock {
            tip_height: 1_510,
            cycle_progress: 0.5,
            last_superblock: 1_500,
            next_superblock: 1_520,
            blocks_to_superblock: 10,
            superblock_eta: 0,
            voting_cutoff_estimate: 0,
            voting_cutoff_height: 1_510,
            voting_open: true,
        },
        superblock_cycle: 20,
        maturity_window: 10,
        target_spacing: 150,
        proposal_fee: 100_000_000,
        budget: 3_400_000_000,
        // The facade's threshold applies the quorum floor; Core's
        // `fundingthreshold` does not.
        passing_threshold: 5,
        valid_masternodes: 4,
        valid_evonodes: 1,
        valid_weight: 7,
        proposals: Some(ProposalCounts {
            passing: 0,
            failing: 0,
            unfunded: 0,
            total: 0,
            close_call: 0,
        }),
        allocated_estimate: None,
        unallocated_estimate: None,
    }
}

#[test]
fn getgovernanceinfo_has_cores_eight_fields() {
    let json = governance_info_json(&info(), 1);
    assert_eq!(
        json.write(2),
        "{\n  \"governanceminquorum\": 1,\n  \"proposalfee\": 1.00000000,\n  \"superblockcycle\": 20,\n  \"superblockmaturitywindow\": 10,\n  \"lastsuperblock\": 1500,\n  \"nextsuperblock\": 1520,\n  \"fundingthreshold\": 0,\n  \"governancebudget\": 34.00000000\n}"
    );
    // `m_valid_weighted / 10`, no quorum floor.
    let mut big = info();
    big.valid_weight = 4_005;
    assert_eq!(
        governance_info_json(&big, 10).query("fundingthreshold"),
        Some(Json::int(400u32))
    );
}

#[test]
fn gobject_count_in_both_modes() {
    assert_eq!(
        count_json(3, 42).write(2),
        "{\n  \"objects_total\": 3,\n  \"proposals\": 3,\n  \"triggers\": 0,\n  \"other\": 0,\n  \"erased\": 0,\n  \"votes\": 42\n}"
    );
    assert_eq!(
        count_text(3, 42),
        "Governance Objects: 3 (Proposals: 3, Triggers: 0, Other: 0; Erased: 0), Votes: 42"
    );
}

#[test]
fn gobject_list_entry_has_cores_keys_in_cores_order() {
    let d = detail(hash(0, 1), 12, 2, 1);
    let entry = list_entry_json(&d, 10);
    assert_eq!(
        keys(&entry),
        [
            "DataHex",
            "DataString",
            "Hash",
            "CollateralHash",
            "ObjectType",
            "CreationTime",
            "fBlockchainValidity",
            "IsValidReason",
            "fCachedValid",
            "fCachedFunding",
            "fCachedDelete",
            "fCachedEndorsed",
            "AbsoluteYesCount",
            "YesCount",
            "NoCount",
            "AbstainCount",
        ]
    );
    assert_eq!(entry.query("DataHex"), Some(Json::str(hex::encode(DATA))));
    assert_eq!(entry.query("DataString"), Some(Json::str(DATA)));
    assert_eq!(entry.query("Hash"), Some(Json::str(hash(0, 1))));
    assert_eq!(
        entry.query("CollateralHash"),
        Some(Json::str("cc".repeat(32)))
    );
    assert_eq!(entry.query("ObjectType"), Some(Json::int(1u32)));
    assert_eq!(
        entry.query("CreationTime"),
        Some(Json::int(1_700_000_000i64))
    );
    assert_eq!(entry.query("fBlockchainValidity"), Some(Json::Bool(true)));
    assert_eq!(entry.query("IsValidReason"), Some(Json::str("")));
    assert_eq!(entry.query("fCachedValid"), Some(Json::Bool(true)));
    assert_eq!(entry.query("fCachedFunding"), Some(Json::Bool(true)));
    assert_eq!(entry.query("fCachedDelete"), Some(Json::Bool(false)));
    assert_eq!(entry.query("fCachedEndorsed"), Some(Json::Bool(false)));
    assert_eq!(entry.query("AbsoluteYesCount"), Some(Json::int(10u32)));
    assert_eq!(entry.query("YesCount"), Some(Json::int(12u32)));
    assert_eq!(entry.query("NoCount"), Some(Json::int(2u32)));
    assert_eq!(entry.query("AbstainCount"), Some(Json::int(1u32)));
    // Below the threshold the funding flag is off.
    let low = list_entry_json(&detail(hash(0, 1), 3, 0, 0), 10);
    assert_eq!(low.query("fCachedFunding"), Some(Json::Bool(false)));
}

#[test]
fn gobject_list_filters_by_signal_and_type_in_core_hash_order() {
    // Core's map orders hashes by their reversed bytes, so the hash ending
    // in 0x01 comes before the one ending in 0x02 even though it starts
    // with the larger byte.
    let funded = detail(hash(1, 0), 12, 0, 0);
    let unfunded = detail(hash(0, 2), 3, 0, 0);
    let details = [unfunded.clone(), funded.clone()];

    let all = list_json(&details, 10, ListSignal::Valid, ListType::All);
    assert_eq!(
        keys(&all),
        [funded.row.hash.as_str(), unfunded.row.hash.as_str()]
    );
    assert_eq!(
        all.query(&funded.row.hash),
        Some(list_entry_json(&funded, 10))
    );
    assert_eq!(
        keys(&list_json(
            &details,
            10,
            ListSignal::All,
            ListType::Proposals
        )),
        keys(&all)
    );
    assert_eq!(
        keys(&list_json(&details, 10, ListSignal::Funding, ListType::All)),
        [funded.row.hash.as_str()]
    );
    // Nothing is cached delete or endorsed, and no trigger is offered.
    for signal in [ListSignal::Delete, ListSignal::Endorsed] {
        assert_eq!(
            list_json(&details, 10, signal, ListType::All).write(2),
            "{\n}"
        );
    }
    assert_eq!(
        list_json(&details, 10, ListSignal::Valid, ListType::Triggers).write(2),
        "{\n}"
    );
    assert_eq!(
        list_json(&[], 10, ListSignal::Valid, ListType::All).write(2),
        "{\n}"
    );
}

#[test]
fn list_signals_and_types_parse_as_core_does() {
    assert_eq!(ListSignal::parse("valid"), Some(ListSignal::Valid));
    assert_eq!(ListSignal::parse("funding"), Some(ListSignal::Funding));
    assert_eq!(ListSignal::parse("delete"), Some(ListSignal::Delete));
    assert_eq!(ListSignal::parse("endorsed"), Some(ListSignal::Endorsed));
    assert_eq!(ListSignal::parse("all"), Some(ListSignal::All));
    assert_eq!(ListSignal::parse("Valid"), None);
    assert_eq!(ListType::parse("proposals"), Some(ListType::Proposals));
    assert_eq!(ListType::parse("triggers"), Some(ListType::Triggers));
    assert_eq!(ListType::parse("all"), Some(ListType::All));
    assert_eq!(ListType::parse("proposal"), None);
}

#[test]
fn gobject_get_has_cores_keys_and_zero_non_funding_results() {
    let d = detail(hash(0, 1), 12, 2, 1);
    let json = get_json(&d, 10);
    assert_eq!(
        keys(&json),
        [
            "DataHex",
            "DataString",
            "Hash",
            "CollateralHash",
            "ObjectType",
            "CreationTime",
            "fLocalValidity",
            "IsValidReason",
            "fCachedValid",
            "fCachedFunding",
            "fCachedDelete",
            "fCachedEndorsed",
            "FundingResult",
            "ValidResult",
            "DeleteResult",
            "EndorsedResult",
        ]
    );
    assert_eq!(
        json.query("FundingResult").unwrap().write(2),
        "{\n  \"AbsoluteYesCount\": 10,\n  \"YesCount\": 12,\n  \"NoCount\": 2,\n  \"AbstainCount\": 1\n}"
    );
    for key in ["ValidResult", "DeleteResult", "EndorsedResult"] {
        assert_eq!(
            json.query(key).unwrap().write(2),
            "{\n  \"AbsoluteYesCount\": 0,\n  \"YesCount\": 0,\n  \"NoCount\": 0,\n  \"AbstainCount\": 0\n}",
            "{key}"
        );
    }
    assert_eq!(json.query("fLocalValidity"), Some(Json::Bool(true)));
    assert_eq!(json.query("fCachedFunding"), Some(Json::Bool(true)));
}

#[test]
fn gobject_getcurrentvotes_keys_votes_by_their_hash() {
    let proposal = hash(0, 9);
    let txid = "ab".repeat(32);
    let row = |vout: u32, time: i64, outcome, estimate| CurrentVoteRow {
        outpoint: format!("{txid}-{vout}"),
        time,
        outcome,
        weight: WeightRange {
            min: 0,
            max: 4,
            estimate,
        },
        exact: false,
    };
    let rows = [
        row(0, 1_700_000_100, VoteOutcome::Yes, 1),
        row(1, 1_700_000_200, VoteOutcome::Abstain, 4),
    ];
    let json = current_votes_json(&proposal, &rows);
    let Json::Obj(fields) = &json else {
        panic!("not an object")
    };
    assert_eq!(fields.len(), 2);
    for ((key, value), r) in fields.iter().zip(&rows) {
        assert_eq!(key.len(), 64, "{key}");
        assert!(key.bytes().all(|b| b.is_ascii_hexdigit()), "{key}");
        let outcome = if r.outcome == VoteOutcome::Yes {
            "yes"
        } else {
            "abstain"
        };
        assert_eq!(
            value,
            &Json::str(format!(
                "{}:{}:{outcome}:funding:{}",
                r.outpoint, r.time, r.weight.estimate
            ))
        );
    }
    // The key is Core's vote hash: it changes with the outcome and the time.
    assert_ne!(fields[0].0, fields[1].0);
    let again = current_votes_json(&proposal, &[row(0, 1_700_000_100, VoteOutcome::Yes, 1)]);
    assert_eq!(keys(&again), [fields[0].0.as_str()]);
    let other_outcome = current_votes_json(&proposal, &[row(0, 1_700_000_100, VoteOutcome::No, 1)]);
    assert_ne!(keys(&other_outcome), keys(&again));
    assert_eq!(current_votes_json(&proposal, &[]).write(2), "{\n}");
}

// --- an offline session -------------------------------------------------

struct Null;
impl EventSink for Null {
    fn emit(&self, _: EngineEvent) {}
}

struct Offline {
    engine: Engine,
    ctx: ConsoleContext,
    network: DashNetwork,
    /// Keeps the data root alive.
    _dir: Box<dyn std::any::Any>,
}

fn offline() -> Offline {
    let dir = dw_testutil::private_tempdir();
    let engine = Engine::new(
        EngineConfig {
            data_root: dir.path().join("d"),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            },
        },
        Arc::new(Null),
    )
    .unwrap();
    let session = engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                ..Default::default()
            },
        ))
        .unwrap();
    Offline {
        engine,
        ctx: ConsoleContext {
            session,
            wallet: None,
            grant_id: None,
        },
        network: DashNetwork::Regtest,
        _dir: Box::new(dir),
    }
}

impl Offline {
    fn run(&mut self, line: &str) -> Result<String, ConsoleFailure> {
        self.engine.block_on(self.ctx.run(line)).map(|p| p.result)
    }

    fn rpc_error(&mut self, line: &str) -> (i32, String) {
        match self.run(line) {
            Err(ConsoleFailure::Rpc { code, message }) => (code, message),
            other => panic!("{line}: expected an RPC error, got {other:?}"),
        }
    }
}

#[test]
fn governance_commands_answer_the_disabled_error_while_sync_is_off() {
    let mut o = offline();
    let hash = hash(0, 1);
    let txid = "ab".repeat(32);
    let disabled = dw_console::governance::disabled();
    let ConsoleFailure::Rpc { code, message } = disabled else {
        panic!("not an RPC error")
    };
    assert_eq!(code, -1);
    for line in [
        "getgovernanceinfo".to_string(),
        "gobject count".to_string(),
        "gobject count all".to_string(),
        "gobject list".to_string(),
        "gobject list funding proposals".to_string(),
        format!("gobject get {hash}"),
        format!("gobject getcurrentvotes {hash}"),
        format!("gobject getcurrentvotes {hash} {txid} 0"),
        format!("voteraw {txid} 0 {hash} funding yes 1700000000 AAAA"),
    ] {
        assert_eq!(o.rpc_error(&line), (code, message.clone()), "{line}");
    }
}

#[test]
fn getsuperblockbudget_answers_from_the_network_alone() {
    let mut o = offline();
    // 1500 is the first regtest superblock, 1 is not one.
    let budget = dw_engine::governance::superblock_budget(&o.network, 1_500);
    assert!(budget > 0);
    assert_eq!(
        o.run("getsuperblockbudget 1500").unwrap(),
        Json::amount(budget as i64).write(2)
    );
    assert_eq!(o.run("getsuperblockbudget 1").unwrap(), "0.00000000");
    assert_eq!(
        o.rpc_error("getsuperblockbudget -1"),
        (-8, "Block height out of range".into())
    );
    assert_eq!(o.rpc_error("getsuperblockbudget").0, -1);
    assert_eq!(o.rpc_error("getsuperblockbudget 1 2").0, -1);
    // The console converts the index as JSON before Core sees it, then Core
    // checks its type and its range.
    assert!(matches!(
        o.run("getsuperblockbudget abc"),
        Err(ConsoleFailure::Parse(m)) if m == "Error parsing JSON: abc"
    ));
    assert_eq!(
        o.rpc_error("getsuperblockbudget true"),
        (
            -3,
            "Wrong type passed:\n{\n    \"Position 1 (index)\": \"JSON value of type bool is not of expected type number\"\n}".into()
        )
    );
    for out_of_range in ["4294967296", "1.5", "1e3"] {
        assert_eq!(
            o.rpc_error(&format!("getsuperblockbudget {out_of_range}")),
            (-1, "JSON integer out of range".into()),
            "{out_of_range}"
        );
    }
}

#[test]
fn gobject_checks_its_arguments_like_core() {
    let mut o = offline();
    // No command: the RPC help. An unknown one: Core's invalid-command error.
    let (code, help) = o.rpc_error("gobject");
    assert_eq!(code, -1);
    assert!(
        help.starts_with("gobject \"command\"\n\nSet of commands"),
        "{help}"
    );
    assert_eq!(
        o.rpc_error("gobject nosuch"),
        (-8, "Must be a valid command".into())
    );
    // Subcommands that need more than the facade offers.
    for sub in [
        "check",
        "prepare",
        "list-prepared",
        "submit",
        "deserialize",
        "diff",
        "vote-alias",
        "vote-many",
    ] {
        match o.run(&format!("gobject {sub} 1")) {
            Err(ConsoleFailure::NotAvailable(what)) => assert_eq!(what, format!("gobject {sub}")),
            other => panic!("{sub}: {other:?}"),
        }
    }
    // The arguments are checked before the disabled state.
    assert_eq!(
        o.rpc_error("gobject count text"),
        (-8, "mode can be 'json' or 'all'".into())
    );
    assert_eq!(o.rpc_error("gobject count json all").0, -1);
    assert_eq!(o.rpc_error("gobject list a b c").0, -1);
    // A bad filter is a successful string result, as in Core.
    assert_eq!(
        o.run("gobject list nosuch").unwrap(),
        "Invalid signal, should be 'valid', 'funding', 'delete', 'endorsed' or 'all'"
    );
    assert_eq!(
        o.run("gobject list all nosuch").unwrap(),
        "Invalid type, should be 'proposals', 'triggers' or 'all'"
    );
    assert_eq!(o.rpc_error("gobject get").0, -1);
    assert_eq!(
        o.rpc_error("gobject get abc"),
        (
            -8,
            "GovObj hash must be of length 64 (not 3, for 'abc')".into()
        )
    );
    let bad = format!("{}zz", "0".repeat(62));
    assert_eq!(
        o.rpc_error(&format!("gobject get {bad}")),
        (
            -8,
            format!("GovObj hash must be hexadecimal string (not '{bad}')")
        )
    );
    assert_eq!(
        o.rpc_error("gobject getcurrentvotes abc"),
        (
            -8,
            "Governance hash must be of length 64 (not 3, for 'abc')".into()
        )
    );
    assert_eq!(
        o.rpc_error(&format!("gobject getcurrentvotes {} abc 0", hash(0, 1)))
            .0,
        -8
    );
}

#[test]
fn voteraw_checks_its_arguments_like_core() {
    let mut o = offline();
    let txid = "ab".repeat(32);
    let gov = hash(0, 1);
    assert_eq!(o.rpc_error("voteraw 1 2").0, -1);
    assert_eq!(
        o.rpc_error(&format!("voteraw abc 0 {gov} funding yes 1 AAAA")),
        (
            -8,
            "mn collateral tx hash must be of length 64 (not 3, for 'abc')".into()
        )
    );
    assert!(matches!(
        o.run(&format!("voteraw {txid} x {gov} funding yes 1 AAAA")),
        Err(ConsoleFailure::Parse(m)) if m == "Error parsing JSON: x"
    ));
    // Both numeric arguments are type-checked before the command runs.
    assert_eq!(
        o.rpc_error(&format!("voteraw abc null {gov} funding yes true AAAA")),
        (
            -3,
            "Wrong type passed:\n{\n    \"Position 2 (mn-collateral-tx-index)\": \"JSON value of type null is not of expected type number\",\n    \"Position 6 (time)\": \"JSON value of type bool is not of expected type number\"\n}".into()
        )
    );
    assert_eq!(
        o.rpc_error(&format!(
            "voteraw {txid} 2147483648 {gov} funding yes 1 AAAA"
        )),
        (-1, "JSON integer out of range".into())
    );
    assert_eq!(
        o.rpc_error(&format!("voteraw {txid} 0 {gov} nosuch yes 1 AAAA")),
        (
            -8,
            "Invalid vote signal. Please use one of the following: (funding|valid|delete|endorsed)"
                .into()
        )
    );
    for outcome in ["maybe", "none"] {
        assert_eq!(
            o.rpc_error(&format!("voteraw {txid} 0 {gov} funding {outcome} 1 AAAA")),
            (
                -8,
                "Invalid vote outcome. Please use one of the following: 'yes', 'no' or 'abstain'"
                    .into()
            ),
            "{outcome}"
        );
    }
    // Core relays these signals; the wallet tallies funding votes only.
    for signal in ["valid", "delete", "endorsed"] {
        match o.run(&format!("voteraw {txid} 0 {gov} {signal} yes 1 AAAA")) {
            Err(ConsoleFailure::NotAvailable(what)) => {
                assert_eq!(what, format!("voteraw with signal {signal}"));
            }
            other => panic!("{signal}: {other:?}"),
        }
    }
    assert!(matches!(
        o.run(&format!("voteraw {txid} 0 {gov} funding yes soon AAAA")),
        Err(ConsoleFailure::Parse(m)) if m == "Error parsing JSON: soon"
    ));
}

#[test]
fn getcurrentvotes_reads_the_vout_as_cores_atoi() {
    use dw_console::governance::atoi_u32;
    for (text, vout) in [
        ("7", 7),
        (" 7\t", 7),
        ("+7", 7),
        ("7abc", 7),
        ("abc", 0),
        ("", 0),
        ("-1", 0),
        ("+-1", 0),
        ("+", 0),
        ("4294967295", u32::MAX),
        ("99999999999", u32::MAX),
    ] {
        assert_eq!(atoi_u32(text), vout, "{text:?}");
    }
}

#[test]
fn voteraw_words_the_facades_rejections_as_core() {
    let o = "ab".repeat(32) + "-1";
    let h = hash(0, 1);
    let text = |reason: &str| match dw_console::governance::vote_rejection(reason, &o, &h, || 77) {
        ConsoleFailure::Rpc { code, message } => (code, message),
        other => panic!("{reason}: {other:?}"),
    };
    assert_eq!(
        text("unknown_object"),
        (-8, "Governance object not found".into())
    );
    assert_eq!(
        text("masternode_not_found"),
        (-32603, format!("Failure to find masternode in list : {o}"))
    );
    for reason in [
        "bad_signature",
        "unknown_voting_key",
        "wrong_voting_key",
        "too_far_ahead",
    ] {
        assert_eq!(
            text(reason),
            (-32603, "Failure to verify vote.".into()),
            "{reason}"
        );
    }
    // Core's manager leaves the exception default-built for these.
    for reason in ["duplicate", "expired"] {
        assert_eq!(
            text(reason),
            (-32603, "Error voting : GOVERNANCE_EXCEPTION_NONE:".into()),
            "{reason}"
        );
    }
    assert_eq!(
        text("obsolete").1,
        "Error voting : GOVERNANCE_EXCEPTION_NONE:CGovernanceObject::ProcessVote -- Obsolete vote"
    );
    assert_eq!(
        text("same_timestamp").1,
        "Error voting : GOVERNANCE_EXCEPTION_NONE:CGovernanceObject::ProcessVote -- Invalid vote, same timestamp for the different outcome, rejected"
    );
    let often = text("too_often").1;
    assert!(
        often.starts_with(&format!(
            "Error voting : GOVERNANCE_EXCEPTION_TEMPORARY_ERROR:CGovernanceObject::ProcessVote -- Masternode voting too often, MN outpoint = {o}, governance object hash = {h}, last update = 77, current time = "
        )),
        "{often}"
    );
}

#[test]
fn help_lists_the_governance_commands() {
    let mut o = offline();
    let help = o.run("help").unwrap();
    assert!(
        help.contains("== Governance ==\ngetgovernanceinfo\ngetsuperblockbudget index\ngobject \"command\"\nvoteraw "),
        "{help}"
    );
    assert_eq!(
        o.run("help getgovernanceinfo").unwrap(),
        "getgovernanceinfo"
    );
}
