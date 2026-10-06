//! M3 R3 commands: the masternode list, details, provider transactions
//! (register, update service, update registrar, revoke), the keychain,
//! tracked masternodes, and payload vectors of the v24 shared-masternode
//! codecs. Output is line-oriented `key=value` text for the regtest suite
//! (`regtest/harness/tests/test_protx.py`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Subcommand, ValueEnum};
use dashcore::hashes::Hash;
use dw_engine::masternodes::{
    CollateralChoice, FeeSourceChoice, MasternodeQuery, MasternodeRow, MasternodeType,
    MasternodeTypeFilter, OperatorKeyChoice, PlatformFields, RegistrationRequest, RevocationReason,
    RevokeRequest, UpdateRegistrarRequest, UpdateServiceRequest,
};
use dw_engine::{Engine, EngineError, MasternodeKeyRole, NetworkSession};
use dw_vault::GrantPurpose;
use zeroize::Zeroizing;

use crate::pay::{credential, outpoint, wait_for_height, wallet_id};

#[derive(Clone, Copy, ValueEnum)]
pub enum NodeType {
    Regular,
    Evo,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Role {
    Owner,
    Voting,
    Operator,
    Platform,
    OwnerPayout,
    OperatorPayout,
}

impl From<Role> for MasternodeKeyRole {
    fn from(r: Role) -> Self {
        match r {
            Role::Owner => Self::Owner,
            Role::Voting => Self::Voting,
            Role::Operator => Self::Operator,
            Role::Platform => Self::PlatformNode,
            Role::OwnerPayout => Self::OwnerPayout,
            Role::OperatorPayout => Self::OperatorPayout,
        }
    }
}

/// Platform options shared by register and update-service.
#[derive(clap::Args, Clone, Default)]
pub struct PlatformArgs {
    /// Tenderdash node id (40 hex), EvoNodes only.
    #[arg(long)]
    platform_node_id: Option<String>,
    /// Platform P2P port.
    #[arg(long)]
    platform_p2p_port: Option<u16>,
    /// Platform HTTPS port.
    #[arg(long)]
    platform_http_port: Option<u16>,
}

impl PlatformArgs {
    fn fields(&self, node_id_default: Option<String>) -> Option<PlatformFields> {
        let node_id = self.platform_node_id.clone().or(node_id_default)?;
        Some(PlatformFields {
            node_id_hex: node_id,
            p2p_addresses: self
                .platform_p2p_port
                .map(|p| p.to_string())
                .into_iter()
                .collect(),
            https_addresses: self
                .platform_http_port
                .map(|p| p.to_string())
                .into_iter()
                .collect(),
        })
    }
}

#[derive(Subcommand)]
pub enum MnCommand {
    /// The masternode list: `mn <proTxHash> type=… status=… service=…
    /// owned=<roles> registered=… collateral=… owner=… voting=… payout=…`.
    /// Starts SPV and waits for `--sync-height` first.
    #[command(name = "mn-list")]
    List {
        #[arg(long)]
        sync_height: Option<u32>,
        /// Wallet whose scan height `--sync-height` waits for.
        #[arg(long)]
        wallet: Option<String>,
        #[arg(long)]
        owned: bool,
        #[arg(long)]
        hide_banned: bool,
        #[arg(long)]
        text: Option<String>,
        #[arg(long, value_enum)]
        node_type: Option<NodeType>,
    },
    /// List availability: `mnstate available=… height=… total=… enabled=…`.
    #[command(name = "mn-state")]
    State,
    /// Details of one masternode.
    #[command(name = "mn-info")]
    Info {
        pro_tx_hash: String,
        #[arg(long)]
        sync_height: Option<u32>,
        #[arg(long)]
        wallet: Option<String>,
    },
    /// Register a masternode (QT-123). Prints `prepared …`, the generated
    /// operator secret (`operator_secret <hex>`), then `broadcast
    /// <proTxHash>` once the network accepted it.
    #[command(name = "mn-register")]
    Register {
        wallet: String,
        #[arg(long, value_enum, default_value = "regular")]
        node_type: NodeType,
        /// `fund-new` or an outpoint `txid:vout` of the wallet.
        #[arg(long, default_value = "fund-new")]
        collateral: String,
        /// `ip:port`; omit to register without a service.
        #[arg(long)]
        service: Option<String>,
        #[arg(long)]
        payout: String,
        #[arg(long)]
        owner: Option<String>,
        #[arg(long)]
        voting: Option<String>,
        /// An existing operator public key (96 hex); default: generate one.
        #[arg(long)]
        operator_pubkey: Option<String>,
        /// Operator reward in hundredths of a percent.
        #[arg(long, default_value_t = 0)]
        reward: u16,
        #[command(flatten)]
        platform: PlatformArgs,
        /// Fee source address; default: any spendable coin.
        #[arg(long)]
        fee_source: Option<String>,
        #[arg(long)]
        sync_height: Option<u32>,
        /// Prepare only: print the raw transaction and abandon it.
        #[arg(long)]
        no_broadcast: bool,
        /// Type these as the last-4 confirmation instead of the right ones
        /// (checks the gate refuses).
        #[arg(long)]
        wrong_last4: Option<String>,
    },
    /// Update Service (QT-125). The operator secret comes from
    /// `--operator-secret-file` (first line) or the wallet/attached key.
    #[command(name = "mn-update-service")]
    UpdateService {
        wallet: String,
        pro_tx_hash: String,
        #[arg(long)]
        service: String,
        #[arg(long)]
        operator_secret_file: Option<PathBuf>,
        #[arg(long)]
        operator_payout: Option<String>,
        #[command(flatten)]
        platform: PlatformArgs,
        #[arg(long)]
        fee_source: Option<String>,
        #[arg(long)]
        sync_height: Option<u32>,
    },
    /// Update Registrar (QT-125): only the given fields change.
    #[command(name = "mn-update-registrar")]
    UpdateRegistrar {
        wallet: String,
        pro_tx_hash: String,
        #[arg(long)]
        operator_pubkey: Option<String>,
        #[arg(long)]
        voting: Option<String>,
        #[arg(long)]
        payout: Option<String>,
        #[arg(long)]
        fee_source: Option<String>,
        #[arg(long)]
        sync_height: Option<u32>,
    },
    /// Revoke (QT-125), reason 0–3.
    #[command(name = "mn-revoke")]
    Revoke {
        wallet: String,
        pro_tx_hash: String,
        #[arg(long, default_value_t = 0)]
        reason: u16,
        #[arg(long)]
        operator_secret_file: Option<PathBuf>,
        #[arg(long)]
        fee_source: Option<String>,
        #[arg(long)]
        sync_height: Option<u32>,
    },
    /// Collateral candidates: `candidate <txid:vout> <amount> conf=… refusal=…`.
    #[command(name = "mn-collaterals")]
    Collaterals {
        wallet: String,
        #[arg(long, value_enum, default_value = "regular")]
        node_type: NodeType,
        #[arg(long)]
        sync_height: Option<u32>,
    },
    /// Keychain (IOS-083): `key <index> path=… address=… public=… used_by=…`.
    #[command(name = "mn-keys")]
    Keys {
        wallet: String,
        #[arg(long, value_enum)]
        role: Role,
        #[arg(long, default_value_t = 0)]
        start: u32,
        #[arg(long, default_value_t = 5)]
        count: u32,
        /// Also reveal key `--start` (RevealSecret grant).
        #[arg(long)]
        reveal: bool,
    },
    /// Track a masternode (IOS-082).
    #[command(name = "mn-track")]
    Track {
        pro_tx_hash: String,
        #[arg(long)]
        label: Option<String>,
    },
    /// Tracked masternodes: `tracked <proTxHash> label=… attached=…`.
    #[command(name = "mn-tracked")]
    Tracked,
    #[command(name = "mn-untrack")]
    Untrack { pro_tx_hash: String },
    /// Attach a key (first line of `--key-file`) to a tracked masternode.
    #[command(name = "mn-attach")]
    Attach {
        pro_tx_hash: String,
        #[arg(long, value_enum)]
        role: Role,
        #[arg(long)]
        key_file: PathBuf,
        /// Wallet the MasternodeOp grant is bound to.
        #[arg(long)]
        wallet: String,
    },
    /// Raw v24 shared-masternode transactions built by dw-protx's codecs,
    /// for `decoderawtransaction` checks: `vector <kind> <hex>`.
    #[command(name = "mn-shared-vectors")]
    SharedVectors,
}

fn type_name(t: MasternodeType) -> &'static str {
    match t {
        MasternodeType::Regular => "regular",
        MasternodeType::Evo => "evo",
    }
}

fn print_row(r: &MasternodeRow) {
    let owned: Vec<String> = r.owned_roles.iter().map(|o| format!("{o:?}")).collect();
    println!(
        "mn {} type={} status={} service={} owned={} registered={} collateral={} owner={} \
         voting={} payout={} operator={} reward={} label={}",
        r.pro_tx_hash,
        type_name(r.node_type),
        match r.status {
            dw_engine::masternodes::MasternodeListStatus::Active { .. } => "active",
            dw_engine::masternodes::MasternodeListStatus::Banned { .. } => "banned",
            dw_engine::masternodes::MasternodeListStatus::Retired => "retired",
            dw_engine::masternodes::MasternodeListStatus::Unknown => "unknown",
        },
        r.service.as_deref().unwrap_or("-"),
        if owned.is_empty() {
            "-".to_string()
        } else {
            owned.join(",")
        },
        r.registered_height.map_or("-".into(), |h| h.to_string()),
        r.collateral.map_or("-".into(), |c| c.to_string()),
        r.owner_address.as_deref().unwrap_or("-"),
        if r.voting_address.is_empty() {
            "-"
        } else {
            &r.voting_address
        },
        r.payout_addresses
            .first()
            .map(String::as_str)
            .unwrap_or("-"),
        r.operator_public_key,
        r.operator_reward
            .as_ref()
            .map_or("-".into(), |o| o.percent_x100.to_string()),
        r.label.as_deref().unwrap_or("-"),
    );
}

fn fee_source(text: Option<String>) -> FeeSourceChoice {
    match text {
        Some(a) => FeeSourceChoice::Address(a),
        None => FeeSourceChoice::Automatic,
    }
}

/// The first line of a file, as bytes in a zeroing buffer.
fn read_secret(path: &std::path::Path) -> Result<Zeroizing<Vec<u8>>, String> {
    crate::read_passphrase(path)
}

fn sync_to(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    wallet: Option<&str>,
    height: Option<u32>,
) -> Result<(), String> {
    if let Some(h) = height {
        let id = match wallet {
            Some(w) => wallet_id(w)?,
            None => crate::first_wallet(session).map_err(|e| e.to_string())?,
        };
        wait_for_height(engine, session, id, h, Duration::from_secs(240))?;
    } else if !session.spv_running().map_err(|e| e.to_string())? {
        engine
            .block_on(session.start_spv())
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn grant(
    session: &Arc<NetworkSession>,
    passphrase: Option<&Zeroizing<Vec<u8>>>,
    wallet: &dw_engine::WalletId,
    purpose: GrantPurpose,
) -> Result<String, String> {
    session
        .vault()
        .authorize(purpose, Some(&wallet.0), credential(session, passphrase))
        .map(|g| g.id)
        .map_err(|e| e.to_string())
}

pub fn run(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    passphrase: Option<&Zeroizing<Vec<u8>>>,
    cmd: MnCommand,
) -> Result<(), String> {
    let e = |e: EngineError| e.to_string();
    match cmd {
        MnCommand::List {
            sync_height,
            wallet,
            owned,
            hide_banned,
            text,
            node_type,
        } => {
            sync_to(engine, session, wallet.as_deref(), sync_height)?;
            let query = MasternodeQuery {
                type_filter: match node_type {
                    None => MasternodeTypeFilter::All,
                    Some(NodeType::Regular) => MasternodeTypeFilter::Regular,
                    Some(NodeType::Evo) => MasternodeTypeFilter::Evo,
                },
                text,
                owned_only: owned,
                hide_banned,
            };
            for r in engine.block_on(session.masternodes(query)).map_err(e)? {
                print_row(&r);
            }
        }
        MnCommand::State => {
            let s = session.masternode_list_state().map_err(e)?;
            println!(
                "mnstate available={} height={} total={} enabled={} evo_total={} evo_enabled={} syncing={}",
                u8::from(s.available),
                s.height.map_or("-".into(), |h| h.to_string()),
                s.total,
                s.enabled,
                s.evo_total,
                s.evo_enabled,
                u8::from(s.syncing)
            );
        }
        MnCommand::Info {
            pro_tx_hash,
            sync_height,
            wallet,
        } => {
            sync_to(engine, session, wallet.as_deref(), sync_height)?;
            let d = engine
                .block_on(session.masternode_detail(pro_tx_hash))
                .map_err(e)?;
            print_row(&d.row);
            println!(
                "detail network={} platform_p2p={} platform_https={} revoked={} wallet_txs={} pose_score={}",
                d.network_addresses.join(","),
                d.platform_p2p_addresses.join(","),
                d.platform_https_addresses.join(","),
                d.revocation_reason.map_or("-".into(), |r| r.to_string()),
                d.wallet_transactions,
                d.row.pose_score.map_or("-".into(), |p| p.to_string()),
            );
        }
        MnCommand::Register {
            wallet,
            node_type,
            collateral,
            service,
            payout,
            owner,
            voting,
            operator_pubkey,
            reward,
            platform,
            fee_source: fee,
            sync_height,
            no_broadcast,
            wrong_last4,
        } => {
            let id = wallet_id(&wallet)?;
            sync_to(engine, session, Some(&wallet), sync_height)?;
            let node_type = match node_type {
                NodeType::Regular => MasternodeType::Regular,
                NodeType::Evo => MasternodeType::Evo,
            };
            let request = RegistrationRequest {
                wallet_id: id,
                node_type,
                collateral: if collateral == "fund-new" {
                    CollateralChoice::FundNew
                } else {
                    CollateralChoice::ExistingUtxo(outpoint(&collateral)?)
                },
                service_addresses: service.into_iter().collect(),
                owner_address: owner,
                voting_address: voting,
                operator_key: match operator_pubkey {
                    Some(k) => OperatorKeyChoice::Existing(k),
                    None => OperatorKeyChoice::Generate,
                },
                payout_address: payout,
                operator_reward_x100: reward,
                platform: platform.fields(None),
                fee_source: fee_source(fee),
            };
            let g = grant(session, passphrase, &id, GrantPurpose::MasternodeOp)?;
            let prepared = engine
                .block_on(session.prepare_registration(request, g))
                .map_err(e)?;
            let s = prepared.summary().clone();
            println!(
                "prepared txid={} fee={} total={} collateral={} collateral_address={} owner={} voting={} payout={} operator={} secret_required={}",
                s.pro_tx_hash,
                s.fee,
                s.total_spent,
                s.collateral,
                s.collateral_address,
                s.owner_address,
                s.voting_address,
                s.payout_address,
                s.operator_public_key,
                u8::from(s.operator_secret_required)
            );
            if no_broadcast {
                if let Some(raw) = prepared.raw() {
                    println!("raw {}", hex::encode(raw));
                }
                engine.block_on(prepared.abandon()).map_err(e)?;
                println!("abandoned {}", s.pro_tx_hash);
                return Ok(());
            }
            if s.operator_secret_required {
                // Headless driver: the secret is printed so the test can
                // configure the node and sign later updates, as `bls
                // generate` prints it.
                let (secret, _) = prepared.operator_secret().map_err(e)?;
                let text = String::from_utf8_lossy(&secret).to_string();
                println!("operator_secret {text}");
                // QT-124: submit is refused before the gate.
                match engine.block_on(prepared.submit(None)) {
                    Err(EngineError::Masternode(
                        dw_engine::MasternodeFailure::OperatorSecretUnconfirmed,
                    )) => println!("gate closed"),
                    other => return Err(format!("submit before the gate: {other:?}")),
                }
                if let Some(wrong) = wrong_last4 {
                    let ok = prepared.confirm_operator_secret(&wrong).map_err(e)?;
                    println!("gate wrong_last4_accepted={}", u8::from(ok));
                }
                if !prepared
                    .confirm_operator_secret(&text[text.len() - 4..])
                    .map_err(e)?
                {
                    return Err("the last four characters were refused".into());
                }
                println!("gate open");
            }
            let pro_tx_hash = engine.block_on(prepared.submit(None)).map_err(e)?;
            println!("broadcast {pro_tx_hash}");
        }
        MnCommand::UpdateService {
            wallet,
            pro_tx_hash,
            service,
            operator_secret_file,
            operator_payout,
            platform,
            fee_source: fee,
            sync_height,
        } => {
            let id = wallet_id(&wallet)?;
            sync_to(engine, session, Some(&wallet), sync_height)?;
            let secret = operator_secret_file
                .as_deref()
                .map(read_secret)
                .transpose()?;
            let g = grant(session, passphrase, &id, GrantPurpose::MasternodeOp)?;
            let prepared = engine
                .block_on(session.prepare_update_service(
                    UpdateServiceRequest {
                        pro_tx_hash,
                        service_addresses: vec![service],
                        operator_secret: secret,
                        platform: platform.fields(None),
                        operator_payout_address: operator_payout,
                        fee_source: fee_source(fee),
                        fee_wallet_id: id,
                    },
                    g,
                ))
                .map_err(e)?;
            broadcast_provider(engine, &prepared)?;
        }
        MnCommand::UpdateRegistrar {
            wallet,
            pro_tx_hash,
            operator_pubkey,
            voting,
            payout,
            fee_source: fee,
            sync_height,
        } => {
            let id = wallet_id(&wallet)?;
            sync_to(engine, session, Some(&wallet), sync_height)?;
            let g = grant(session, passphrase, &id, GrantPurpose::MasternodeOp)?;
            let prepared = engine
                .block_on(session.prepare_update_registrar(
                    UpdateRegistrarRequest {
                        pro_tx_hash,
                        operator_public_key: operator_pubkey,
                        voting_address: voting,
                        payout_address: payout,
                        fee_source: fee_source(fee),
                        fee_wallet_id: id,
                    },
                    g,
                ))
                .map_err(e)?;
            broadcast_provider(engine, &prepared)?;
        }
        MnCommand::Revoke {
            wallet,
            pro_tx_hash,
            reason,
            operator_secret_file,
            fee_source: fee,
            sync_height,
        } => {
            let id = wallet_id(&wallet)?;
            sync_to(engine, session, Some(&wallet), sync_height)?;
            let secret = operator_secret_file
                .as_deref()
                .map(read_secret)
                .transpose()?;
            let g = grant(session, passphrase, &id, GrantPurpose::MasternodeOp)?;
            let reason = match reason {
                0 => RevocationReason::NotSpecified,
                1 => RevocationReason::TerminationOfService,
                2 => RevocationReason::CompromisedKeys,
                3 => RevocationReason::ChangeOfKeys,
                other => return Err(format!("reason {other} is not 0..=3")),
            };
            let prepared = engine
                .block_on(session.prepare_revoke(
                    RevokeRequest {
                        pro_tx_hash,
                        operator_secret: secret,
                        reason,
                        fee_source: fee_source(fee),
                        fee_wallet_id: id,
                    },
                    g,
                ))
                .map_err(e)?;
            broadcast_provider(engine, &prepared)?;
        }
        MnCommand::Collaterals {
            wallet,
            node_type,
            sync_height,
        } => {
            sync_to(engine, session, Some(&wallet), sync_height)?;
            let node_type = match node_type {
                NodeType::Regular => MasternodeType::Regular,
                NodeType::Evo => MasternodeType::Evo,
            };
            for c in engine
                .block_on(session.collateral_candidates(wallet_id(&wallet)?, node_type))
                .map_err(e)?
            {
                println!(
                    "candidate {} {} conf={} refusal={}",
                    c.outpoint,
                    c.amount,
                    c.confirmations,
                    c.refusal.map_or("-".into(), |r| format!("{r:?}"))
                );
            }
        }
        MnCommand::Keys {
            wallet,
            role,
            start,
            count,
            reveal,
        } => {
            let id = wallet_id(&wallet)?;
            for k in engine
                .block_on(session.masternode_keys(id, role.into(), start, count))
                .map_err(e)?
            {
                let used: Vec<String> = k.used_by.iter().map(|u| u.pro_tx_hash.clone()).collect();
                println!(
                    "key {} path={} address={} public={} legacy={} node_id={} used_by={}",
                    k.index,
                    k.derivation_path,
                    k.address.as_deref().unwrap_or("-"),
                    k.public_key_hex,
                    k.legacy_public_key_hex.as_deref().unwrap_or("-"),
                    k.platform_node_id.as_deref().unwrap_or("-"),
                    if used.is_empty() {
                        "-".into()
                    } else {
                        used.join(",")
                    }
                );
            }
            if reveal {
                let g = grant(session, passphrase, &id, GrantPurpose::RevealSecret)?;
                let r = engine
                    .block_on(session.reveal_masternode_key(Some(id), None, role.into(), start, g))
                    .map_err(e)?;
                println!(
                    "revealed {} wif={}",
                    String::from_utf8_lossy(&r.private_key_hex),
                    r.wif
                        .as_ref()
                        .map_or("-".into(), |w| String::from_utf8_lossy(w).to_string())
                );
            }
        }
        MnCommand::Track { pro_tx_hash, label } => {
            let t = engine
                .block_on(session.track_masternode(pro_tx_hash, label))
                .map_err(e)?;
            println!(
                "tracked {} label={}",
                t.row.pro_tx_hash,
                t.label.as_deref().unwrap_or("-")
            );
        }
        MnCommand::Tracked => {
            for t in engine.block_on(session.tracked_masternodes()).map_err(e)? {
                let roles: Vec<String> =
                    t.attached_roles.iter().map(|r| format!("{r:?}")).collect();
                println!(
                    "tracked {} label={} attached={} update_service={}",
                    t.row.pro_tx_hash,
                    t.label.as_deref().unwrap_or("-"),
                    if roles.is_empty() {
                        "-".into()
                    } else {
                        roles.join(",")
                    },
                    u8::from(t.capabilities.can_update_service)
                );
            }
        }
        MnCommand::Untrack { pro_tx_hash } => {
            let removed = engine
                .block_on(session.untrack_masternode(pro_tx_hash))
                .map_err(e)?;
            println!("untracked {}", u8::from(removed));
        }
        MnCommand::Attach {
            pro_tx_hash,
            role,
            key_file,
            wallet,
        } => {
            let id = wallet_id(&wallet)?;
            let key = read_secret(&key_file)?;
            let g = grant(session, passphrase, &id, GrantPurpose::MasternodeOp)?;
            engine
                .block_on(session.attach_masternode_key(pro_tx_hash, role.into(), key, g))
                .map_err(e)?;
            println!("attached");
        }
        MnCommand::SharedVectors => shared_vectors(),
    }
    Ok(())
}

fn broadcast_provider(
    engine: &Engine,
    prepared: &Arc<dw_engine::masternodes::PreparedProviderTx>,
) -> Result<(), String> {
    let s = prepared.summary().clone();
    println!(
        "prepared txid={} fee={} kind={:?} bans={}",
        s.txid,
        s.fee,
        s.kind,
        u8::from(s.bans_masternode)
    );
    let txid = engine
        .block_on(prepared.broadcast())
        .map_err(|e| e.to_string())?;
    println!("broadcast {txid}");
    Ok(())
}

/// One transaction per v24 payload type, with fixed field values, for the
/// regtest suite to decode with dashd.
fn shared_vectors() {
    use dashcore::{OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid};
    use dw_protx::shared::{
        ProDisTx, ProUpShareTx, ProUpSharedRegTx, TRANSACTION_PROVIDER_DISSOLVE,
        TRANSACTION_PROVIDER_UPDATE_SHARE, TRANSACTION_PROVIDER_UPDATE_SHARED_REGISTRAR,
        encode_special_tx, payload_bytes,
    };
    let pro_tx_hash = Txid::from_byte_array([0x11; 32]);
    let tx = Transaction {
        version: 3,
        lock_time: 0,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: Txid::from_byte_array([0x22; 32]),
                vout: 1,
            },
            ..Default::default()
        }],
        output: vec![TxOut {
            value: 1_000,
            script_pubkey: ScriptBuf::from_bytes(
                hex::decode("76a914000102030405060708090a0b0c0d0e0f1011121388ac")
                    .expect("static script"),
            ),
        }],
        special_transaction_payload: None,
    };
    let (basic, _) = dw_protx::bls::public_keys(&[7u8; 32]).expect("static key");
    let dis = ProDisTx {
        version: ProDisTx::CURRENT_VERSION,
        pro_tx_hash,
        actor_index: 1,
        sigs: vec![[0x33; 65]],
    };
    let share = ProUpShareTx {
        version: ProUpShareTx::CURRENT_VERSION,
        pro_tx_hash,
        share_index: 2,
        reward_script: tx.output[0].script_pubkey.clone(),
        inputs_hash: tx.hash_inputs().to_byte_array(),
        sig: vec![0x44; 65],
    };
    let reg = ProUpSharedRegTx {
        version: ProUpSharedRegTx::CURRENT_VERSION,
        pro_tx_hash,
        operator_public_key: basic,
        voting_key_id: [0x55; 20],
        inputs_hash: tx.hash_inputs().to_byte_array(),
        sigs: vec![[0x66; 65], [0x77; 65]],
    };
    for (kind, ty, payload) in [
        (
            "prodistx",
            TRANSACTION_PROVIDER_DISSOLVE,
            payload_bytes(&dis),
        ),
        (
            "proupsharetx",
            TRANSACTION_PROVIDER_UPDATE_SHARE,
            payload_bytes(&share),
        ),
        (
            "proupsharedregtx",
            TRANSACTION_PROVIDER_UPDATE_SHARED_REGISTRAR,
            payload_bytes(&reg),
        ),
    ] {
        println!(
            "vector {kind} {}",
            hex::encode(encode_special_tx(&tx, ty, &payload))
        );
    }
    println!(
        "expect pro_tx_hash={pro_tx_hash} operator={} inputs_hash={}",
        hex::encode(basic),
        tx.hash_inputs()
    );
}
