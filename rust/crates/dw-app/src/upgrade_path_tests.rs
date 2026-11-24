//! Upgrade path (PLAN-1.0 H-08): a data root that a released app wrote opens
//! on this build with the same wallets, balances, history, labels, settings
//! and keys.
//!
//! Each `testdata/upgrade/<tag>/` holds `data/`, written by the release
//! binary of `<tag>` (`apps/desktop/scripts/upgrade-fixture.sh`), and
//! `expected.json`: the release's own answers to the read commands, recorded
//! after a restart, and the message signatures it made. The test copies
//! `data/` into a private directory and opens it twice, each time in a child
//! process (the mock app's engine outlives its drop, so a second open in this
//! process would find its databases held). A child unlocks the vault with the
//! release's passphrase, asks the same commands, signs the same messages and
//! shuts the engine down. Every field the release answered must come back
//! equal (fields this build added are free), except the renames in
//! [`RENAMED`] and the per-unlock values in [`VOLATILE`]; and the second
//! open must answer exactly as the first, less [`VOLATILE`]. From v0.2.0 a
//! snapshot also holds a signature by every funded address
//! (`fundedSignatures`), which each open must make again, and dashd's fee
//! for the payment; for v0.1.0 the fee and the deposits are read from the
//! recorded transactions. The recorded balances must account for them to the
//! duff. After its calls, each open also prepares and signs, without
//! broadcasting, a spend of every coin the release recorded, verifies each
//! input's signature, abandons it and checks that the wallet is as it was
//! ([`spend_every_recorded_coin`]). A new release is covered by adding its
//! directory.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashcore::hashes::{Hash, sha256};
use dw_engine::DashNetwork;
use dw_vault::{MemoryOsStore, VaultConfig};
use serde_json::{Value, json};
use tauri::Manager;
use tauri::test::{MockRuntime, mock_context, noop_assets};

use crate::launch::LaunchOptions;

const CHILD: &str = "DW_UPGRADE_PATH_CHILD";
const TEST: &str = "upgrade_path_tests::every_release_data_root_opens_with_the_same_answers";
const DONE: &str = "upgrade-path child done";
/// How long one open may take before its child is killed: generous, for hosts
/// and CI runners under heavy load.
const CHILD_DEADLINE: Duration = Duration::from_secs(600);

/// Fields a later build renamed: (command, field in the release's answer,
/// field in this build's answer).
const RENAMED: &[(&str, &str, &str)] = &[("get_network_choice", "launched", "active")];

/// Fields a later build dropped, with why that loses nothing.
const DROPPED: &[(&str, &str, &str)] = &[
    (
        "get_network_choice",
        "restartNeeded",
        "the window switches networks without a restart",
    ),
    (
        "ui_settings",
        "showMasternodes",
        "replaced by masternodesItem: a saved true reads as on (settings.rs tests)",
    ),
];

/// Values each unlock or open sets anew: compared for presence only.
const VOLATILE: &[(&str, &str)] = &[("quick_unlock_policy", "lastPassphraseAt")];

/// Differences that are known defects: (tag, command, path, why). Each must
/// still occur, so a fix removes its line.
const KNOWN: &[(&str, &str, &str, &str)] = &[(
    "v0.1.0",
    "username_in_requests",
    "calls[36] {\"walletId\":\"da8b3e81edb50926c70d3cd3c5de4102fa7c1419b0559fe19d06813c64dcca30\"}: release true, this build false",
    "47a179cf made the unset per-wallet setting read as off; a 0.1.0 wallet that never changed it was on",
)];

/// Releases whose snapshot predates `fundedSignatures` and `dashd.deposits`.
/// Their deposits and dashd's fee are read from the recorded transactions
/// instead ([`dashd_from_transactions`]).
const BEFORE_FUNDED_SIGNATURES: &[&str] = &["v0.1.0"];

/// SHA-256 of each release's committed `expected.json`, with LF line endings.
/// A snapshot stands for what its release answered, so it never changes once
/// committed. The replay asks only what is recorded and compares only the
/// fields that are recorded, so nothing else notices a call or a value that
/// is deleted from it.
const SNAPSHOT_SHA256: &[(&str, &str)] = &[
    (
        "v0.1.0",
        "b322696b5af380e2d2e4012f180b8dbb0b45892ba623219bbe32330e0ff17ef3",
    ),
    (
        "v0.2.0",
        "0c013e154937c2d28112d630173a9588a1e719d1150b983224b791fb2fa4a37d",
    ),
    (
        "v0.3.0",
        "42c4ad4fd72815d379b538ae3aad637965e5253987f63503c455cd2cee440b59",
    ),
];

/// Why `text` is not the snapshot committed for `tag`, if so.
fn snapshot_digest_problem(tag: &str, text: &str) -> Option<String> {
    let digest =
        hex::encode(sha256::Hash::hash(text.replace("\r\n", "\n").as_bytes()).to_byte_array());
    match SNAPSHOT_SHA256.iter().find(|(t, _)| *t == tag) {
        Some((_, pinned)) if *pinned == digest => None,
        Some(_) => Some(format!(
            "expected.json's SHA-256 is {digest}, not the one pinned for {tag}"
        )),
        None => Some(format!("no SHA-256 is pinned for {tag}")),
    }
}

/// Each tag pinned in `SNAPSHOT_SHA256` without its directory under
/// `testdata/upgrade`, and each directory without a pinned tag.
fn pinned_tag_problems(tags: &[String]) -> Vec<String> {
    let mut problems: Vec<String> = SNAPSHOT_SHA256
        .iter()
        .filter(|(pinned, _)| !tags.iter().any(|t| t == pinned))
        .map(|(pinned, _)| format!("{pinned} is pinned but has no fixture directory"))
        .collect();
    problems.extend(
        tags.iter()
            .filter(|t| !SNAPSHOT_SHA256.iter().any(|(pinned, _)| pinned == t))
            .map(|t| format!("{t} has a fixture directory but no pinned SHA-256")),
    );
    problems
}

/// `fixtures_in(root)`, refused unless its directories are exactly the
/// tags pinned in `SNAPSHOT_SHA256`.
fn pinned_fixtures_in(root: &Path) -> Result<Vec<(PathBuf, Value)>, String> {
    let fixtures = fixtures_in(root)?;
    let tags: Vec<String> = fixtures
        .iter()
        .map(|(f, _)| f.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    let unmatched = pinned_tag_problems(&tags);
    if unmatched.is_empty() {
        Ok(fixtures)
    } else {
        Err(unmatched.join("; "))
    }
}

/// Every release directory under `root`, each with a readable, parseable
/// `expected.json`; files beside them (the README) are skipped. Any entry that
/// cannot be read, or a directory without its snapshot, is an error naming it.
fn fixtures_in(root: &Path) -> Result<Vec<(PathBuf, Value)>, String> {
    let entries = std::fs::read_dir(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", root.display()))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if !kind.is_dir() {
            continue;
        }
        let snapshot = path.join("expected.json");
        let text = std::fs::read_to_string(&snapshot)
            .map_err(|e| format!("{}: {e}", snapshot.display()))?;
        let expected =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", snapshot.display()))?;
        found.push((path, expected));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(found)
}

/// What makes a snapshot too thin to stand for its release: no calls, a
/// wallet without exactly one signature, or a wallet `list_wallets` lacks;
/// and, from v0.2.0, a funded address without its signature or a balance
/// that is not the deposits less the payment and its fee.
fn snapshot_problems(tag: &str, expected: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    let wallets: Vec<&Value> = match expected["wallets"].as_array() {
        Some(w) if !w.is_empty() => w.iter().map(|w| &w["walletId"]).collect(),
        _ => vec![],
    };
    if wallets.is_empty() {
        problems.push("no wallets".to_string());
    }
    let calls = expected["calls"].as_array().map_or(&[][..], Vec::as_slice);
    if calls.is_empty() {
        problems.push("no calls".to_string());
    }
    let signatures = expected["signatures"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    if signatures.len() != wallets.len() {
        problems.push(format!(
            "{} signatures for {} wallets",
            signatures.len(),
            wallets.len()
        ));
    }
    let listed = calls
        .iter()
        .find(|c| c["cmd"] == "list_wallets")
        .and_then(|c| c["result"].as_array());
    for id in &wallets {
        if signatures.iter().filter(|s| &&s["walletId"] == id).count() != 1 {
            problems.push(format!("wallet {id} has not exactly one signature"));
        }
    }
    let watch_only = expected["watchOnly"].get("walletId");
    let every_wallet: Vec<&Value> = wallets.iter().copied().chain(watch_only).collect();
    for id in &every_wallet {
        if !listed.is_some_and(|l| l.iter().any(|w| &&w["walletId"] == id)) {
            problems.push(format!("wallet {id} is not in list_wallets"));
        }
    }
    if !BEFORE_FUNDED_SIGNATURES.contains(&tag) {
        let undeclared = listed.map_or(0, |l| {
            l.iter()
                .filter(|w| !every_wallet.contains(&&w["walletId"]))
                .count()
        });
        if undeclared != 0 {
            problems.push(format!(
                "list_wallets has {undeclared} wallets the snapshot does not declare"
            ));
        }
        let mut named = Vec::new();
        for call in calls {
            named_wallets(&call["args"], &mut named);
            named_wallets(&call["result"], &mut named);
        }
        let stray = named
            .into_iter()
            .filter(|id| !every_wallet.contains(id))
            .count();
        if stray != 0 {
            problems.push(format!(
                "the recorded calls name a wallet the snapshot does not declare, {stray} times"
            ));
        }
        problems.extend(funded_signature_problems(&wallets, calls, expected));
    }
    match dashd_figures(tag, expected) {
        Ok(dashd) => problems.extend(balance_problems(&every_wallet, calls, &dashd)),
        Err(e) => problems.push(e),
    }
    problems
}

/// dashd's deposits and its fee for the payment: the snapshot's own
/// `dashd` from v0.2.0, read from the recorded transactions before.
fn dashd_figures(tag: &str, expected: &Value) -> Result<Value, String> {
    if BEFORE_FUNDED_SIGNATURES.contains(&tag) {
        dashd_from_transactions(expected)
    } else {
        Ok(expected["dashd"].clone())
    }
}

/// The transaction `txid` from its recorded `tx_detail` answer, whose
/// `rawHex` must hash to `txid`.
fn recorded_tx(calls: &[Value], txid: &str) -> Result<dashcore::Transaction, String> {
    let hex = calls
        .iter()
        .find(|c| c["cmd"] == "tx_detail" && c["args"]["txid"] == txid)
        .and_then(|c| c["result"]["rawHex"].as_str())
        .ok_or_else(|| format!("no recorded rawHex for {txid}"))?;
    let bytes = hex::decode(hex).map_err(|e| format!("rawHex of {txid}: {e}"))?;
    let tx: dashcore::Transaction =
        dashcore::consensus::deserialize(&bytes).map_err(|e| format!("rawHex of {txid}: {e}"))?;
    if tx.txid().to_string() != txid {
        return Err(format!("the rawHex recorded for {txid} is {}", tx.txid()));
    }
    Ok(tx)
}

/// `dashd.deposits` and `dashd.payment` with its `walletId` and `feeDuffs`,
/// for a snapshot that records neither: from the transactions dashd named.
/// Each must be a recorded `tx_detail` answer whose `rawHex` hashes to that
/// txid. A wallet's deposits are the funding outputs to its recorded
/// `addresses`; the payment spends funding outputs of one wallet, the payer,
/// pays `dashd.payment.to` its `duffs`, and its fee is what it spends less
/// what it pays out.
fn dashd_from_transactions(expected: &Value) -> Result<Value, String> {
    let calls = expected["calls"].as_array().map_or(&[][..], Vec::as_slice);
    let dashd = &expected["dashd"];
    let raw = |txid: &str| recorded_tx(calls, txid);
    let mut owner = std::collections::HashMap::new();
    for wallet in expected["wallets"]
        .as_array()
        .map_or(&[][..], Vec::as_slice)
    {
        let id = wallet["walletId"]
            .as_str()
            .ok_or("a wallet without walletId")?;
        let listed = first_answer(calls, "addresses", &wallet["walletId"])
            .and_then(Value::as_array)
            .ok_or_else(|| format!("wallet {id}: no addresses answer"))?;
        for address in listed.iter().filter_map(|a| a["address"].as_str()) {
            owner.insert(address.to_string(), id.to_string());
        }
    }
    let address_of = |script: &dashcore::ScriptBuf| {
        dashcore::Address::from_script(script, dashcore::Network::Regtest)
            .map(|a| a.to_string())
            .ok()
    };
    let funding = dashd["funding"]
        .as_object()
        .filter(|f| !f.is_empty())
        .ok_or("no dashd.funding")?;
    let mut deposits = serde_json::Map::new();
    let mut funded = std::collections::HashMap::new();
    for (name, txid) in funding {
        let txid = txid
            .as_str()
            .ok_or_else(|| format!("dashd.funding.{name}"))?;
        let tx = raw(txid)?;
        let mut credited = 0;
        for (vout, out) in tx.output.iter().enumerate() {
            let Some(id) = address_of(&out.script_pubkey).and_then(|a| owner.get(&a)) else {
                continue;
            };
            credited += 1;
            funded.insert((txid.to_string(), vout as u32), (id.clone(), out.value));
            let sum = deposits.entry(id.clone()).or_insert(json!(0));
            *sum = json!(
                sum.as_u64()
                    .unwrap()
                    .checked_add(out.value)
                    .ok_or("deposits overflow")?
            );
        }
        if credited != 1 {
            return Err(format!(
                "funding {name} ({txid}) pays {credited} recorded wallet addresses, not one"
            ));
        }
    }
    let payment = &dashd["payment"];
    let (Some(txid), Some(to), Some(paid)) = (
        payment["txid"].as_str(),
        payment["to"].as_str(),
        payment["duffs"].as_u64(),
    ) else {
        return Err("no dashd.payment txid, to and duffs".to_string());
    };
    let tx = raw(txid)?;
    let mut payer = None;
    let mut spent = 0u64;
    for input in &tx.input {
        let at = (
            input.previous_output.txid.to_string(),
            input.previous_output.vout,
        );
        let (id, value) = funded
            .get(&at)
            .ok_or_else(|| format!("the payment spends {}:{}, no funding output", at.0, at.1))?;
        if *payer.get_or_insert(id) != id {
            return Err("the payment spends coins of two wallets".to_string());
        }
        spent = spent.checked_add(*value).ok_or("inputs overflow")?;
    }
    let paid_out = tx
        .output
        .iter()
        .try_fold(0u64, |sum, o| sum.checked_add(o.value))
        .ok_or("outputs overflow")?;
    if !tx
        .output
        .iter()
        .any(|o| o.value == paid && address_of(&o.script_pubkey).as_deref() == Some(to))
    {
        return Err(format!("the payment {txid} does not pay {to} {paid} duffs"));
    }
    let fee = spent
        .checked_sub(paid_out)
        .ok_or("the payment pays out more than it spends")?;
    Ok(json!({
        "deposits": deposits,
        "payment": { "txid": txid, "to": to, "duffs": paid, "walletId": payer.ok_or("the payment has no inputs")?, "feeDuffs": fee },
    }))
}

/// Every wallet id a recorded call names, in its arguments or its answer:
/// each `walletId` at any depth, and the ids in a `walletsWithSecrets` list.
fn named_wallets<'a>(value: &'a Value, found: &mut Vec<&'a Value>) {
    match value {
        Value::Object(fields) => {
            for (key, inner) in fields {
                match (key.as_str(), inner) {
                    ("walletId", id) => found.push(id),
                    ("walletsWithSecrets", Value::Array(ids)) => found.extend(ids),
                    _ => named_wallets(inner, found),
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                named_wallets(item, found);
            }
        }
        _ => {}
    }
}

fn first_answer<'a>(calls: &'a [Value], cmd: &str, wallet: &Value) -> Option<&'a Value> {
    calls
        .iter()
        .find(|c| c["cmd"] == cmd && &c["args"]["walletId"] == wallet)
        .map(|c| &c["result"])
}

/// The `utxos` answer that lists every coin of `wallet`: the one asked with
/// the writer's filter, wherever a narrower one stands among the calls.
fn every_coin<'a>(calls: &'a [Value], wallet: &Value) -> Option<&'a [Value]> {
    let all = json!({ "includeLocked": true, "fullyMixedOnly": false, "minConfirmations": null });
    calls
        .iter()
        .find(|c| {
            c["cmd"] == "utxos" && &c["args"]["walletId"] == wallet && c["args"]["filter"] == all
        })
        .and_then(|c| c["result"].as_array())
        .map(Vec::as_slice)
}

/// Each wallet's distinct `utxos` addresses, change included, against the
/// addresses of its `fundedSignatures`. A recorded coin without an address
/// is a problem: it would otherwise drop out of what must be signed.
fn funded_signature_problems(wallets: &[&Value], calls: &[Value], expected: &Value) -> Vec<String> {
    let Some(funded) = expected["fundedSignatures"].as_array() else {
        return vec!["no fundedSignatures".to_string()];
    };
    let mut problems = Vec::new();
    for id in wallets {
        let Some(coins) = every_coin(calls, id) else {
            problems.push(format!("wallet {id}: no utxos answer for every coin"));
            continue;
        };
        let mut want: Vec<&str> = coins
            .iter()
            .filter_map(|u| u["address"].as_str())
            .filter(|a| !a.is_empty())
            .collect();
        if want.len() != coins.len() {
            problems.push(format!("wallet {id}: a recorded coin has no address"));
        }
        want.sort_unstable();
        want.dedup();
        let mut got: Vec<&str> = funded
            .iter()
            .filter(|s| &&s["walletId"] == id)
            .filter_map(|s| s["address"].as_str())
            .collect();
        got.sort_unstable();
        if want.is_empty() || want != got {
            problems.push(format!(
                "wallet {id}: funded addresses {want:?}, signed by {got:?}"
            ));
        }
    }
    problems
}

/// `dashd.deposits` names exactly the fixture's wallets, the paying wallet
/// among them, and each reports, as `balance.confirmed`, its deposits less
/// what it paid and dashd's fee for that payment. Each wallet's recorded
/// coins add up to its `balance.total`, and no balance is recorded for a
/// wallet without a deposit.
fn balance_problems(wallets: &[&Value], calls: &[Value], dashd: &Value) -> Vec<String> {
    let Some(deposits) = dashd["deposits"].as_object().filter(|d| !d.is_empty()) else {
        return vec!["no dashd.deposits".to_string()];
    };
    let payment = &dashd["payment"];
    if payment["duffs"].as_u64().is_none() || payment["feeDuffs"].as_u64().is_none() {
        return vec!["no dashd.payment.duffs and feeDuffs".to_string()];
    }
    let mut problems = Vec::new();
    let mut want: Vec<&str> = wallets.iter().filter_map(|w| w.as_str()).collect();
    want.sort_unstable();
    let mut got: Vec<&str> = deposits.keys().map(String::as_str).collect();
    got.sort_unstable();
    if want.len() != wallets.len() || want != got {
        problems.push(format!(
            "dashd.deposits names {got:?}, the fixture's wallets are {want:?}"
        ));
    }
    let payer = &payment["walletId"];
    if !payer.as_str().is_some_and(|p| deposits.contains_key(p)) {
        problems.push(format!("the paying wallet {payer} has no deposit"));
    }
    for id in deposits.keys() {
        let id = Value::String(id.clone());
        let want = exact_balance(dashd, &id);
        let got = first_answer(calls, "balance", &id).and_then(|b| b["confirmed"].as_u64());
        if want.is_none() || want != got {
            problems.push(format!(
                "wallet {id}: balance {got:?}, dashd's deposits less payment and fee {want:?}"
            ));
        }
        let total = first_answer(calls, "balance", &id).and_then(|b| b["total"].as_u64());
        let coins = every_coin(calls, &id).and_then(|coins| {
            coins
                .iter()
                .try_fold(0u64, |sum, u| sum.checked_add(u["amount"].as_u64()?))
        });
        if coins.is_none() || coins != total {
            problems.push(format!(
                "wallet {id}: its recorded coins add up to {coins:?}, its balance is {total:?}"
            ));
        }
    }
    for call in calls.iter().filter(|c| c["cmd"] == "balance") {
        let id = &call["args"]["walletId"];
        if !id.as_str().is_some_and(|i| deposits.contains_key(i)) {
            problems.push(format!(
                "a balance is recorded for wallet {id}, which has no deposit"
            ));
        }
    }
    problems
}

/// What `wallet` must report as `balance.confirmed`: its deposits, less the
/// payment and dashd's fee for it when it paid.
fn exact_balance(dashd: &Value, wallet: &Value) -> Option<u64> {
    let payment = &dashd["payment"];
    let spent = if &payment["walletId"] == wallet {
        payment["duffs"]
            .as_u64()?
            .checked_add(payment["feeDuffs"].as_u64()?)?
    } else {
        0
    };
    dashd["deposits"][wallet.as_str()?]
        .as_u64()?
        .checked_sub(spent)
}

/// Each open's `spends` against the release's coins: one per wallet with
/// keys, its inputs signed by exactly the wallet's distinct recorded coin
/// addresses (change and user-locked coins included), each user-locked coin
/// locked again, and the balance after the abandoned spend still dashd's
/// exact figure.
fn spend_problems(tag: &str, expected: &Value, answers: &Value) -> Vec<String> {
    let calls = expected["calls"].as_array().map_or(&[][..], Vec::as_slice);
    let dashd = match dashd_figures(tag, expected) {
        Ok(d) => d,
        Err(e) => return vec![e],
    };
    let spends = answers["spends"].as_array().map_or(&[][..], Vec::as_slice);
    let wallets = expected["wallets"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    let mut problems = Vec::new();
    if spends.len() != wallets.len() {
        problems.push(format!(
            "{} spends for {} wallets with keys",
            spends.len(),
            wallets.len()
        ));
    }
    for wallet in wallets {
        let id = &wallet["walletId"];
        let Some(spend) = spends.iter().find(|s| &s["walletId"] == id) else {
            problems.push(format!("wallet {id}: no spend"));
            continue;
        };
        let coins = every_coin(calls, id).unwrap_or_default();
        let mut want: Vec<&str> = coins.iter().filter_map(|c| c["address"].as_str()).collect();
        want.sort_unstable();
        want.dedup();
        let got: Vec<&str> = spend["addresses"]
            .as_array()
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .filter_map(Value::as_str)
            .collect();
        if want.is_empty() || got != want || spend["inputs"].as_u64() != Some(coins.len() as u64) {
            problems.push(format!(
                "wallet {id}: the spend signed {} inputs by {got:?}, the release recorded {} coins at {want:?}",
                spend["inputs"],
                coins.len()
            ));
        }
        let locked = coins.iter().filter(|c| c["userLocked"] == true).count() as u64;
        if spend["relocked"].as_u64() != Some(locked) {
            problems.push(format!(
                "wallet {id}: {} coins locked again, the release recorded {locked} locked",
                spend["relocked"]
            ));
        }
        let want_balance = exact_balance(&dashd, id);
        if want_balance.is_none() || spend["balanceAfter"].as_u64() != want_balance {
            problems.push(format!(
                "wallet {id}: balance {} after the abandoned spend, dashd's figure {want_balance:?}",
                spend["balanceAfter"]
            ));
        }
    }
    problems
}

fn copy_private(from: &Path, to: &Path) {
    #[allow(unused_mut)]
    let mut mkdir = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut mkdir, 0o700);
    mkdir.create(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type().unwrap().is_dir() {
            copy_private(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
    }
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn every_release_data_root_opens_with_the_same_answers() {
    if let Some(dir) = std::env::var_os(CHILD) {
        child(Path::new(&dir));
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/upgrade");
    let fixtures = pinned_fixtures_in(&root).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        !fixtures.is_empty(),
        "no testdata/upgrade/<tag>/expected.json"
    );
    let mut report = Vec::new();
    for (fixture, expected) in &fixtures {
        let tag = fixture.file_name().unwrap().to_string_lossy().into_owned();
        let mut problems = snapshot_problems(&tag, expected);
        let text = std::fs::read_to_string(fixture.join("expected.json")).unwrap();
        problems.extend(snapshot_digest_problem(&tag, &text));
        assert!(
            problems.is_empty(),
            "{tag}: expected.json is incomplete: {}",
            problems.join("; ")
        );
        let dir = dw_testutil::private_tempdir();
        copy_private(&fixture.join("data"), &dir.path().join("data"));
        std::fs::copy(
            fixture.join("expected.json"),
            dir.path().join("expected.json"),
        )
        .unwrap();
        let first = open_in_child(&tag, dir.path(), 1);
        let second = open_in_child(&tag, dir.path(), 2);
        let mut found = Vec::new();
        found.extend(replay_problems(&tag, expected, &first, &second));
        compare_with_release(expected, &first, &mut found);
        found.extend(
            compare_opens(&first, &second)
                .into_iter()
                .map(|(cmd, d)| (format!("{cmd} (second open)"), d)),
        );
        for (n, answers) in [(1, &first), (2, &second)] {
            found.extend(
                spend_problems(&tag, expected, answers)
                    .into_iter()
                    .map(|d| (format!("draft_prepare (open {n})"), d)),
            );
        }
        let known: Vec<_> = KNOWN.iter().filter(|k| k.0 == tag).collect();
        for k in &known {
            assert!(
                found
                    .iter()
                    .any(|(cmd, path)| cmd == k.1 && path.starts_with(k.2)),
                "{tag}: the known defect at {} {} no longer occurs ({}): remove it from KNOWN",
                k.1,
                k.2,
                k.3
            );
        }
        found.retain(|(cmd, path)| !known.iter().any(|k| cmd == k.1 && path.starts_with(k.2)));
        report.extend(
            found
                .into_iter()
                .map(|(cmd, d)| format!("{tag}: {cmd}: {d}")),
        );
    }
    assert!(
        report.is_empty(),
        "this build answers differently from the release that wrote the data root:\n{}",
        report.join("\n")
    );
}

/// Releases whose wallets with keys are replayed from their birth at the
/// first open: 0.1.0 and 0.2.0 scanned with narrow CoinJoin pools and kept
/// no marker; 0.3.0's `done` marker could be written without that replay.
const NARROW_SCAN_RELEASES: &[&str] = &["v0.1.0", "v0.2.0", "v0.3.0"];

/// Each wallet with keys of a narrow-scan release replayed at the first
/// open, and nothing replayed at the second.
fn replay_problems(
    tag: &str,
    expected: &Value,
    first: &Value,
    second: &Value,
) -> Vec<(String, String)> {
    let replayed = |answers: &Value, key: &str| -> Vec<String> {
        (answers[key]
            .as_array()
            .map_or(&[][..], Vec::as_slice)
            .iter())
        .filter_map(|id| id.as_str().map(str::to_ascii_lowercase))
        .collect()
    };
    let mut found = Vec::new();
    if NARROW_SCAN_RELEASES.contains(&tag) {
        let first = replayed(first, "replayed");
        for wallet in expected["wallets"]
            .as_array()
            .map_or(&[][..], Vec::as_slice)
        {
            let id = wallet["walletId"].as_str().unwrap().to_ascii_lowercase();
            if !first.contains(&id) {
                found.push((
                    "open 1".to_string(),
                    format!("wallet {id} was not replayed"),
                ));
            }
        }
    }
    let again = replayed(second, "replayedAny");
    if !again.is_empty() {
        found.push(("open 2".to_string(), format!("replayed again: {again:?}")));
    }
    found
}

fn open_in_child(tag: &str, dir: &Path, n: u32) -> Value {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", TEST, "--nocapture", "--test-threads=1"])
        .env(CHILD, dir)
        .env("DW_UPGRADE_PATH_OPEN", n.to_string());
    let (status, stdout, stderr) =
        run_with_deadline(cmd, CHILD_DEADLINE).unwrap_or_else(|e| panic!("{tag}: open {n}: {e}"));
    assert!(
        status.success() && stdout.contains(DONE),
        "{tag}: open {n} failed: {status}\n{stdout}\n{stderr}"
    );
    read_json(&dir.join(format!("answers-{n}.json")))
}

/// Runs `cmd` with its output drained on threads; past `deadline` it is
/// killed and reaped, and the error carries what it printed.
fn run_with_deadline(
    mut cmd: Command,
    deadline: Duration,
) -> Result<(ExitStatus, String, String), String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start: {e}"))?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < deadline => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Err(e) => return Err(format!("could not wait: {e}")),
        }
    };
    let (stdout, stderr) = (stdout.join().unwrap(), stderr.join().unwrap());
    match status {
        Some(status) => Ok((status, stdout, stderr)),
        None => Err(format!(
            "killed after {deadline:?} without finishing\n{stdout}\n{stderr}"
        )),
    }
}

fn compare_with_release(expected: &Value, answers: &Value, found: &mut Vec<(String, String)>) {
    let want = expected["calls"].as_array().unwrap();
    let got = answers["calls"].as_array().unwrap();
    assert_eq!(want.len(), got.len());
    for (i, (w, g)) in want.iter().zip(got).enumerate() {
        let cmd = w["cmd"].as_str().unwrap();
        let path = format!("calls[{i}] {}", w["args"]);
        let rise = (w["args"]["walletId"].as_str())
            .filter(|_| cmd == "utxos")
            .and_then(|id| answers["processedRise"][id.to_ascii_lowercase()].as_u64())
            .unwrap_or(0);
        let mut g = g.clone();
        lower_confirmations(&mut g, rise);
        found.extend(
            answer_diffs(cmd, w, &g, &path)
                .into_iter()
                .map(|d| (cmd.to_string(), d)),
        );
    }
    for key in SIGNATURES {
        if expected[key] != answers[key] {
            found.push((
                "sign_message".into(),
                format!(
                    "{key}: release signed {}, this build {}",
                    expected[key], answers[key]
                ),
            ));
        }
    }
}

const SIGNATURES: [&str; 2] = ["signatures", "fundedSignatures"];

/// Takes `rise` off every confirmed count in a `utxos` answer, which counts
/// from the wallet's processed height: the replay a first open ends offline
/// raises it to the replay's target, where the release's stood lower, as
/// SPV's would. History counts from the filter checkpoint, which is back
/// where the release had it.
fn lower_confirmations(answer: &mut Value, rise: u64) {
    match answer {
        Value::Object(fields) => {
            for (key, value) in fields.iter_mut() {
                match value.as_u64() {
                    Some(n) if key == "confirmations" && n > 0 => {
                        *value = json!(n.saturating_sub(rise))
                    }
                    _ => lower_confirmations(value, rise),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| lower_confirmations(v, rise)),
        _ => {}
    }
}

/// The second open's answers against the first's, call by call: equal once
/// the [`VOLATILE`] values are reduced to whether they are set.
fn compare_opens(first: &Value, second: &Value) -> Vec<(String, String)> {
    let a = first["calls"].as_array().unwrap();
    let b = second["calls"].as_array().unwrap();
    let mut found = Vec::new();
    if a.len() != b.len() {
        found.push((
            "calls".to_string(),
            format!("{} answers, then {}", a.len(), b.len()),
        ));
    }
    for (i, (a, b)) in a.iter().zip(b).enumerate() {
        let cmd = a["cmd"].as_str().unwrap();
        let path = format!("calls[{i}] {}", a["args"]);
        let (a, b) = (stable(cmd, a), stable(cmd, b));
        if a.get("error").is_some() || b.get("error").is_some() {
            found.extend(
                answer_diffs(cmd, &a, &b, &path)
                    .into_iter()
                    .map(|d| (cmd.to_string(), d)),
            );
        } else {
            let mut diffs = Vec::new();
            equal(&a["result"], &b["result"], OPENS, &path, &mut diffs);
            found.extend(diffs.into_iter().map(|d| (cmd.to_string(), d)));
        }
    }
    for key in SIGNATURES {
        if first[key] != second[key] {
            found.push(("sign_message".into(), format!("{key} differs")));
        }
    }
    found
}

/// `value` with each [`VOLATILE`] field of `cmd` replaced by whether it is set.
fn stable(cmd: &str, value: &Value) -> Value {
    match value {
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| {
                    let v = if VOLATILE.iter().any(|f| f.0 == cmd && f.1 == k) {
                        Value::Bool(!v.is_null())
                    } else {
                        stable(cmd, v)
                    };
                    (k.clone(), v)
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(|v| stable(cmd, v)).collect()),
        other => other.clone(),
    }
}

/// The two sides [`equal`] names in its differences, for the two opens.
const OPENS: [&str; 2] = ["on the first open", "on the second open"];

/// Every difference between `a` and `b`, which `sides` names: a field on one
/// side only, an array of another length, or another value.
fn equal(a: &Value, b: &Value, sides: [&str; 2], path: &str, diffs: &mut Vec<String>) {
    let [first, second] = sides;
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (key, xv) in x {
                match y.get(key) {
                    Some(yv) => equal(xv, yv, sides, &format!("{path}.{key}"), diffs),
                    None => diffs.push(format!("{path}.{key}: only {first} ({xv})")),
                }
            }
            for (key, yv) in y {
                if !x.contains_key(key) {
                    diffs.push(format!("{path}.{key}: only {second} ({yv})"));
                }
            }
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            for (i, (xv, yv)) in x.iter().zip(y).enumerate() {
                equal(xv, yv, sides, &format!("{path}[{i}]"), diffs);
            }
        }
        (x, y) if x == y => {}
        (x, y) => diffs.push(format!("{path}: {first} {x}, {second} {y}")),
    }
}

/// `got` answers a call as `want` did: a result containing `want`'s, or, when
/// either failed, the same failure on both sides counts as a difference too.
fn answer_diffs(cmd: &str, want: &Value, got: &Value, path: &str) -> Vec<String> {
    let mut diffs = Vec::new();
    match (want.get("error"), got.get("error")) {
        (None, None) => contains(cmd, &want["result"], &got["result"], path, &mut diffs),
        (_, Some(e)) => diffs.push(format!("{path}: fails with {e}")),
        (Some(e), None) => diffs.push(format!("{path}: failed before with {e}")),
    }
    diffs
}

/// Every field of `want` is in `got` with an equal value, recursively;
/// arrays match element by element. Differences go to `diffs`.
fn contains(cmd: &str, want: &Value, got: &Value, path: &str, diffs: &mut Vec<String>) {
    match (want, got) {
        (Value::Object(w), Value::Object(g)) => {
            for (key, wv) in w {
                if DROPPED.iter().any(|d| d.0 == cmd && d.1 == key) {
                    continue;
                }
                let name = RENAMED
                    .iter()
                    .find(|r| r.0 == cmd && r.1 == key)
                    .map_or(key.as_str(), |r| r.2);
                let at = format!("{path}.{key}");
                match g.get(name) {
                    None => diffs.push(format!("{at}: missing (release: {wv})")),
                    Some(gv) if VOLATILE.iter().any(|v| v.0 == cmd && v.1 == key) => {
                        if wv.is_null() != gv.is_null() {
                            diffs.push(format!("{at}: release {wv}, this build {gv}"));
                        }
                    }
                    Some(gv) => contains(cmd, wv, gv, &at, diffs),
                }
            }
        }
        (Value::Array(w), Value::Array(g)) if w.len() == g.len() => {
            for (i, (wv, gv)) in w.iter().zip(g).enumerate() {
                contains(cmd, wv, gv, &format!("{path}[{i}]"), diffs);
            }
        }
        (w, g) if w == g => {}
        (w, g) => diffs.push(format!("{path}: release {w}, this build {g}")),
    }
}

fn child(dir: &Path) {
    let n = std::env::var("DW_UPGRADE_PATH_OPEN").unwrap();
    let expected = read_json(&dir.join("expected.json"));
    let root = dir.join("data");
    let network = match expected["network"].as_str().unwrap() {
        "regtest" => DashNetwork::Regtest,
        other => panic!("fixture network {other}"),
    };
    let launch = LaunchOptions {
        data_dir: Some(root.clone()),
        network: network.clone(),
        network_given: true,
        ..LaunchOptions::default()
    };
    let vault = VaultConfig {
        os_store: Arc::new(MemoryOsStore::new()),
        ..VaultConfig::default()
    };
    let mut app = crate::builder_with_vault::<MockRuntime>(launch, Instant::now(), vault)
        .enable_macos_default_menu(false)
        .build(mock_context(noop_assets()))
        .expect("app");
    #[allow(deprecated)]
    app.run_iteration(|_, _| {});
    let state = app.state::<crate::state::AppState>();
    tauri::async_runtime::block_on(state.engine.wait()).expect("the engine opens the data root");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("webview");
    let call = |cmd: &str, args: Value| {
        crate::ipc_tests::invoke(&webview, app.invoke_key(), cmd, args).map_err(|e| {
            e.to_string()
                .replace(&*root.to_string_lossy(), "$DATA_ROOT")
        })
    };
    let ok = |cmd: &str, args: Value| call(cmd, args).unwrap_or_else(|e| panic!("{cmd}: {e}"));
    ok(
        "open_network",
        json!({ "settings": crate::ipc_tests::offline_settings() }),
    );
    let passphrase = expected["passphrase"].as_str().unwrap();
    ok("unlock", json!({ "passphrase": passphrase }));
    // An older build's wallets are replayed once from their birth at the
    // first open, which a data root opened offline cannot run: ended as SPV
    // ends one that finds nothing new, so the answers compare.
    let finished = {
        let session = tauri::async_runtime::block_on(state.session()).expect("the session");
        tauri::async_runtime::block_on(dw_engine::test_support::finish_replays(&session))
    };
    let replayed: Vec<String> = (finished.iter())
        .filter(|f| f.from_birth)
        .map(|f| f.wallet.to_string())
        .collect();
    let replayed_any: Vec<String> = finished.iter().map(|f| f.wallet.to_string()).collect();
    let processed_rise: serde_json::Map<String, Value> = (finished.iter())
        .map(|f| (f.wallet.to_string(), json!(f.processed_rise)))
        .collect();

    let root_text = serde_json::to_string(&root.to_string_lossy()).unwrap();
    let root_text = &root_text[1..root_text.len() - 1];
    let calls: Vec<Value> = expected["calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let cmd = c["cmd"].as_str().unwrap();
            match call(cmd, c["args"].clone()) {
                Ok(result) => {
                    let text = result.to_string().replace(root_text, "$DATA_ROOT");
                    json!({ "cmd": cmd, "args": c["args"], "result": serde_json::from_str::<Value>(&text).unwrap() })
                }
                Err(error) => json!({ "cmd": cmd, "args": c["args"], "error": error }),
            }
        })
        .collect();
    let sign_again = |key: &str| -> Vec<Value> {
        expected[key]
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .map(|s| {
            let wallet_id = &s["walletId"];
            let grant = ok(
                "authorize",
                json!({
                    "purpose": { "kind": "signMessage" },
                    "walletId": wallet_id,
                    "credential": { "kind": "passphrase", "passphrase": passphrase },
                }),
            );
            let signature = ok(
                "sign_message",
                json!({ "walletId": wallet_id, "address": s["address"], "message": s["message"], "grantId": grant["id"] }),
            );
            let verified = ok(
                "verify_message",
                json!({ "address": s["address"], "signature": signature, "message": s["message"] }),
            );
            assert_eq!(verified["outcome"], "verified", "{verified}");
            json!({ "walletId": wallet_id, "address": s["address"], "message": s["message"], "signature": signature })
        })
        .collect()
    };
    let signatures = sign_again("signatures");
    let funded = expected
        .get("fundedSignatures")
        .map(|_| Value::Array(sign_again("fundedSignatures")));
    let prepared_raw = |handle: u64| {
        let (_, tx) = state
            .sends
            .prepared(handle)
            .expect("the prepared transaction");
        tx.raw().expect("a signed transaction")
    };
    let sent_marks = |wallet_id: &Value| {
        let session = tauri::async_runtime::block_on(state.session()).expect("the session");
        let wallet = wallet_id.as_str().unwrap().parse().expect("a wallet id");
        dw_engine::test_support::sent_marks(&session, wallet)
    };
    let spends: Vec<Value> = expected["wallets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| {
            let probes = SpendProbes {
                prepared_raw: &prepared_raw,
                sent_marks: &sent_marks,
            };
            spend_every_recorded_coin(&expected, &w["walletId"], passphrase, &ok, &probes)
        })
        .collect();
    tauri::async_runtime::block_on(state.shutdown_engine()).expect("engine shutdown");
    let marker = root.join(network.dir_name()).join(".session-open");
    assert!(
        !marker.try_exists().expect("check the open-session marker"),
        "open {n} left an unclean session: {}",
        marker.display()
    );
    std::fs::write(
        dir.join(format!("answers-{n}.json")),
        serde_json::to_string_pretty(
            &json!({ "calls": calls, "signatures": signatures, "fundedSignatures": funded, "spends": spends, "replayed": replayed, "replayedAny": replayed_any, "processedRise": processed_rise }),
        )
        .unwrap(),
    )
    .unwrap();
    println!("{DONE}");
}

/// The wallet's state that a spend that is prepared and abandoned must leave
/// as it was: this open's answers to the recorded `balance`, `utxos`,
/// `locked_outpoints` and `history_page` calls of the wallet.
fn spend_state(expected: &Value, wallet_id: &Value, ok: &dyn Fn(&str, Value) -> Value) -> Value {
    Value::Array(
        expected["calls"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| {
                &c["args"]["walletId"] == wallet_id
                    && ["balance", "utxos", "locked_outpoints", "history_page"]
                        .contains(&c["cmd"].as_str().unwrap())
            })
            .map(|c| json!({ "cmd": c["cmd"], "args": c["args"], "result": ok(c["cmd"].as_str().unwrap(), c["args"].clone()) }))
            .collect(),
    )
}

/// What a child reads of its session beside the IPC answers: a prepared
/// transaction's signed bytes, and the txids of a wallet's durable sent
/// marks.
struct SpendProbes<'a> {
    prepared_raw: &'a dyn Fn(u64) -> Vec<u8>,
    sent_marks: &'a dyn Fn(&Value) -> Vec<String>,
}

/// Builds and signs, through `draft_prepare` (which never broadcasts), one
/// transaction that spends every coin the release recorded for the wallet,
/// change and user-locked coins included (a locked coin is unlocked for it
/// and locked again). Each input must be one of those coins, and must spend
/// the output its parent's recorded `rawHex` holds at that outpoint, to the
/// coin's address and of its amount, signed by that address's key over the
/// legacy sighash. The transaction is then abandoned. Before the coins are
/// locked again, the same coins must sign again (key-wallet's builder and
/// the engine leave out a reserved coin) and no sent mark may name either
/// transaction. After it the wallet's [`spend_state`] must be what it was
/// before: the coins locked again, none reserved, no row in the history. A
/// lock made again has a new time, which `locked_outpoints` does not show;
/// no release records more than one lock in a wallet, so the order of its
/// locks cannot change.
fn spend_every_recorded_coin(
    expected: &Value,
    wallet_id: &Value,
    passphrase: &str,
    ok: &dyn Fn(&str, Value) -> Value,
    probes: &SpendProbes,
) -> Value {
    let calls = expected["calls"].as_array().unwrap();
    let coins = every_coin(calls, wallet_id).expect("every coin of the wallet is recorded");
    assert!(!coins.is_empty(), "wallet {wallet_id} has no recorded coin");
    let outpoint = |c: &Value| {
        (
            c["outpoint"]["txid"].as_str().unwrap().to_string(),
            c["outpoint"]["vout"].as_u64().unwrap() as u32,
        )
    };
    let outpoints: Vec<Value> = coins.iter().map(|c| c["outpoint"].clone()).collect();
    let locked: Vec<Value> = coins
        .iter()
        .filter(|c| c["userLocked"] == true)
        .map(|c| c["outpoint"].clone())
        .collect();
    let total = coins
        .iter()
        .try_fold(0u64, |sum, c| sum.checked_add(c["amount"].as_u64()?))
        .expect("the recorded coins add up");
    let to = first_answer(calls, "current_receive_address", wallet_id)
        .expect("a recorded receive address")["address"]
        .clone();
    let mut want: Vec<(String, u32)> = coins.iter().map(outpoint).collect();
    want.sort_unstable();
    let prepare = || {
        let draft = ok("new_tx_draft", json!({ "walletId": wallet_id }));
        ok(
            "draft_set_source",
            json!({ "draft": draft, "source": { "kind": "outpoints", "outpoints": outpoints } }),
        );
        ok(
            "draft_set_fee",
            json!({ "draft": draft, "fee": { "kind": "perKb", "duffsPerKb": 1000 } }),
        );
        ok(
            "draft_set_recipients",
            json!({ "draft": draft, "recipients": [{ "address": to, "amount": total, "subtractFeeFromAmount": true, "label": null, "message": null }] }),
        );
        let grant = ok(
            "authorize",
            json!({
                "purpose": { "kind": "spend", "maxDuffs": total },
                "walletId": wallet_id,
                "credential": { "kind": "passphrase", "passphrase": passphrase },
            }),
        );
        let prepared = ok(
            "draft_prepare",
            json!({ "draft": draft, "grantId": grant["id"] }),
        );
        let handle = prepared["handle"].as_u64().unwrap();
        let tx: dashcore::Transaction =
            dashcore::consensus::deserialize(&(probes.prepared_raw)(handle))
                .expect("the signed transaction decodes");
        let mut got: Vec<(String, u32)> = tx
            .input
            .iter()
            .map(|i| (i.previous_output.txid.to_string(), i.previous_output.vout))
            .collect();
        got.sort_unstable();
        assert_eq!(
            got, want,
            "wallet {wallet_id}: the spend's inputs are not the recorded coins"
        );
        (draft, handle, tx)
    };
    let abandon = |draft: &Value, handle: u64| {
        ok(
            "draft_abandon",
            json!({ "draft": draft, "prepared": handle }),
        );
        ok("close_draft", json!({ "draft": draft }));
    };

    let before = spend_state(expected, wallet_id, ok);
    let marks_before = (probes.sent_marks)(wallet_id);
    if !locked.is_empty() {
        ok(
            "unlock_outpoints",
            json!({ "walletId": wallet_id, "outpoints": locked }),
        );
    }
    let (draft, handle, tx) = prepare();
    let mut addresses = Vec::new();
    for (index, input) in tx.input.iter().enumerate() {
        let at = (
            input.previous_output.txid.to_string(),
            input.previous_output.vout,
        );
        let coin = coins.iter().find(|c| outpoint(c) == at).unwrap();
        let address = coin["address"].as_str().unwrap();
        let parent = recorded_tx(calls, &at.0)
            .unwrap_or_else(|e| panic!("wallet {wallet_id}: input {index}: {e}"));
        let spent = parent.output.get(at.1 as usize).unwrap_or_else(|| {
            panic!(
                "wallet {wallet_id}: input {index}: {} has no output {}",
                at.0, at.1
            )
        });
        let recorded = address
            .parse::<dashcore::Address<_>>()
            .expect("a recorded address")
            .require_network(dashcore::Network::Regtest)
            .expect("a regtest address")
            .script_pubkey();
        assert!(
            spent.script_pubkey == recorded && Some(spent.value) == coin["amount"].as_u64(),
            "wallet {wallet_id}: input {index}: the output it spends pays {} duffs to {:?}, the recorded coin {} to {address}",
            spent.value,
            spent.script_pubkey,
            coin["amount"]
        );
        if let Some(problem) = p2pkh_input_problem(&tx, index, &spent.script_pubkey) {
            panic!("wallet {wallet_id}: input {index} ({address}): {problem}");
        }
        addresses.push(address.to_string());
    }
    addresses.sort_unstable();
    addresses.dedup();
    abandon(&draft, handle);

    let (again, again_handle, again_tx) = prepare();
    abandon(&again, again_handle);
    let marks = (probes.sent_marks)(wallet_id);
    for txid in [tx.txid().to_string(), again_tx.txid().to_string()] {
        assert!(
            !marks.contains(&txid),
            "wallet {wallet_id}: the abandoned spend {txid} is marked sent"
        );
    }
    assert_eq!(
        marks, marks_before,
        "wallet {wallet_id}: the abandoned spends changed the sent marks"
    );
    if !locked.is_empty() {
        ok(
            "lock_outpoints",
            json!({ "walletId": wallet_id, "outpoints": locked }),
        );
    }
    let after = spend_state(expected, wallet_id, ok);
    let mut diffs = Vec::new();
    equal(
        &before,
        &after,
        ["before the spend", "after it was abandoned"],
        "",
        &mut diffs,
    );
    assert!(
        diffs.is_empty(),
        "wallet {wallet_id}: the abandoned spend changed the wallet: {diffs:?}"
    );
    let balance = after
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["cmd"] == "balance")
        .map(|c| c["result"]["confirmed"].clone());
    json!({
        "walletId": wallet_id,
        "inputs": tx.input.len(),
        "addresses": addresses,
        "relocked": locked.len(),
        "balanceAfter": balance,
    })
}

/// Why input `index` of `tx` is not a P2PKH spend of `script`: its scriptSig
/// must be `<DER signature ‖ SIGHASH_ALL> <public key>`, the key must hash to
/// `script`'s, and the signature must verify over the legacy sighash.
fn p2pkh_input_problem(
    tx: &dashcore::Transaction,
    index: usize,
    script: &dashcore::ScriptBuf,
) -> Option<String> {
    use dashcore::hashes::Hash as _;
    use dashcore::secp256k1::{Message, ecdsa};
    let mut pushes = tx.input[index].script_sig.instructions();
    let (Some(Ok(sig)), Some(Ok(key)), None) = (pushes.next(), pushes.next(), pushes.next()) else {
        return Some("its scriptSig is not two pushes".into());
    };
    let (Some(sig), Some(key)) = (sig.push_bytes(), key.push_bytes()) else {
        return Some("its scriptSig is not two pushes".into());
    };
    let Some((&hash_type, der)) = sig.as_bytes().split_last() else {
        return Some("an empty signature".into());
    };
    if hash_type != 1 {
        return Some(format!("sighash type {hash_type}, not SIGHASH_ALL"));
    }
    let pk = match dashcore::PublicKey::from_slice(key.as_bytes()) {
        Ok(pk) => pk,
        Err(e) => return Some(format!("the public key: {e}")),
    };
    if dashcore::ScriptBuf::new_p2pkh(&pk.pubkey_hash()) != *script {
        return Some("its key is not the key of the coin's address".into());
    }
    let hash =
        match dashcore::sighash::SighashCache::new(tx).legacy_signature_hash(index, script, 1) {
            Ok(h) => h,
            Err(e) => return Some(format!("the sighash: {e}")),
        };
    ecdsa::Signature::from_der(der)
        .and_then(|s| ecdsa::verify(&s, Message::from_digest(hash.to_byte_array()), &pk.inner))
        .err()
        .map(|e| format!("the signature does not verify: {e}"))
}

fn v0_1_0_snapshot() -> Value {
    read_json(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata/upgrade/v0.1.0/expected.json"),
    )
}

fn v0_2_0_snapshot() -> Value {
    read_json(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata/upgrade/v0.2.0/expected.json"),
    )
}

#[test]
fn a_call_that_fails_on_one_open_and_answers_on_the_other_differs() {
    let answered = |result: Value| json!({ "calls": [{ "cmd": "tx_label", "args": {}, "result": result }], "signatures": [] });
    let failed = json!({ "calls": [{ "cmd": "tx_label", "args": {}, "error": "storage" }], "signatures": [] });
    assert!(compare_opens(&answered(Value::Null), &answered(Value::Null)).is_empty());
    assert_eq!(compare_opens(&answered(Value::Null), &failed).len(), 1);
    assert_eq!(compare_opens(&failed, &answered(Value::Null)).len(), 1);
    assert_eq!(compare_opens(&failed, &failed).len(), 1);
    assert_eq!(
        compare_opens(&answered(json!("a")), &answered(json!("b"))).len(),
        1
    );
}

#[test]
fn the_second_open_must_answer_exactly_as_the_first_less_volatile_values() {
    let open = |cmd: &str, result: Value| json!({ "calls": [{ "cmd": cmd, "args": {}, "result": result }], "signatures": [] });
    let row = json!({ "txid": "aa", "amount": 1 });
    let grown = json!({ "txid": "aa", "amount": 1, "fiatThen": 2 });
    assert!(
        compare_opens(
            &open("history_page", row.clone()),
            &open("history_page", row.clone())
        )
        .is_empty()
    );
    assert_eq!(
        compare_opens(
            &open("history_page", row.clone()),
            &open("history_page", grown.clone())
        )
        .len(),
        1
    );
    assert_eq!(
        compare_opens(&open("history_page", grown), &open("history_page", row)).len(),
        1
    );
    let policy = |at: Value| {
        open(
            "quick_unlock_policy",
            json!({ "lastPassphraseAt": at, "enabled": false }),
        )
    };
    assert!(compare_opens(&policy(json!(1)), &policy(json!(2))).is_empty());
    assert_eq!(
        compare_opens(&policy(json!(1)), &policy(Value::Null)).len(),
        1
    );
}

#[test]
fn discovery_names_a_release_directory_without_a_readable_snapshot() {
    let root = dw_testutil::private_tempdir();
    std::fs::write(root.path().join("README.md"), "notes").unwrap();
    std::fs::create_dir(root.path().join("v9.9.8")).unwrap();
    std::fs::write(root.path().join("v9.9.8/expected.json"), "{}").unwrap();
    assert_eq!(fixtures_in(root.path()).unwrap().len(), 1);
    std::fs::create_dir(root.path().join("v9.9.9")).unwrap();
    let e = fixtures_in(root.path()).unwrap_err();
    assert!(e.contains("v9.9.9"), "{e}");
    std::fs::write(root.path().join("v9.9.9/expected.json"), "{").unwrap();
    let e = fixtures_in(root.path()).unwrap_err();
    assert!(e.contains("v9.9.9"), "{e}");
}

#[test]
fn every_pinned_release_has_its_fixture_and_every_fixture_is_pinned() {
    let pinned: Vec<String> = SNAPSHOT_SHA256.iter().map(|(t, _)| t.to_string()).collect();
    assert_eq!(pinned_tag_problems(&pinned), Vec::<String>::new());
    let missing = &pinned[1..];
    let problems = pinned_tag_problems(missing);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains(&pinned[0]), "{problems:?}");
    let mut extra = pinned.clone();
    extra.push("v9.9.9".to_string());
    let problems = pinned_tag_problems(&extra);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("v9.9.9"), "{problems:?}");

    let root = dw_testutil::private_tempdir();
    for tag in &pinned {
        std::fs::create_dir(root.path().join(tag)).unwrap();
        std::fs::write(root.path().join(tag).join("expected.json"), "{}").unwrap();
    }
    assert_eq!(pinned_fixtures_in(root.path()).unwrap().len(), pinned.len());
    std::fs::remove_dir_all(root.path().join(&pinned[0])).unwrap();
    let e = pinned_fixtures_in(root.path()).unwrap_err();
    assert!(
        e.contains(&pinned[0]) && e.contains("no fixture directory"),
        "{e}"
    );
}

#[test]
fn a_coin_without_an_amount_or_a_sum_past_u64_is_a_problem() {
    let full = v0_2_0_snapshot();
    let payer = full["dashd"]["payment"]["walletId"]
        .as_str()
        .unwrap()
        .to_string();
    let edit_coins = |amount: Value| {
        let mut snapshot = full.clone();
        for call in snapshot["calls"].as_array_mut().unwrap() {
            if call["cmd"] == "utxos" && call["args"]["walletId"] == payer.as_str() {
                for coin in call["result"].as_array_mut().unwrap() {
                    coin["amount"] = amount.clone();
                }
            }
        }
        snapshot_problems("v0.2.0", &snapshot)
    };
    let problems = edit_coins(json!("10"));
    assert!(
        problems
            .iter()
            .any(|p| p.contains("recorded coins add up to None")),
        "{problems:?}"
    );
    let problems = edit_coins(json!(u64::MAX));
    assert!(
        problems
            .iter()
            .any(|p| p.contains("recorded coins add up to None")),
        "{problems:?}"
    );
    let mut fee_overflows = full.clone();
    fee_overflows["dashd"]["payment"]["feeDuffs"] = json!(u64::MAX);
    let problems = snapshot_problems("v0.2.0", &fee_overflows);
    assert!(
        problems
            .iter()
            .any(|p| p.contains(&payer) && p.contains("less payment and fee None")),
        "{problems:?}"
    );
}

fn snapshot_text(tag: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata/upgrade")
        .join(tag)
        .join("expected.json");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn a_snapshot_that_is_not_the_committed_one_is_refused() {
    assert_eq!(
        snapshot_digest_problem("v0.1.0", &snapshot_text("v0.1.0")),
        None
    );
    let text = snapshot_text("v0.2.0");
    assert_eq!(snapshot_digest_problem("v0.2.0", &text), None);
    assert_eq!(
        snapshot_digest_problem("v0.2.0", &text.replace("\r\n", "\n").replace('\n', "\r\n")),
        None
    );
    let network = "\"network\":";
    assert!(text.contains(network));
    assert!(
        snapshot_digest_problem("v0.2.0", &text.replacen(network, "\"network\"\r:", 1)).is_some()
    );
    let full = v0_2_0_snapshot();
    let payer = full["dashd"]["payment"]["walletId"]
        .as_str()
        .unwrap()
        .to_string();
    let mut unaddressed = full.clone();
    unaddressed["calls"]
        .as_array_mut()
        .unwrap()
        .retain(|c| c["cmd"] != "addresses" || c["args"]["walletId"] != payer.as_str());
    assert_eq!(
        snapshot_problems("v0.2.0", &unaddressed),
        Vec::<String>::new()
    );
    let edited = serde_json::to_string_pretty(&unaddressed).unwrap();
    assert!(snapshot_digest_problem("v0.2.0", &edited).is_some());
    let unconfirmed = "\"unconfirmed\": 0";
    assert!(text.contains(unconfirmed));
    assert!(snapshot_digest_problem("v0.2.0", &text.replacen(unconfirmed, "", 1)).is_some());
    assert!(snapshot_digest_problem("v9.9.9", &text).is_some());
}

#[test]
fn a_thinned_snapshot_is_refused() {
    let full = v0_1_0_snapshot();
    assert_eq!(snapshot_problems("v0.1.0", &full), Vec::<String>::new());
    let mut no_calls = full.clone();
    no_calls["calls"] = json!([]);
    assert!(!snapshot_problems("v0.1.0", &no_calls).is_empty());
    let mut one_signature = full.clone();
    one_signature["signatures"].as_array_mut().unwrap().pop();
    assert!(!snapshot_problems("v0.1.0", &one_signature).is_empty());
    let mut unlisted = full.clone();
    for call in unlisted["calls"].as_array_mut().unwrap() {
        if call["cmd"] == "list_wallets" {
            call["result"].as_array_mut().unwrap().pop();
        }
    }
    assert!(!snapshot_problems("v0.1.0", &unlisted).is_empty());
}

#[test]
fn a_snapshot_without_a_funded_signature_or_with_an_unaccounted_balance_is_refused() {
    let full = v0_2_0_snapshot();
    assert_eq!(snapshot_problems("v0.2.0", &full), Vec::<String>::new());
    let mut change_unsigned = full.clone();
    change_unsigned["fundedSignatures"]
        .as_array_mut()
        .unwrap()
        .remove(0);
    assert!(!snapshot_problems("v0.2.0", &change_unsigned).is_empty());
    let mut no_funded = full.clone();
    no_funded
        .as_object_mut()
        .unwrap()
        .remove("fundedSignatures");
    assert!(!snapshot_problems("v0.2.0", &no_funded).is_empty());
    let mut fee_off = full.clone();
    fee_off["dashd"]["payment"]["feeDuffs"] = json!(225);
    assert!(!snapshot_problems("v0.2.0", &fee_off).is_empty());
    let mut no_fee = full.clone();
    no_fee["dashd"]["payment"]
        .as_object_mut()
        .unwrap()
        .remove("feeDuffs");
    assert!(!snapshot_problems("v0.2.0", &no_fee).is_empty());
    let payer = full["dashd"]["payment"]["walletId"]
        .as_str()
        .unwrap()
        .to_string();
    let mut payer_undeposited = full.clone();
    payer_undeposited["dashd"]["deposits"]
        .as_object_mut()
        .unwrap()
        .remove(&payer);
    assert!(!snapshot_problems("v0.2.0", &payer_undeposited).is_empty());
    let mut coin_unaddressed = full.clone();
    let mut address = None;
    for call in coin_unaddressed["calls"].as_array_mut().unwrap() {
        if call["cmd"] == "utxos" && call["args"]["walletId"] == payer.as_str() {
            address = call["result"][0].as_object_mut().unwrap().remove("address");
            break;
        }
    }
    let address = address.expect("the paying wallet's first coin has an address");
    coin_unaddressed["fundedSignatures"]
        .as_array_mut()
        .unwrap()
        .retain(|s| s["address"] != address);
    assert_eq!(
        coin_unaddressed["fundedSignatures"]
            .as_array()
            .unwrap()
            .len(),
        full["fundedSignatures"].as_array().unwrap().len() - 1
    );
    assert!(!snapshot_problems("v0.2.0", &coin_unaddressed).is_empty());
    let mut narrow_first = full.clone();
    let calls = narrow_first["calls"].as_array_mut().unwrap();
    let at = calls
        .iter()
        .position(|c| c["cmd"] == "utxos" && c["args"]["walletId"] == payer.as_str())
        .unwrap();
    let mut narrow = calls[at].clone();
    narrow["args"]["filter"]["includeLocked"] = json!(false);
    let hidden = narrow["result"].as_array_mut().unwrap().remove(1);
    calls.insert(at, narrow);
    narrow_first["fundedSignatures"]
        .as_array_mut()
        .unwrap()
        .retain(|s| s["address"] != hidden["address"]);
    assert_eq!(
        narrow_first["fundedSignatures"].as_array().unwrap().len(),
        full["fundedSignatures"].as_array().unwrap().len() - 1
    );
    let problems = snapshot_problems("v0.2.0", &narrow_first);
    assert!(
        problems.iter().any(|p| p.contains("funded addresses")),
        "{problems:?}"
    );
    let watch_only = full["watchOnly"]["walletId"].as_str().unwrap().to_string();
    let mut watch_only_dropped = full.clone();
    watch_only_dropped
        .as_object_mut()
        .unwrap()
        .remove("watchOnly");
    watch_only_dropped["dashd"]["deposits"]
        .as_object_mut()
        .unwrap()
        .remove(&watch_only);
    assert!(!snapshot_problems("v0.2.0", &watch_only_dropped).is_empty());
    let mut watch_only_unlisted = watch_only_dropped.clone();
    for call in watch_only_unlisted["calls"].as_array_mut().unwrap() {
        if call["cmd"] == "list_wallets" {
            call["result"]
                .as_array_mut()
                .unwrap()
                .retain(|w| w["walletId"] != watch_only.as_str());
        }
    }
    assert!(!snapshot_problems("v0.2.0", &watch_only_unlisted).is_empty());
    let mut watch_only_unbalanced = watch_only_unlisted.clone();
    watch_only_unbalanced["calls"]
        .as_array_mut()
        .unwrap()
        .retain(|c| c["cmd"] != "balance" || c["args"]["walletId"] != watch_only.as_str());
    assert_eq!(
        watch_only_unbalanced["calls"].as_array().unwrap().len(),
        full["calls"].as_array().unwrap().len() - 1
    );
    assert!(!snapshot_problems("v0.2.0", &watch_only_unbalanced).is_empty());
    let mut watch_only_unasked = watch_only_unbalanced.clone();
    watch_only_unasked["calls"]
        .as_array_mut()
        .unwrap()
        .retain(|c| c["args"]["walletId"] != watch_only.as_str());
    assert!(
        watch_only_unasked["calls"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["cmd"] == "wallet_load_states")
    );
    assert!(!snapshot_problems("v0.2.0", &watch_only_unasked).is_empty());
    let mut coin_dropped = full.clone();
    let mut gone = None;
    for call in coin_dropped["calls"].as_array_mut().unwrap() {
        if call["cmd"] == "utxos" && call["args"]["walletId"] == payer.as_str() {
            gone = Some(call["result"].as_array_mut().unwrap().remove(1));
            break;
        }
    }
    let gone = gone.expect("the paying wallet has two coins");
    coin_dropped["fundedSignatures"]
        .as_array_mut()
        .unwrap()
        .retain(|s| s["address"] != gone["address"]);
    assert_eq!(
        coin_dropped["fundedSignatures"].as_array().unwrap().len(),
        full["fundedSignatures"].as_array().unwrap().len() - 1
    );
    assert!(!snapshot_problems("v0.2.0", &coin_dropped).is_empty());
}

#[test]
fn a_v0_1_0_balance_off_by_a_duff_is_refused() {
    let full = v0_1_0_snapshot();
    assert_eq!(snapshot_problems("v0.1.0", &full), Vec::<String>::new());
    for wallet in full["wallets"].as_array().unwrap() {
        let id = wallet["walletId"].as_str().unwrap();
        let mut off = full.clone();
        for call in off["calls"].as_array_mut().unwrap() {
            if call["cmd"] == "balance" && call["args"]["walletId"] == id {
                let confirmed = call["result"]["confirmed"].as_u64().unwrap();
                call["result"]["confirmed"] = json!(confirmed - 1);
            }
        }
        let problems = snapshot_problems("v0.1.0", &off);
        assert!(
            problems
                .iter()
                .any(|p| p.contains(id) && p.contains("less payment and fee")),
            "{problems:?}"
        );
    }
}

#[test]
fn v0_1_0_deposits_and_fee_come_from_the_transactions_dashd_named() {
    let full = v0_1_0_snapshot();
    let dashd = dashd_from_transactions(&full).unwrap();
    let main = full["wallets"][0]["walletId"].as_str().unwrap();
    let second = full["wallets"][1]["walletId"].as_str().unwrap();
    assert_eq!(dashd["deposits"][main], 1_250_000_000u64);
    assert_eq!(dashd["deposits"][second], 300_000_000u64);
    assert_eq!(dashd["payment"]["walletId"], main);
    assert_eq!(dashd["payment"]["duffs"], 125_000_000u64);
    assert_eq!(dashd["payment"]["feeDuffs"], 226u64);

    let payment = full["dashd"]["payment"]["txid"].as_str().unwrap();
    let edit_raw = |txid: &str, edit: &dyn Fn(&str) -> String| {
        let mut snapshot = full.clone();
        for call in snapshot["calls"].as_array_mut().unwrap() {
            if call["cmd"] == "tx_detail" && call["args"]["txid"] == txid {
                let raw = call["result"]["rawHex"].as_str().unwrap().to_string();
                call["result"]["rawHex"] = json!(edit(&raw));
            }
        }
        dashd_from_transactions(&snapshot)
    };
    let flipped = |raw: &str| {
        let mut bytes = hex::decode(raw).unwrap();
        let last = bytes.len() - 5;
        bytes[last] ^= 1;
        hex::encode(bytes)
    };
    let e = edit_raw(payment, &flipped).unwrap_err();
    assert!(e.contains(payment), "{e}");
    let funding = full["dashd"]["funding"]["mainFirst"].as_str().unwrap();
    let e = edit_raw(funding, &flipped).unwrap_err();
    assert!(e.contains(funding), "{e}");
    let e = edit_raw(payment, &|_| "zz".to_string()).unwrap_err();
    assert!(e.contains(payment), "{e}");

    let mut unaddressed = full.clone();
    unaddressed["calls"]
        .as_array_mut()
        .unwrap()
        .retain(|c| c["cmd"] != "addresses" || c["args"]["walletId"] != second);
    assert!(dashd_from_transactions(&unaddressed).is_err());
    let mut other_payee = full.clone();
    other_payee["dashd"]["payment"]["duffs"] = json!(125_000_001u64);
    assert!(dashd_from_transactions(&other_payee).is_err());
}

#[test]
fn a_p2pkh_input_verifies_only_with_its_own_key_signature_and_sighash() {
    use dashcore::hashes::Hash as _;
    use dashcore::secp256k1::{Message, PublicKey, SecretKey, ecdsa};
    let key = |b: u8| {
        let sk = SecretKey::from_secret_bytes([b; 32]).unwrap();
        (
            sk,
            dashcore::PublicKey::new(PublicKey::from_secret_key(&sk)),
        )
    };
    let (sk, pk) = key(1);
    let (other_sk, other_pk) = key(2);
    let script = dashcore::ScriptBuf::new_p2pkh(&pk.pubkey_hash());
    let mut tx = dashcore::Transaction {
        version: 2,
        lock_time: 0,
        input: vec![dashcore::TxIn {
            previous_output: dashcore::OutPoint::new(dashcore::Txid::all_zeros(), 0),
            script_sig: dashcore::ScriptBuf::new(),
            sequence: 0xffff_ffff,
            witness: Default::default(),
        }],
        output: vec![dashcore::TxOut {
            value: 1000,
            script_pubkey: script.clone(),
        }],
        special_transaction_payload: None,
    };
    let script_sig =
        |tx: &dashcore::Transaction, sk: &SecretKey, pk: &dashcore::PublicKey, hash_type: u8| {
            let hash = dashcore::sighash::SighashCache::new(tx)
                .legacy_signature_hash(0, &script, u32::from(hash_type))
                .unwrap();
            let mut der = ecdsa::sign(Message::from_digest(hash.to_byte_array()), sk)
                .serialize_der()
                .to_vec();
            der.push(hash_type);
            let pk = pk.to_bytes();
            let mut bytes = vec![der.len() as u8];
            bytes.extend_from_slice(&der);
            bytes.push(pk.len() as u8);
            bytes.extend_from_slice(&pk);
            dashcore::ScriptBuf::from_bytes(bytes)
        };
    tx.input[0].script_sig = script_sig(&tx, &sk, &pk, 1);
    assert_eq!(p2pkh_input_problem(&tx, 0, &script), None);
    let mut moved = tx.clone();
    moved.output[0].value = 999;
    assert!(
        p2pkh_input_problem(&moved, 0, &script)
            .unwrap()
            .contains("does not verify")
    );
    let mut other = tx.clone();
    other.input[0].script_sig = script_sig(&tx, &other_sk, &other_pk, 1);
    assert!(
        p2pkh_input_problem(&other, 0, &script)
            .unwrap()
            .contains("not the key")
    );
    let mut wrong_signer = tx.clone();
    wrong_signer.input[0].script_sig = script_sig(&tx, &other_sk, &pk, 1);
    assert!(
        p2pkh_input_problem(&wrong_signer, 0, &script)
            .unwrap()
            .contains("does not verify")
    );
    let mut none = tx.clone();
    none.input[0].script_sig = script_sig(&tx, &sk, &pk, 2);
    assert!(
        p2pkh_input_problem(&none, 0, &script)
            .unwrap()
            .contains("SIGHASH_ALL")
    );
    let mut empty = tx.clone();
    empty.input[0].script_sig = dashcore::ScriptBuf::new();
    assert!(p2pkh_input_problem(&empty, 0, &script).is_some());
}

#[test]
fn a_spend_that_misses_a_coin_a_lock_or_the_exact_balance_is_a_problem() {
    let full = v0_2_0_snapshot();
    let calls = full["calls"].as_array().unwrap();
    let dashd = &full["dashd"];
    let spends: Vec<Value> = full["wallets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| {
            let id = &w["walletId"];
            let coins = every_coin(calls, id).unwrap();
            let mut addresses: Vec<&str> = coins
                .iter()
                .map(|c| c["address"].as_str().unwrap())
                .collect();
            addresses.sort_unstable();
            addresses.dedup();
            json!({
                "walletId": id,
                "inputs": coins.len(),
                "addresses": addresses,
                "relocked": coins.iter().filter(|c| c["userLocked"] == true).count(),
                "balanceAfter": exact_balance(dashd, id),
            })
        })
        .collect();
    let answers = |spends: &[Value]| json!({ "spends": spends });
    assert_eq!(
        spend_problems("v0.2.0", &full, &answers(&spends)),
        Vec::<String>::new()
    );
    let payer = spends
        .iter()
        .position(|s| s["walletId"] == dashd["payment"]["walletId"])
        .unwrap();
    assert_eq!(
        spends[payer]["relocked"], 1,
        "the paying wallet has a locked coin"
    );
    let edited = |edit: &dyn Fn(&mut Value)| {
        let mut s = spends.clone();
        edit(&mut s[payer]);
        spend_problems("v0.2.0", &full, &answers(&s))
    };
    assert_eq!(
        edited(&|s| {
            s["addresses"].as_array_mut().unwrap().pop();
        })
        .len(),
        1
    );
    assert_eq!(edited(&|s| s["inputs"] = json!(1)).len(), 1);
    assert_eq!(edited(&|s| s["relocked"] = json!(0)).len(), 1);
    assert_eq!(
        edited(&|s| s["balanceAfter"] = json!(s["balanceAfter"].as_u64().unwrap() - 1)).len(),
        1
    );
    assert_eq!(
        spend_problems("v0.2.0", &full, &answers(&spends[..1])).len(),
        2
    );
    assert!(!spend_problems("v0.2.0", &full, &json!({})).is_empty());
}

#[cfg(unix)]
#[test]
fn a_child_past_its_deadline_is_killed_and_its_output_kept() {
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "echo started; exec sleep 60"]);
    let started = Instant::now();
    let e = run_with_deadline(cmd, Duration::from_secs(1)).unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(e.contains("killed") && e.contains("started"), "{e}");
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "echo done"]);
    let (status, stdout, _) = run_with_deadline(cmd, Duration::from_secs(60)).unwrap();
    assert!(status.success() && stdout.contains("done"));
}
