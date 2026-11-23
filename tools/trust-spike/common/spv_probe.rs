//! The dash-spv side of the E0-10a trust spike, shared by the probe built at
//! platform's rust-dashcore pin (`pin/`) and the one built at rust-dashcore
//! `dev` (`dev/`). Each crate supplies a `compat` module for the few calls
//! whose shape differs between the two revisions.
//!
//! The probe runs a dash-spv client configured like the desktop's
//! (`dw-engine` `Session::spv_config`: masternode sync on), tails the tuple
//! log the capture probe writes, and asks the client for every captured
//! (quorum type, quorum hash, core chain-locked height) tuple through
//! `get_quorum_at_height`, the call platform-wallet's
//! `SpvRuntime::get_quorum_public_key` makes. Every lookup result is
//! appended to `--out` as one JSON line.

use std::collections::BTreeMap;
use std::io::{BufRead, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dash_spv::network::PeerNetworkManager;
use dash_spv::storage::DiskStorageManager;
use dash_spv::{ClientConfig, DashSpvClient, LLMQType, LevelFilter, Network, QuorumHash};
use dashcore::hashes::Hash;
use dashcore::sml::llmq_entry_verification::LLMQEntryVerificationStatus;
use dashcore::sml::quorum_entry::qualified_quorum_entry::QualifiedQuorumEntry;
use key_wallet::wallet::managed_wallet_info::ManagedWalletInfo;
use key_wallet_manager::WalletManager;
use serde_json::{Value, json};
use tokio::sync::RwLock;

pub type Client =
    DashSpvClient<WalletManager<ManagedWalletInfo>, PeerNetworkManager, DiskStorageManager>;

pub struct SpvArgs {
    pub network: Network,
    pub dir: PathBuf,
    pub tuples: PathBuf,
    pub out: PathBuf,
    pub duration: Duration,
    pub label: String,
    /// How often the probe reads new tuples and re-asks unresolved ones.
    pub poll: Duration,
    pub filters: bool,
    /// dash-spv console logging at this level (`--log warn`), off by default.
    pub log: Option<LevelFilter>,
}

pub fn parse_spv_args(mut args: impl Iterator<Item = String>, label: &str) -> SpvArgs {
    let mut network = Network::Testnet;
    let mut dir = None;
    let mut tuples = None;
    let mut out = None;
    let mut duration = Duration::from_secs(3600);
    let mut poll = Duration::from_secs(15);
    let mut filters = false;
    let mut log = None;
    let mut label = label.to_string();
    while let Some(arg) = args.next() {
        let mut value = || args.next().unwrap_or_else(|| panic!("{arg} needs a value"));
        match arg.as_str() {
            "--network" => network = parse_network(&value()),
            "--dir" => dir = Some(PathBuf::from(value())),
            "--tuples" => tuples = Some(PathBuf::from(value())),
            "--out" => out = Some(PathBuf::from(value())),
            "--duration-secs" => duration = Duration::from_secs(value().parse().expect("secs")),
            "--poll-secs" => poll = Duration::from_secs(value().parse().expect("secs")),
            "--label" => label = value(),
            "--filters" => filters = true,
            "--log" => log = Some(value().parse().expect("a tracing level")),
            other => panic!("unknown argument {other}"),
        }
    }
    SpvArgs {
        network,
        dir: dir.expect("--dir"),
        tuples: tuples.expect("--tuples"),
        out: out.expect("--out"),
        duration,
        label,
        poll,
        filters,
        log,
    }
}

pub fn parse_network(name: &str) -> Network {
    match name {
        "mainnet" => Network::Mainnet,
        "testnet" => Network::Testnet,
        other => panic!("unsupported network {other}"),
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// One (type, hash, height) tuple a real proof cited, as the capture probe
/// logged it. `hash` is the byte order the SDK hands its `ContextProvider`.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub struct Tuple {
    pub quorum_type: u32,
    pub hash: String,
    pub height: u32,
}

/// How long a tuple that has not resolved `Verified` with the trusted key
/// keeps being re-asked.
const REASK_FOR_MS: u64 = 30 * 60 * 1000;

#[derive(Default)]
struct TupleState {
    trusted_key: Option<String>,
    /// When the capture probe logged the proof that cited it.
    captured_ms: u64,
    first_asked_ms: Option<u64>,
    asks: u32,
    /// Strongest outcome so far: 0 miss (or a key that differs from the
    /// trusted one), 1 found but not `Verified`, 2 `Verified`.
    best: u8,
}

/// Reads the tuple log from `offset`, returning new tuples with the key the
/// trusted service gave for them and the time their proof was captured.
fn read_new_tuples(path: &Path, offset: &mut u64) -> Vec<(Tuple, Option<String>, u64)> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    if file.seek(SeekFrom::Start(*offset)).is_err() {
        return Vec::new();
    }
    let mut reader = std::io::BufReader::new(file);
    let mut found = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Err(error) => {
                eprintln!("tuple log unreadable at offset {offset}: {error}");
                break;
            }
            Ok(_) if !line.ends_with('\n') => break, // a partial line: re-read it next time
            Ok(n) => {
                *offset += n as u64;
                let Ok(row) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let captured_ms = row["t"].as_u64().unwrap_or_else(now_ms);
                for call in row["ctx_calls"].as_array().into_iter().flatten() {
                    let (Some(t), Some(h), Some(height)) = (
                        call["type"].as_u64(),
                        call["hash"].as_str(),
                        call["ccl"].as_u64(),
                    ) else {
                        continue;
                    };
                    found.push((
                        Tuple {
                            quorum_type: t as u32,
                            hash: h.to_string(),
                            height: height as u32,
                        },
                        call["key"].as_str().map(str::to_string),
                        captured_ms,
                    ));
                }
            }
        }
    }
    found
}

fn status_name(status: &LLMQEntryVerificationStatus) -> String {
    match status {
        LLMQEntryVerificationStatus::Unknown => "Unknown".into(),
        LLMQEntryVerificationStatus::Verified => "Verified".into(),
        LLMQEntryVerificationStatus::Skipped(reason) => format!("Skipped({reason:?})"),
        LLMQEntryVerificationStatus::Invalid(error) => format!("Invalid({error})"),
    }
}

/// A quorum as the engine stats list it: display hash and status.
pub fn quorum_row(quorum: &QualifiedQuorumEntry) -> Value {
    json!([
        quorum.quorum_entry.quorum_hash.to_string(),
        format!("{:?}", quorum.verified)
    ])
}

pub async fn build_client(args: &SpvArgs) -> Result<Client, Box<dyn std::error::Error>> {
    std::fs::create_dir_all(&args.dir)?;
    let mut config = ClientConfig::new(args.network)
        .with_storage_path(args.dir.clone())
        .with_user_agent(format!("/dw-trust-spike:{}/", args.label));
    config.enable_masternodes = true;
    if !args.filters {
        config = config.without_filters();
    }
    config.validate()?;
    let network = PeerNetworkManager::new(&config).await?;
    let storage = DiskStorageManager::new(&config).await?;
    let wallet = Arc::new(RwLock::new(WalletManager::<ManagedWalletInfo>::new(
        config.network,
    )));
    Ok(DashSpvClient::new(config, network, storage, wallet, vec![]).await?)
}

pub async fn run(args: SpvArgs) -> Result<(), Box<dyn std::error::Error>> {
    let _logging = args.log.map(dash_spv::init_console_logging).transpose()?;
    let started = Instant::now();
    let started_ms = now_ms();
    let secs_since_start = |t: u64| t.saturating_sub(started_ms) as f64 / 1000.0;
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.out)?;
    let mut emit = |row: Value| {
        let _ = writeln!(out, "{row}");
        let _ = out.flush();
    };
    emit(
        json!({"event": "start", "t": started_ms, "label": args.label,
                "network": args.network.to_string(), "dir": args.dir, "filters": args.filters}),
    );

    let client = Arc::new(build_client(&args).await?);
    let runner = crate::compat::start(Arc::clone(&client));

    let mut offset = 0u64;
    let mut tuples: BTreeMap<Tuple, TupleState> = BTreeMap::new();
    let mut first_verified_ms: Option<u64> = None;
    let mut first_found_ms: Option<u64> = None;
    let mut synced_ms: Option<u64> = None;
    let mut ticks = 0u64;
    let mut runner_ended = false;

    while started.elapsed() < args.duration {
        let progress = sync_progress(&client).await;
        let synced = progress.synced;
        if synced && synced_ms.is_none() {
            let now = now_ms();
            synced_ms = Some(now);
            emit(
                json!({"event": "synced", "t": now, "after_s": started.elapsed().as_secs_f64(),
                        "progress": progress.detail}),
            );
        }
        if runner.is_finished() && !runner_ended {
            runner_ended = true;
            emit(json!({"event": "runner_ended", "t": now_ms()}));
        }
        ticks += 1;
        let stats = if ticks % 20 == 1 {
            crate::compat::engine_stats(&client).await
        } else {
            Value::Null
        };
        emit(
            json!({"event": "tick", "t": now_ms(), "elapsed_s": started.elapsed().as_secs_f64(),
                    "synced": synced, "runner_alive": !runner_ended,
                    "progress": progress.detail, "engine": stats}),
        );

        for (tuple, key, captured_ms) in read_new_tuples(&args.tuples, &mut offset) {
            let state = tuples.entry(tuple).or_insert_with(|| TupleState {
                captured_ms,
                ..Default::default()
            });
            if state.trusted_key.is_none() {
                state.trusted_key = key;
            }
        }

        for (tuple, state) in tuples.iter_mut() {
            let now = now_ms();
            let reasking = state
                .first_asked_ms
                .is_some_and(|first| now.saturating_sub(first) < REASK_FOR_MS);
            if state.best == 2 || (state.asks > 0 && !reasking) {
                continue;
            }
            state.first_asked_ms.get_or_insert(now);
            let mut raw = [0u8; 32];
            if hex::decode_to_slice(&tuple.hash, &mut raw).is_err() {
                continue;
            }
            // platform-wallet `SpvRuntime::get_quorum_public_key` reverses the
            // SDK's bytes into a `QuorumHash`.
            raw.reverse();
            let quorum_hash = QuorumHash::from_byte_array(raw);
            let llmq_type = LLMQType::from(tuple.quorum_type as u8);
            let asked = Instant::now();
            let result = client
                .get_quorum_at_height(tuple.height, llmq_type, quorum_hash)
                .await;
            let lookup_ms = asked.elapsed().as_secs_f64() * 1000.0;
            state.asks += 1;
            let now = now_ms();
            let mut row = json!({"event": "lookup", "t": now, "type": tuple.quorum_type,
                   "hash": tuple.hash, "ccl": tuple.height, "synced": synced,
                   "ask": state.asks, "lookup_ms": lookup_ms,
                   "age_s": now.saturating_sub(state.captured_ms) as f64 / 1000.0});
            match result {
                Ok(entry) => {
                    let key = hex::encode(entry.quorum_entry.quorum_public_key.as_ref() as &[u8]);
                    let verified = matches!(entry.verified, LLMQEntryVerificationStatus::Verified);
                    let matches = state.trusted_key.as_deref().map(|k| k == key);
                    // A key that differs from the trusted one fails the proof.
                    let level = match (matches, verified) {
                        (Some(false), _) => 0,
                        (_, true) => 2,
                        (_, false) => 1,
                    };
                    if level > state.best {
                        state.best = level;
                        first_found_ms.get_or_insert(now);
                        if level == 2 {
                            first_verified_ms.get_or_insert(now);
                        }
                    }
                    row["found"] = json!(true);
                    row["status"] = json!(status_name(&entry.verified));
                    row["key_matches_trusted"] = json!(matches);
                }
                Err(error) => {
                    row["found"] = json!(false);
                    row["error"] = json!(error.to_string());
                }
            }
            emit(row);
        }
        tokio::time::sleep(args.poll).await;
    }

    let summary: Vec<Value> = tuples
        .iter()
        .map(|(t, s)| {
            json!({"type": t.quorum_type, "hash": t.hash, "ccl": t.height,
                   "best": s.best, "asks": s.asks})
        })
        .collect();
    emit(
        json!({"event": "end", "t": now_ms(), "elapsed_s": started.elapsed().as_secs_f64(),
                "synced_after_s": synced_ms.map(secs_since_start),
                "first_found_after_s": first_found_ms.map(secs_since_start),
                "first_verified_after_s": first_verified_ms.map(secs_since_start),
                "tuples": summary}),
    );
    crate::compat::stop(&client, runner).await;
    Ok(())
}

/// The part of `SyncProgress` the probe logs.
pub struct ProgressView {
    pub synced: bool,
    pub detail: Value,
}

pub async fn sync_progress(client: &Client) -> ProgressView {
    let progress = client.progress().await;
    let headers = progress
        .headers()
        .ok()
        .map(|h| json!({"tip": h.tip_height(), "state": format!("{:?}", h.state())}));
    let masternodes = progress.masternodes().ok();
    let synced = masternodes.is_some_and(|m| format!("{:?}", m.state()) == "Synced");
    let masternodes = masternodes.map(|m| {
        json!({"state": format!("{:?}", m.state()), "current": m.current_height(),
               "target": m.target_height(), "diffs": m.diffs_processed(),
               "qrinfos": m.qr_infos_requested(), "validated_cycles": m.validated_cycles()})
    });
    ProgressView {
        synced,
        detail: json!({"headers": headers, "masternodes": masternodes}),
    }
}
