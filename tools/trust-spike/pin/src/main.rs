//! E0-10a trust spike, built at platform's pin (`bc41f1bc23`, rust-dashcore
//! `40268cc0`).
//!
//! - `capture`: read-only DAPI queries against one evonode at a time, with a
//!   `ContextProvider` that forwards to the trusted quorum service and logs
//!   every (quorum type, quorum hash, core chain-locked height) the SDK asks
//!   for while verifying the proof. One JSON line per query.
//! - `spv`: the shared dash-spv probe (`../common/spv_probe.rs`) at the pin.

use std::io::Write;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dash_context_provider::{ContextProvider, ContextProviderError};
use dash_sdk::dapi_client::{Address, AddressList};
use dash_sdk::platform::{DataContract, Fetch, Identifier, Identity};
use dash_sdk::{RequestSettings, Sdk, SdkBuilder};
use dpp::data_contract::TokenConfiguration;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::CoreBlockHeight;
use dpp::system_data_contracts::{dashpay_contract, dpns_contract};
use dpp::version::PlatformVersion;
use rand::seq::SliceRandom;
use rs_sdk_trusted_context_provider::TrustedHttpContextProvider;
use serde_json::{Value, json};

mod compat;
#[path = "../../common/spv_probe.rs"]
mod spv_probe;

use spv_probe::{now_ms, parse_network};

/// Forwards to the trusted quorum service and records each quorum lookup.
struct Recorder {
    inner: TrustedHttpContextProvider,
    calls: Mutex<Vec<Value>>,
}

impl ContextProvider for Recorder {
    fn get_data_contract(
        &self,
        id: &Identifier,
        platform_version: &PlatformVersion,
    ) -> Result<Option<Arc<DataContract>>, ContextProviderError> {
        self.inner.get_data_contract(id, platform_version)
    }

    fn register_data_contract(&self, contract: Arc<DataContract>) {
        self.inner.register_data_contract(contract)
    }

    fn get_token_configuration(
        &self,
        token_id: &Identifier,
    ) -> Result<Option<TokenConfiguration>, ContextProviderError> {
        self.inner.get_token_configuration(token_id)
    }

    fn get_quorum_public_key(
        &self,
        quorum_type: u32,
        quorum_hash: [u8; 32],
        core_chain_locked_height: u32,
    ) -> Result<[u8; 48], ContextProviderError> {
        let asked = Instant::now();
        let result =
            self.inner
                .get_quorum_public_key(quorum_type, quorum_hash, core_chain_locked_height);
        // The SDK passes the hash in RPC (display) order; dash-spv's
        // `QuorumHash` holds it reversed.
        let mut internal = quorum_hash;
        internal.reverse();
        let mut row = json!({
            "type": quorum_type,
            "hash": hex::encode(quorum_hash),
            "hash_internal": hex::encode(internal),
            "ccl": core_chain_locked_height,
            "trusted_ms": asked.elapsed().as_secs_f64() * 1000.0,
        });
        match &result {
            Ok(key) => row["key"] = json!(hex::encode(key)),
            Err(error) => row["error"] = json!(error.to_string()),
        }
        self.calls.lock().unwrap().push(row);
        result
    }

    fn get_platform_activation_height(&self) -> Result<CoreBlockHeight, ContextProviderError> {
        self.inner.get_platform_activation_height()
    }
}

struct CaptureArgs {
    network: dashcore::Network,
    out: std::path::PathBuf,
    rounds: u64,
    interval: Duration,
    identities: Vec<Identifier>,
}

fn parse_capture_args(mut args: impl Iterator<Item = String>) -> CaptureArgs {
    let mut network = None;
    let mut out = None;
    let mut rounds = 1;
    let mut interval = Duration::from_secs(60);
    let mut identities = Vec::new();
    while let Some(arg) = args.next() {
        let mut value = || args.next().unwrap_or_else(|| panic!("{arg} needs a value"));
        match arg.as_str() {
            "--network" => network = Some(parse_network(&value())),
            "--out" => out = Some(value().into()),
            "--rounds" => rounds = value().parse().expect("rounds"),
            "--interval-secs" => interval = Duration::from_secs(value().parse().expect("secs")),
            "--identity" => identities.push(
                Identifier::from_string(&value(), Encoding::Base58).expect("base58 identity id"),
            ),
            other => panic!("unknown argument {other}"),
        }
    }
    CaptureArgs {
        network: network.expect("--network"),
        out: out.expect("--out"),
        rounds,
        interval,
        identities,
    }
}

fn single_node_sdk(
    network: dashcore::Network,
    address: &Address,
    recorder: Arc<Recorder>,
) -> Result<Sdk, Box<dash_sdk::Error>> {
    let list = AddressList::from_iter([address.clone()]);
    SdkBuilder::new(list)
        .with_network(network)
        .with_settings(RequestSettings {
            retries: Some(0),
            timeout: Some(Duration::from_secs(20)),
            ..Default::default()
        })
        .with_context_provider(recorder)
        .build()
        .map_err(Box::new)
}

async fn capture(args: CaptureArgs) -> Result<(), Box<dyn std::error::Error>> {
    let cache = NonZeroUsize::new(100).expect("non-zero");
    // Built only to read the SDK's seed list; it never sends a request.
    let seeds = match args.network {
        dashcore::Network::Mainnet => SdkBuilder::new_mainnet(),
        _ => SdkBuilder::new_testnet(),
    }
    .with_context_provider(TrustedHttpContextProvider::new(args.network, None, cache)?)
    .build()?
    .address_list()
    .get_live_addresses();
    eprintln!("{} evonode addresses from the SDK seed list", seeds.len());

    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.out)?;

    let queries: Vec<(&str, Identifier)> = [
        ("contract:dpns", dpns_contract::ID),
        ("contract:dashpay", dashpay_contract::ID),
    ]
    .into_iter()
    .chain(args.identities.iter().map(|id| ("identity", *id)))
    .collect();
    for round in 0..args.rounds {
        let round_started = Instant::now();
        for &(kind, id) in &queries {
            let address = seeds
                .choose(&mut rand::thread_rng())
                .expect("seed list empty")
                .clone();
            // A fresh trusted provider per query, so its cache never answers
            // and every lookup reaches the service.
            let recorder = Arc::new(Recorder {
                inner: TrustedHttpContextProvider::new(args.network, None, cache)?,
                calls: Mutex::new(Vec::new()),
            });
            let started = Instant::now();
            let mut row = json!({
                "t": now_ms(), "round": round, "network": args.network.to_string(),
                "node": address.uri().to_string(), "query": kind, "id": id.to_string(
                    Encoding::Base58),
            });
            let sdk = single_node_sdk(args.network, &address, Arc::clone(&recorder))?;
            let fetched = match kind {
                "identity" => Identity::fetch_with_metadata_and_proof(&sdk, id, None)
                    .await
                    .map(|(found, meta, proof)| (found.is_some(), meta, proof)),
                _ => DataContract::fetch_with_metadata_and_proof(&sdk, id, None)
                    .await
                    .map(|(found, meta, proof)| (found.is_some(), meta, proof)),
            };
            row["elapsed_ms"] = json!(started.elapsed().as_secs_f64() * 1000.0);
            match fetched {
                Ok((found, meta, proof)) => {
                    row["ok"] = json!(true);
                    row["found"] = json!(found);
                    row["meta"] = json!({
                        "height": meta.height, "ccl": meta.core_chain_locked_height,
                        "epoch": meta.epoch, "time_ms": meta.time_ms,
                        "protocol_version": meta.protocol_version, "chain_id": meta.chain_id,
                    });
                    row["proof"] = json!({
                        "quorum_type": proof.quorum_type,
                        "quorum_hash": hex::encode(&proof.quorum_hash),
                        "round": proof.round,
                    });
                }
                Err(error) => {
                    row["ok"] = json!(false);
                    row["error"] = json!(error.to_string());
                }
            }
            row["ctx_calls"] = json!(recorder.calls.lock().unwrap().clone());
            writeln!(out, "{row}")?;
            out.flush()?;
        }
        if round + 1 < args.rounds {
            tokio::time::sleep(args.interval.saturating_sub(round_started.elapsed())).await;
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("capture") => capture(parse_capture_args(args)).await,
        Some("spv") => spv_probe::run(spv_probe::parse_spv_args(args, "pin-40268cc0")).await,
        _ => {
            eprintln!("usage: trust-spike-pin capture|spv [options]");
            std::process::exit(2);
        }
    }
}
