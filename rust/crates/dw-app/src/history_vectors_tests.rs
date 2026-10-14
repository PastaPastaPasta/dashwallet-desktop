//! Shared history vectors (U-07, Sol r1 #2): `dw_engine::history::page` run
//! over a fixed record set, written to
//! `apps/desktop/src/fixture/vectors/history.json`. The renderer's fixture
//! backend replays every case (`apps/desktop/src/fixture/history.test.ts`),
//! so its `history_page` filters, sorts, pages and refuses cursors as the
//! engine does. This test fails when the committed file differs from what
//! the engine produces now; regenerate it with
//! `DW_UPDATE_VECTORS=1 cargo test -p dw-app history_vectors`.

use std::path::{Path, PathBuf};

use dw_engine::history::page;
use dw_engine::{TxRecord, TxStatus, TxStatusKind, TxType};
use serde_json::{Map, Value, json};

use crate::dto::history::{HistoryRequest, HistoryRows, TxRow};
use crate::error::AppError;

const REGENERATE: &str = "DW_UPDATE_VECTORS=1 cargo test -p dw-app history_vectors";

fn vectors_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../apps/desktop/src/fixture/vectors/history.json")
}

/// A txid of 64 lowercase hex digits drawn from `n`, so txid order is not
/// insertion order.
fn txid(n: u64) -> String {
    let mut x = n.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut out = String::new();
    for _ in 0..4 {
        x ^= x >> 30;
        x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x ^= x >> 27;
        x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
        x ^= x >> 31;
        out.push_str(&format!("{x:016x}"));
    }
    out
}

const T: u64 = 1_790_000_000;

struct Spec {
    n: u64,
    index: u32,
    ty: TxType,
    kind: TxStatusKind,
    ts: Option<u64>,
    amount: i64,
    address: Option<&'static str>,
    label: Option<&'static str>,
    watch: bool,
}

#[allow(clippy::too_many_arguments)]
fn spec(
    n: u64,
    ty: TxType,
    kind: TxStatusKind,
    ts: Option<u64>,
    amount: i64,
    address: Option<&'static str>,
    label: Option<&'static str>,
    watch: bool,
) -> Spec {
    Spec {
        n,
        index: 0,
        ty,
        kind,
        ts,
        amount,
        address,
        label,
        watch,
    }
}

/// Every status kind, many types, unknown and equal times, equal amounts,
/// a two-record transaction, watch-only rows, mixed-case addresses and
/// labels, non-ASCII labels and labels with characters Rust's `Debug`
/// escapes (the cursor tag hashes the filter's `Debug` text).
fn records() -> Vec<TxRecord> {
    use TxStatusKind as K;
    use TxType as Ty;
    let mut specs = vec![
        spec(
            1,
            Ty::RecvWithAddress,
            K::Unconfirmed,
            None,
            20_000_000,
            Some("yAbCdEfGhJkLmNpQrStUvWxYz123456789"),
            None,
            false,
        ),
        spec(
            2,
            Ty::RecvWithAddress,
            K::Unconfirmed,
            None,
            125_000_000,
            Some("yQRSTuvwxyzABCDEfghijkmnopq987654"),
            Some("Coffee Refund"),
            false,
        ),
        spec(
            3,
            Ty::SendToAddress,
            K::Confirming,
            Some(T + 9_000),
            -50_000_226,
            Some("yZZtopAlicePaysBobXyz1234567890ab"),
            Some("coffee refund"),
            false,
        ),
        spec(
            4,
            Ty::SendToAddress,
            K::Confirmed,
            Some(T + 8_000),
            -12_345_678,
            Some("yMixedCASEaddressForSearch0000001"),
            Some("RENT"),
            false,
        ),
        spec(
            5,
            Ty::RecvWithAddress,
            K::Confirmed,
            Some(T + 8_000),
            12_345_678,
            Some("yMixedCASEaddressForSearch0000002"),
            None,
            false,
        ),
        spec(
            6,
            Ty::RecvFromOther,
            K::Confirmed,
            Some(T + 8_000),
            12_345_678,
            None,
            None,
            false,
        ),
        spec(
            7,
            Ty::Generated,
            K::Immature,
            Some(T + 7_000),
            288_000_000,
            Some("yMinerRewardAddr00000000000000001"),
            None,
            false,
        ),
        spec(
            8,
            Ty::SendToAddress,
            K::Abandoned,
            Some(T + 6_500),
            -12_345_678,
            Some("yAbandonedSendAddr000000000000001"),
            None,
            false,
        ),
        spec(
            9,
            Ty::SendToOther,
            K::Conflicted,
            Some(T + 6_000),
            -1_000,
            None,
            Some("Say \"hi\" \\ to\tBob"),
            false,
        ),
        spec(
            10,
            Ty::CoinJoinSend,
            K::NotAccepted,
            Some(T + 5_500),
            -99_999,
            Some("yNotAcceptedAddr00000000000000001"),
            Some("Say \"hi\" \\ to\tBob"),
            false,
        ),
        spec(
            11,
            Ty::CoinJoinMixing,
            K::Confirmed,
            Some(T + 5_000),
            -226,
            None,
            None,
            false,
        ),
        spec(
            12,
            Ty::SendToSelf,
            K::Confirmed,
            Some(T + 4_000),
            -374,
            Some("yOwnChangeAddr0000000000000000001"),
            None,
            false,
        ),
        spec(
            13,
            Ty::RecvWithAddress,
            K::Confirmed,
            Some(T + 3_000),
            100_001,
            Some("yWatchOnlyAddrAAAAAAAAAAAAAAAAAA1"),
            Some("Watched"),
            true,
        ),
        spec(
            14,
            Ty::SendToAddress,
            K::Confirmed,
            Some(T + 3_000),
            -100_001,
            Some("yWatchOnlyAddrAAAAAAAAAAAAAAAAAA2"),
            Some("watched"),
            true,
        ),
        spec(
            15,
            Ty::RecvWithAddress,
            K::Confirming,
            Some(T + 2_500),
            100_001,
            Some("yWatchOnlyAddrAAAAAAAAAAAAAAAAAA3"),
            None,
            true,
        ),
        spec(
            16,
            Ty::PlatformTransfer,
            K::Confirmed,
            Some(T + 2_000),
            -300_000_000,
            None,
            Some("ΟΔΥΣΣΕΥΣ"),
            false,
        ),
        spec(
            17,
            Ty::AssetLock,
            K::Confirmed,
            Some(T + 1_900),
            -100_000_000,
            None,
            Some("café au lait"),
            false,
        ),
        spec(
            18,
            Ty::MasternodeRegistration,
            K::Confirmed,
            Some(T + 1_800),
            -1_000_000_000_000,
            Some("yMasternodeCollateral000000000001"),
            None,
            false,
        ),
        spec(
            19,
            Ty::MasternodeUpdate,
            K::Confirmed,
            Some(T + 1_700),
            -1_000,
            None,
            None,
            false,
        ),
        spec(
            20,
            Ty::DustReceive,
            K::Unconfirmed,
            Some(T + 1_600),
            546,
            None,
            Some("bell\u{7}x"),
            false,
        ),
        spec(
            21,
            Ty::DataTransaction,
            K::Confirmed,
            Some(T + 1_500),
            -1_000,
            None,
            Some("bell\u{7}y"),
            false,
        ),
        spec(
            22,
            Ty::RecvWithCoinJoin,
            K::Confirmed,
            Some(T + 1_400),
            100_001_000,
            None,
            Some("Cafe\u{301} decomposed"),
            false,
        ),
        spec(
            23,
            Ty::CoinJoinCreateDenominations,
            K::Confirmed,
            Some(T + 1_300),
            -1_000,
            None,
            Some("Cafe\u{301} again"),
            false,
        ),
        spec(
            24,
            Ty::CoinJoinMakeCollaterals,
            K::Confirmed,
            Some(T + 1_200),
            -40_000,
            None,
            Some("Tab\u{85}Next"),
            false,
        ),
        spec(
            25,
            Ty::CoinJoinCollateralPayment,
            K::Confirmed,
            Some(T + 1_100),
            -10_000,
            None,
            Some("Tab\u{85}Other"),
            false,
        ),
        spec(
            26,
            Ty::Other,
            K::Confirmed,
            Some(T + 1_000),
            0,
            None,
            None,
            false,
        ),
        spec(
            27,
            Ty::RecvWithAddress,
            K::Confirmed,
            Some(T),
            7_000_000_000,
            Some("yOldestKnownTime00000000000000001"),
            None,
            false,
        ),
        spec(
            28,
            Ty::SendToAddress,
            K::Confirmed,
            Some(1),
            -5,
            Some("yAncientAddr00000000000000000001"),
            None,
            true,
        ),
    ];
    // One transaction, two records (a send to two addresses): same txid,
    // time and amount, so the record index breaks the tie in every sort.
    // Listed index 1 first, so a stable sort without the tie-break fails.
    for index in [1, 0] {
        specs.push(Spec {
            index,
            ..spec(
                29,
                Ty::SendToAddress,
                K::Confirmed,
                Some(T + 8_000),
                -12_345_678,
                Some("yTwoRecordsSameTx0000000000000001"),
                Some("Café"),
                false,
            )
        });
    }
    let mut records: Vec<TxRecord> = specs
        .into_iter()
        .map(|s| {
            let confirmations = match s.kind {
                TxStatusKind::Confirmed => 150,
                TxStatusKind::Confirming => 3,
                TxStatusKind::Immature => 40,
                _ => 0,
            };
            TxRecord {
                txid: txid(s.n),
                record_index: s.index,
                tx_type: s.ty,
                category: s.ty.category(),
                status: TxStatus {
                    kind: s.kind,
                    confirmations,
                    instant_locked: s.kind == TxStatusKind::Unconfirmed && s.n % 2 == 0,
                    chain_locked: confirmations > 0,
                    matures_in: (s.kind == TxStatusKind::Immature).then_some(61),
                },
                timestamp: s.ts,
                block_height: (confirmations > 0).then(|| 1_352_000 - confirmations + 1),
                amount: s.amount,
                fee: (s.amount < 0).then_some(226),
                address: s.address.map(str::to_string),
                label: s.label.map(str::to_string),
                counts_toward_balance: s.kind != TxStatusKind::Abandoned,
                involves_watch_only: s.watch,
            }
        })
        .collect();
    // The engine orders txids as Rust strings (by code point). These two
    // differ from JS UTF-16 order (U+FFFD sorts before U+1F600 in Rust, after
    // it in UTF-16); same time and amount, so only the txid decides.
    for id in ["\u{1f600}", "\u{fffd}"] {
        let mut r = records[26].clone();
        r.txid = id.to_string();
        records.push(r);
    }
    records
}

struct Vectors {
    records: Vec<TxRecord>,
    cases: Vec<Value>,
}

impl Vectors {
    /// Runs `request` (as the renderer sends it) through dw-app's DTO and
    /// the engine, records the case, and returns the response's
    /// `nextCursor` (`None` for errors and last pages).
    fn case(&mut self, name: impl Into<String>, request: Value) -> Option<String> {
        let name = name.into();
        let parsed: HistoryRequest = serde_json::from_value(request.clone())
            .unwrap_or_else(|e| panic!("{name}: request does not parse: {e}"));
        let mut case = Map::new();
        case.insert("name".into(), name.clone().into());
        case.insert("request".into(), request);
        let next = match page(self.records.clone(), &parsed.into_query()) {
            Ok(p) => {
                let rows = HistoryRows::from(p);
                let next = rows.next_cursor.clone();
                case.insert("response".into(), serde_json::to_value(rows).unwrap());
                next
            }
            Err(e) => {
                let mut err = serde_json::to_value(AppError::from(e)).unwrap();
                err.as_object_mut().unwrap().remove("message");
                case.insert("error".into(), err);
                None
            }
        };
        assert!(
            !self.cases.iter().any(|c| c["name"] == name.as_str()),
            "duplicate case name {name}"
        );
        self.cases.push(case.into());
        next
    }

    /// A case, then its second page when there is one.
    fn case_and_next(&mut self, name: &str, request: Value) {
        if let Some(cursor) = self.case(name, request.clone()) {
            let mut next = request;
            next["cursor"] = cursor.into();
            self.case(format!("{name} (page 2)"), next);
        }
    }

    /// Walks every page of `request`, one case per page.
    fn chain(&mut self, name: &str, request: Value) -> Vec<String> {
        let mut cursors = Vec::new();
        let mut req = request;
        for hop in 1.. {
            assert!(hop < 50, "{name}: cursor chain does not end");
            match self.case(format!("{name} #{hop}"), req.clone()) {
                Some(c) => {
                    cursors.push(c.clone());
                    req["cursor"] = c.into();
                }
                None => break,
            }
        }
        cursors
    }
}

/// The base request plus `fields`.
fn req(fields: Value) -> Value {
    let mut r = json!({ "categories": [], "text": null, "sort": "newestFirst", "cursor": null, "limit": 4 });
    for (k, v) in fields.as_object().expect("fields are an object") {
        r[k] = v.clone();
    }
    r
}

fn build() -> Value {
    let records = records();
    let rows: Vec<TxRow> = records.iter().cloned().map(TxRow::from).collect();
    let mut v = Vectors {
        records,
        cases: Vec::new(),
    };

    // Default page, smaller than the set.
    v.case("default newest first", req(json!({ "limit": 10 })));

    // Every sort, walked to the end by feeding each nextCursor back.
    let mut chains = Vec::new();
    for sort in [
        "newestFirst",
        "oldestFirst",
        "amountDescending",
        "amountAscending",
    ] {
        chains.push(v.chain(
            &format!("chain {sort}"),
            req(json!({ "sort": sort, "limit": 7 })),
        ));
    }
    let newest_cursor = chains[0][0].clone();

    // Each filter alone.
    let r = |f: Value| req(f);
    v.case_and_next(
        "types",
        r(json!({ "types": ["sendToAddress", "recvWithAddress"] })),
    );
    v.case_and_next("types empty is all", r(json!({ "types": [] })));
    v.case_and_next("categories sent", r(json!({ "categories": ["sent"] })));
    v.case_and_next(
        "categories coinJoin+platform",
        r(json!({ "categories": ["coinJoin", "platform"] })),
    );
    v.case_and_next(
        "statuses unconfirmed",
        r(json!({ "statuses": ["unconfirmed"], "limit": 2 })),
    );
    v.case_and_next(
        "statuses rare",
        r(json!({ "statuses": ["conflicted", "abandoned", "immature", "notAccepted"], "limit": 2 })),
    );
    v.case_and_next("dateFrom", r(json!({ "dateFrom": T + 5_000 })));
    v.case_and_next(
        "dateTo excludes its own second",
        r(json!({ "dateTo": T + 1_000 })),
    );
    v.case_and_next(
        "dateFrom and dateTo",
        r(json!({ "dateFrom": T + 1_000, "dateTo": T + 8_000 })),
    );
    v.case(
        "dateFrom equals dateTo",
        r(json!({ "dateFrom": T + 8_000, "dateTo": T + 8_000 })),
    );
    v.case_and_next(
        "dateFrom 0 drops unknown times",
        r(json!({ "dateFrom": 0, "sort": "oldestFirst" })),
    );
    v.case_and_next("minAmount", r(json!({ "minAmount": 12_345_678 })));
    v.case_and_next(
        "minAmount 0",
        r(json!({ "minAmount": 0, "sort": "amountAscending" })),
    );
    v.case_and_next(
        "watchOnly yes",
        r(json!({ "watchOnly": "yes", "limit": 2 })),
    );
    v.case_and_next("watchOnly no", r(json!({ "watchOnly": "no" })));
    v.case_and_next("watchOnly all", r(json!({ "watchOnly": "all" })));
    // Txid 29 has two records, so these match two rows.
    let t29 = txid(29);
    v.case_and_next(
        "text txid substring",
        r(json!({ "text": &t29[10..20], "limit": 1 })),
    );
    v.case_and_next(
        "text txid uppercase",
        r(json!({ "text": t29[10..20].to_uppercase(), "limit": 1 })),
    );
    v.case_and_next(
        "text address any case",
        r(json!({ "text": "MIXEDcaseADDRESS", "limit": 1 })),
    );
    v.case_and_next(
        "text label any case",
        r(json!({ "text": "COFFEE refund", "limit": 1 })),
    );
    v.case_and_next(
        "text non-ASCII label",
        r(json!({ "text": "CAFÉ", "limit": 1 })),
    );
    v.case_and_next("text Greek capitals", r(json!({ "text": "οδυσσευς" })));
    v.case_and_next("text Greek final sigma", r(json!({ "text": "ΣΕΥΣ" })));
    v.case_and_next(
        "text decomposed accent",
        r(json!({ "text": "e\u{301}", "limit": 1 })),
    );
    v.case_and_next(
        "text quotes backslash tab",
        r(json!({ "text": "\"HI\" \\ TO\tbob", "limit": 1 })),
    );
    v.case_and_next(
        "text control char",
        r(json!({ "text": "l\u{7}", "limit": 1 })),
    );
    v.case_and_next(
        "text NEL inside",
        r(json!({ "text": "B\u{85}n", "limit": 1 })),
    );
    // Trailing NEL is trimmed: this is the text "b" (but the tag keeps it).
    v.case_and_next(
        "text NEL trailing",
        r(json!({ "text": "b\u{85}", "limit": 1 })),
    );
    v.case_and_next("text blank", r(json!({ "text": "   " })));
    v.case_and_next(
        "text surrounding spaces",
        r(json!({ "text": "  coffee  ", "limit": 1 })),
    );
    v.case_and_next(
        "text surrounding NEL and NBSP",
        r(json!({ "text": "\u{85}\u{a0}coffee\u{3000}", "limit": 1 })),
    );
    v.case_and_next(
        "text surrounding BOM is not trimmed",
        r(json!({ "text": "\u{feff}coffee" })),
    );
    v.case_and_next("text no match", r(json!({ "text": "zzzz-nothing" })));
    v.case_and_next(
        "combination",
        r(json!({
            "types": ["sendToAddress", "recvWithAddress", "sendToSelf"],
            "categories": ["sent", "received"],
            "statuses": ["confirmed", "confirming"],
            "dateFrom": T,
            "dateTo": T + 9_000,
            "minAmount": 100_000,
            "watchOnly": "no",
            "text": "Y",
            "sort": "amountDescending",
            "limit": 2,
        })),
    );

    // Limits.
    v.case("limit 0", r(json!({ "limit": 0 })));
    v.case("limit 501", r(json!({ "limit": 501 })));
    v.case("limit 500", r(json!({ "limit": 500 })));
    v.case("limit 1", r(json!({ "limit": 1 })));
    v.case(
        "limit 0 beats a bad cursor",
        r(json!({ "limit": 0, "cursor": "garbage" })),
    );

    // Date range and text length.
    v.case(
        "dateFrom after dateTo",
        r(json!({ "dateFrom": T + 2, "dateTo": T + 1 })),
    );
    v.case(
        "bad date range beats a bad cursor",
        r(json!({ "dateFrom": T + 2, "dateTo": T + 1, "cursor": "garbage" })),
    );
    v.case("text 257 chars", r(json!({ "text": "a".repeat(257) })));
    v.case("text 256 chars", r(json!({ "text": "a".repeat(256) })));
    v.case(
        "text 257 two-byte chars",
        r(json!({ "text": "é".repeat(257) })),
    );
    v.case(
        "text 256 two-byte chars",
        r(json!({ "text": "é".repeat(256) })),
    );
    v.case(
        "text 257 astral chars",
        r(json!({ "text": "😀".repeat(257) })),
    );
    v.case(
        "text 256 astral chars",
        r(json!({ "text": "😀".repeat(256) })),
    );
    v.case(
        "text over 256 only with its spaces",
        r(json!({ "text": format!("{}{}", "a".repeat(250), " ".repeat(10)) })),
    );
    v.case("text blank over 256", r(json!({ "text": " ".repeat(300) })));

    // Cursors the engine refuses.
    v.case(
        "cursor from another sort",
        r(json!({ "sort": "oldestFirst", "limit": 7, "cursor": &newest_cursor })),
    );
    let sent_cursor = v
        .case(
            "cursor source sent",
            r(json!({ "categories": ["sent"], "limit": 2 })),
        )
        .expect("sent has a second page");
    v.case(
        "cursor from another filter",
        r(json!({ "categories": ["received"], "limit": 2, "cursor": &sent_cursor })),
    );
    v.case(
        "cursor from another limit is fine",
        r(json!({ "categories": ["sent"], "limit": 3, "cursor": &sent_cursor })),
    );
    let fields: Vec<&str> = newest_cursor.split('.').collect();
    assert_eq!(fields.len(), 5);
    let (tag, primary, tx, index) = (fields[1], fields[2], fields[3], fields[4]);
    let with = |t: &str, p: &str, x: &str, i: &str| format!("v1.{t}.{p}.{x}.{i}");
    let malformed = [
        ("garbage", "garbage".to_string()),
        ("empty", String::new()),
        ("v1 only", "v1.".to_string()),
        ("version 2", format!("v2.{tag}.{primary}.{tx}.{index}")),
        ("four fields", format!("v1.{tag}.{primary}.{tx}")),
        ("six fields", format!("{newest_cursor}.0")),
        ("index not a number", with(tag, primary, tx, "x")),
        ("index negative", with(tag, primary, tx, "-1")),
        ("index over u32", with(tag, primary, tx, "4294967296")),
        ("index empty", with(tag, primary, tx, "")),
        ("primary not a number", with(tag, "1e3", tx, index)),
        (
            "primary over i128",
            with(tag, "170141183460469231731687303715884105728", tx, index),
        ),
        ("primary sign only", with(tag, "-", tx, index)),
        ("tag empty", with("", primary, tx, index)),
        ("tag not hex", with("zz", primary, tx, index)),
        ("tag over u64", with(&format!("1{tag}"), primary, tx, index)),
        ("tag minus", with(&format!("-{tag}"), primary, tx, index)),
    ];
    for (name, cursor) in malformed {
        v.case(
            format!("cursor {name}"),
            r(json!({ "limit": 7, "cursor": cursor })),
        );
    }
    // Cursors Rust's integer parsing accepts in more than one spelling.
    let accepted = [
        (
            "tag uppercase",
            with(&tag.to_uppercase(), primary, tx, index),
        ),
        (
            "tag leading zero",
            with(&format!("0{tag}"), primary, tx, index),
        ),
        ("tag plus", with(&format!("+{tag}"), primary, tx, index)),
        ("index plus", with(tag, primary, tx, &format!("+{index}"))),
        (
            "index leading zeros",
            with(tag, primary, tx, &format!("000{index}")),
        ),
        ("primary plus", with(tag, &format!("+{primary}"), tx, index)),
        (
            "primary min i128",
            with(tag, "-170141183460469231731687303715884105728", tx, index),
        ),
        ("txid empty", with(tag, primary, "", index)),
        (
            "txid not in the set",
            with(tag, &(T + 8_000).to_string(), &"f".repeat(64), "9"),
        ),
        (
            "txid non-ASCII",
            with(tag, &(T + 8_000).to_string(), "é", "0"),
        ),
        (
            "between two records of one tx",
            with(tag, &(T + 8_000).to_string(), &txid(29), "0"),
        ),
    ];
    for (name, cursor) in accepted {
        v.case(
            format!("cursor {name}"),
            r(json!({ "limit": 7, "cursor": cursor })),
        );
    }

    // The tag hashes the filter as the engine holds it: blank text is no
    // text, but other text keeps its surrounding spaces.
    let plain = v
        .case(
            "cursor source text",
            r(json!({ "text": "coffee", "limit": 1 })),
        )
        .expect("two coffee rows");
    v.case(
        "cursor text with surrounding spaces",
        r(json!({ "text": " coffee ", "limit": 1, "cursor": &plain })),
    );
    v.case(
        "cursor text with other case",
        r(json!({ "text": "COFFEE", "limit": 1, "cursor": &plain })),
    );
    let no_text = v
        .case("cursor source no text", r(json!({ "limit": 3 })))
        .expect("more than three rows");
    v.case(
        "cursor blank text is no text",
        r(json!({ "text": "  ", "limit": 3, "cursor": &no_text })),
    );
    v.case(
        "cursor explicit defaults",
        r(json!({ "types": [], "statuses": [], "watchOnly": "all", "limit": 3, "cursor": &no_text })),
    );
    v.case(
        "cursor watchOnly differs",
        r(json!({ "watchOnly": "no", "limit": 3, "cursor": &no_text })),
    );

    json!({
        "comment": format!(
            "Generated by dw-app (rust/crates/dw-app/src/history_vectors_tests.rs) from dw_engine::history::page; do not edit. Regenerate with {REGENERATE}."
        ),
        "rows": rows,
        "cases": v.cases,
    })
}

#[test]
fn history_vectors_are_current() {
    let text = serde_json::to_string_pretty(&build()).unwrap() + "\n";
    let path = vectors_path();
    if std::env::var_os("DW_UPDATE_VECTORS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == text,
        "apps/desktop/src/fixture/vectors/history.json is stale; run {REGENERATE}"
    );
}
