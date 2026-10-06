//! PSBT flows (QT-076…079; docs/contracts/m2-engine.md §2.8): dash-qt's
//! "Create Unsigned" from a draft, the PSBT Operations dialog's analysis,
//! signing with the vault and broadcasting. The PSBT codec, analysis rules,
//! signing and finalizing live in dw-psbt; this module adds what the wallet
//! knows (which scripts are its own, their derivation paths, the previous
//! transactions) and the grant and broadcast rules of the send flow.

use std::collections::BTreeMap;
use std::sync::Arc;

use dashcore::{Address, Transaction, Txid};
use dw_psbt::{InputData, KeyPaths, PartiallySignedTransaction, Status};
use dw_vault::{GrantKind, VaultError};
use key_wallet::bip32::{DerivationPath, Fingerprint};
use key_wallet::managed_account::address_pool::PublicKeyType;
use key_wallet::wallet::managed_wallet_info::ManagedWalletInfo;
use platform_wallet::broadcaster::{BroadcastError, SpvBroadcaster, TransactionBroadcaster};

use super::{ChangeTarget, TxDraft, check_against_plan, reason_means_no_peers, vault_failure};
use crate::{EngineError, NetworkSession, WalletId};

/// dw-appdb setting (wallet scope) holding the hex BIP32 fingerprint of the
/// wallet's master key, written when the seed is imported.
pub(crate) const FINGERPRINT_SETTING: &str = "bip32.master_fingerprint";
/// How long `broadcast_psbt` waits for dash-spv to have broadcast peers.
const BROADCAST_READY_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// Why a PSBT call failed (`psbt.*` codes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PsbtFailure {
    Invalid(String),
    TooLarge(u64),
    NotComplete,
    FeeRateTooHigh {
        duffs_per_kb: u64,
    },
    /// Signing refused: the fee is above [`MAX_TX_FEE`](super::MAX_TX_FEE)
    /// (`send.absurd_fee`'s bound).
    AbsurdFee {
        fee: u64,
    },
    /// Signing refused: an input's previous transaction is missing or is not
    /// the one the input names, so the fee and the wallet's outflow are
    /// unknown.
    FeeUnknown,
    WatchOnly,
    GrantExceeded {
        max_duffs: u64,
    },
    NoPeers,
    BroadcastRejected {
        reason: String,
    },
    BroadcastUnknown {
        reason: String,
    },
}

impl std::fmt::Display for PsbtFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(d) => write!(f, "invalid psbt: {d}"),
            Self::TooLarge(n) => write!(f, "psbt too large: {n} bytes"),
            Self::NotComplete => f.write_str("psbt not complete"),
            Self::FeeRateTooHigh { duffs_per_kb } => {
                write!(f, "fee rate {duffs_per_kb} duff/kB too high")
            }
            Self::AbsurdFee { fee } => write!(f, "fee {fee} duffs is absurdly high"),
            Self::FeeUnknown => {
                f.write_str("the fee is unknown: an input lacks its verified previous transaction")
            }
            Self::WatchOnly => f.write_str("watch-only wallet"),
            Self::GrantExceeded { max_duffs } => write!(f, "grant cap {max_duffs} exceeded"),
            Self::NoPeers => f.write_str("no peers"),
            Self::BroadcastRejected { reason } => write!(f, "rejected: {reason}"),
            Self::BroadcastUnknown { reason } => write!(f, "outcome unknown: {reason}"),
        }
    }
}

impl From<PsbtFailure> for EngineError {
    fn from(f: PsbtFailure) -> Self {
        EngineError::Psbt(f)
    }
}

impl From<dw_psbt::PsbtError> for PsbtFailure {
    fn from(e: dw_psbt::PsbtError) -> Self {
        match e {
            dw_psbt::PsbtError::TooLarge(n) => PsbtFailure::TooLarge(n as u64),
            dw_psbt::PsbtError::NotComplete => PsbtFailure::NotComplete,
            other => PsbtFailure::Invalid(other.to_string()),
        }
    }
}

/// One output line (" * Sends %1 to %2", " (own address)").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PsbtOutputInfo {
    pub address: Option<String>,
    pub amount: u64,
    pub is_mine: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PsbtSignability {
    NoWallet,
    WatchOnly,
    NoMatchingKeys,
    CanSign,
}

/// The PSBT Operations dialog content (QT-079).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PsbtAnalysis {
    pub outputs: Vec<PsbtOutputInfo>,
    pub fee: Option<u64>,
    /// What leaves the wallet: its inputs minus the outputs paying it (the
    /// outputs not paying it plus the fee when every input is the wallet's;
    /// every output plus the fee without a wallet); `None` like `fee`. A
    /// signing `Spend` grant must cover it.
    pub total: Option<u64>,
    pub unsigned_inputs: u32,
    pub status: Status,
    pub signability: PsbtSignability,
    /// Value paid to scripts the wallet does not own; `None` without a
    /// wallet.
    pub external_sent: Option<u64>,
}

/// What the wallet knows about a PSBT: the derivation paths of the inputs
/// it can sign, and which outputs pay it.
struct Ownership {
    paths: KeyPaths,
    outputs_mine: Vec<bool>,
}

impl Ownership {
    /// The wallet's net outflow: the value of its inputs (from verified
    /// previous transactions only, see [`dw_psbt::spent_output`]) minus the
    /// outputs paying it. With every input the wallet's this is the value
    /// sent to others plus the fee. `None` on overflow.
    fn outflow(&self, psbt: &PartiallySignedTransaction) -> Option<u64> {
        let spent = self.paths.keys().try_fold(0u64, |sum, &i| {
            sum.checked_add(dw_psbt::spent_output(psbt, i)?.value)
        })?;
        let back = psbt
            .unsigned_tx
            .output
            .iter()
            .zip(&self.outputs_mine)
            .filter(|(_, mine)| **mine)
            .try_fold(0u64, |sum, (o, _)| sum.checked_add(o.value))?;
        Some(spent.saturating_sub(back))
    }
}

fn ownership(
    info: &ManagedWalletInfo,
    psbt: &PartiallySignedTransaction,
    network: dashcore::Network,
) -> Ownership {
    let path_of = |script: &dashcore::ScriptBuf| -> Option<DerivationPath> {
        let address = Address::from_script(script, network).ok()?;
        info.all_managed_accounts()
            .into_iter()
            .find_map(|account| account.get_address_info(&address))
            .map(|i| i.path)
    };
    let mut paths = BTreeMap::new();
    for i in 0..psbt.inputs.len() {
        if let Some(out) = dw_psbt::spent_output(psbt, i)
            && let Some(path) = path_of(&out.script_pubkey)
        {
            paths.insert(i, path);
        }
    }
    let outputs_mine = psbt
        .unsigned_tx
        .output
        .iter()
        .map(|o| {
            Address::from_script(&o.script_pubkey, network)
                .is_ok_and(|a| crate::coins::wallet_owns(info, &a))
        })
        .collect();
    Ownership {
        paths,
        outputs_mine,
    }
}

/// The key data of `address` for a PSBT derivation record.
fn derivation_of(
    info: &ManagedWalletInfo,
    address: &Address,
    fingerprint: Option<Fingerprint>,
) -> Option<(dashcore::secp256k1::PublicKey, Fingerprint, DerivationPath)> {
    let fp = fingerprint?;
    let a = info
        .all_managed_accounts()
        .into_iter()
        .find_map(|account| account.get_address_info(address))?;
    let pk = match a.public_key? {
        PublicKeyType::ECDSA(bytes) => dashcore::secp256k1::PublicKey::from_slice(&bytes).ok()?,
        _ => return None,
    };
    Some((pk, fp, a.path))
}

impl TxDraft {
    /// dash-qt "Create Unsigned" (QT-076/077): plans the draft as `estimate`
    /// does and returns it as a PSBT with each input's previous transaction
    /// and the BIP32 derivations of the wallet's keys (when the wallet's
    /// master fingerprint is known). Signs and reserves nothing and needs no
    /// grant, so watch-only wallets can use it. With automatic change the
    /// next change address is used, as `prepare` does.
    pub async fn create_unsigned(
        self: &Arc<Self>,
    ) -> Result<PartiallySignedTransaction, EngineError> {
        let this = Arc::clone(self);
        self.session
            .on_runtime(async move {
                let _op = this.session.enter().await?;
                let session = &this.session;
                let r = this.resolve().await?;
                let wallet = session.wallet(&this.wallet_id).await?;
                let change = match (&r.change, r.plan.change) {
                    (ChangeTarget::Address(a), _) => a.clone(),
                    (ChangeTarget::Auto, Some(_)) => {
                        wallet.core().next_change_address_for_account(0).await?
                    }
                    (ChangeTarget::Auto, None) => r.plan.inputs[0].address.clone(),
                };
                let (tx, _, _) = r.builder(&change).build_unsigned_reserved().map_err(|e| {
                    EngineError::Internal(format!("key-wallet cannot build the plan: {e}"))
                })?;
                check_against_plan(&tx, &r.plan, &change).map_err(|m| m.into_error())?;
                let fingerprint = session.master_fingerprint(&this.wallet_id).await;
                let history = session.hub.history.snapshot(&this.wallet_id);
                let state = wallet.state().await;
                let info = &state.core_wallet;
                let network = this.network();
                let inputs = tx
                    .input
                    .iter()
                    .map(|txin| {
                        let prev = history
                            .get(&txin.previous_output.txid)
                            .map(|e| e.tx.clone())
                            .ok_or_else(|| {
                                EngineError::Internal(format!(
                                    "previous transaction {} is not in the wallet's history",
                                    txin.previous_output.txid
                                ))
                            })?;
                        let derivation = prev
                            .output
                            .get(txin.previous_output.vout as usize)
                            .and_then(|o| Address::from_script(&o.script_pubkey, network).ok())
                            .and_then(|a| derivation_of(info, &a, fingerprint));
                        Ok(InputData {
                            prev_tx: prev,
                            derivation,
                        })
                    })
                    .collect::<Result<Vec<_>, EngineError>>()?;
                let outputs = tx
                    .output
                    .iter()
                    .map(|o| {
                        Address::from_script(&o.script_pubkey, network)
                            .ok()
                            .and_then(|a| derivation_of(info, &a, fingerprint))
                    })
                    .collect();
                drop(state);
                dw_psbt::create_unsigned(tx, inputs, outputs)
                    .map_err(|e| EngineError::Internal(format!("PSBT: {e}")))
            })
            .await
    }
}

impl NetworkSession {
    /// The stored master key fingerprint of a wallet, if any.
    async fn master_fingerprint(&self, id: &WalletId) -> Option<Fingerprint> {
        let appdb = self.live().ok()?.appdb;
        let key = id.to_string();
        let hex = tokio::task::spawn_blocking(move || appdb.setting(&key, FINGERPRINT_SETTING))
            .await
            .ok()?
            .ok()??;
        let bytes: [u8; 4] = hex::decode(hex).ok()?.try_into().ok()?;
        Some(Fingerprint::from(bytes))
    }

    /// The PSBT Operations dialog content of `psbt` seen from `wallet`
    /// (`None`: "no wallet is loaded").
    pub async fn analyze_psbt(
        self: &Arc<Self>,
        wallet: Option<WalletId>,
        psbt: PartiallySignedTransaction,
    ) -> Result<PsbtAnalysis, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let network = this.network.core_network();
            let base = dw_psbt::analyze(&psbt, network);
            let Some(id) = wallet else {
                return Ok(PsbtAnalysis {
                    outputs: base
                        .outputs
                        .into_iter()
                        .map(|o| PsbtOutputInfo {
                            address: o.address,
                            amount: o.amount,
                            is_mine: false,
                        })
                        .collect(),
                    fee: base.fee,
                    total: base.total.zip(base.fee).map(|(outs, fee)| outs + fee),
                    unsigned_inputs: base.unsigned_inputs,
                    status: base.status,
                    signability: PsbtSignability::NoWallet,
                    external_sent: None,
                });
            };
            let w = this.wallet(&id).await?;
            let owned = {
                let state = w.state().await;
                ownership(&state.core_wallet, &psbt, network)
            };
            let external_sent = base
                .outputs
                .iter()
                .zip(&owned.outputs_mine)
                .filter(|(_, mine)| !**mine)
                .map(|(o, _)| o.amount)
                .sum::<u64>();
            let signable = owned
                .paths
                .keys()
                .filter(|i| !dw_psbt::input_signed(&psbt, **i))
                .count();
            let total = base.fee.and(owned.outflow(&psbt));
            let signability = if !this.vault.has_wallet_secret(&id.0) {
                PsbtSignability::WatchOnly
            } else if signable == 0 {
                PsbtSignability::NoMatchingKeys
            } else {
                PsbtSignability::CanSign
            };
            Ok(PsbtAnalysis {
                outputs: base
                    .outputs
                    .into_iter()
                    .zip(owned.outputs_mine)
                    .map(|(o, is_mine)| PsbtOutputInfo {
                        address: o.address,
                        amount: o.amount,
                        is_mine,
                    })
                    .collect(),
                total,
                fee: base.fee,
                unsigned_inputs: base.unsigned_inputs,
                status: base.status,
                signability,
                external_sent: Some(external_sent),
            })
        })
        .await
    }

    /// "Sign Tx": signs every input the wallet owns with the vault and
    /// returns the new PSBT. Needs a `Spend` grant for the wallet whose cap
    /// covers the wallet's net outflow (its inputs minus the outputs paying
    /// it: what it sends plus the fee), redeemed after the checks. Every
    /// amount comes from txid-verified previous transactions
    /// (`witness_utxo` is ignored); with any input's unknown the fee is
    /// unknown and nothing is signed (`FeeUnknown`). A fee above
    /// [`MAX_TX_FEE`](super::MAX_TX_FEE) is refused (`AbsurdFee`), as the
    /// send flow does: a signed PSBT can be broadcast anywhere, past
    /// `broadcast_psbt`'s rate check.
    pub async fn sign_psbt(
        self: &Arc<Self>,
        wallet_id: WalletId,
        mut psbt: PartiallySignedTransaction,
        grant_id: String,
    ) -> Result<PartiallySignedTransaction, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let w = this.wallet(&wallet_id).await?;
            if !this.vault.has_wallet_secret(&wallet_id.0) {
                return Err(PsbtFailure::WatchOnly.into());
            }
            if let Err(e @ (VaultError::NoVault | VaultError::Locked | VaultError::MixingOnly)) =
                this.vault
                    .check_grant(&grant_id, GrantKind::Spend, Some(&wallet_id.0))
            {
                return Err(vault_failure(e));
            }
            let network = this.network.core_network();
            let owned = {
                let state = w.state().await;
                ownership(&state.core_wallet, &psbt, network)
            };
            let fee = dw_psbt::analyze(&psbt, network)
                .fee
                .ok_or(PsbtFailure::FeeUnknown)?;
            if fee > super::MAX_TX_FEE {
                return Err(PsbtFailure::AbsurdFee { fee }.into());
            }
            let outflow = owned.outflow(&psbt).ok_or(PsbtFailure::FeeUnknown)?;
            let vault = this.vault.clone();
            let signer = tokio::task::spawn_blocking(move || {
                let token = vault.redeem_grant(&grant_id, GrantKind::Spend, Some(&wallet_id.0))?;
                let max_duffs = token.max_duffs().unwrap_or(0);
                if outflow > max_duffs {
                    return Ok(Err(PsbtFailure::GrantExceeded { max_duffs }));
                }
                vault.signer(&wallet_id.0, &token).map(Ok)
            })
            .await?
            .map_err(vault_failure)??;
            dw_psbt::sign(&mut psbt, &owned.paths, &signer)
                .await
                .map_err(PsbtFailure::from)?;
            Ok(psbt)
        })
        .await
    }

    /// "Broadcast Tx": finalizes a complete PSBT, refuses fee rates above
    /// 0.1 DASH/kB (from txid-verified input values only; an input without
    /// one is `Invalid`) and broadcasts with the send flow's verdict rules
    /// (accepted; never sent: `NoPeers` / `BroadcastRejected`; no verdict:
    /// `BroadcastUnknown`). Returns the txid.
    pub async fn broadcast_psbt(
        self: &Arc<Self>,
        mut psbt: PartiallySignedTransaction,
    ) -> Result<Txid, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            if !dw_psbt::finalize(&mut psbt) {
                return Err(PsbtFailure::NotComplete.into());
            }
            let tx: Transaction = dw_psbt::extract(&psbt).map_err(PsbtFailure::from)?;
            let rate = dw_psbt::fee_rate_per_kb(&psbt, &tx).ok_or_else(|| {
                PsbtFailure::Invalid(
                    "an input's previous transaction is missing or does not match it".into(),
                )
            })?;
            if rate > dw_psbt::MAX_BROADCAST_FEE_PER_KB {
                return Err(PsbtFailure::FeeRateTooHigh { duffs_per_kb: rate }.into());
            }
            let manager = this.manager()?;
            if !manager.spv().is_started() {
                return Err(PsbtFailure::NoPeers.into());
            }
            let broadcaster = SpvBroadcaster::new(manager.spv_arc());
            // A loaded PSBT may be sent right after SPV starts: give dash-spv
            // a bounded time to connect. Without peers the broadcast below
            // reports `NoPeers` (nothing sent).
            broadcaster.wait_until_ready(BROADCAST_READY_WAIT).await;
            match broadcaster.broadcast(&tx).await {
                Ok(txid) => Ok(txid),
                Err(BroadcastError::MaybeSent { reason }) => {
                    Err(PsbtFailure::BroadcastUnknown { reason }.into())
                }
                Err(BroadcastError::Rejected { reason }) => {
                    Err(if reason_means_no_peers(&reason) {
                        PsbtFailure::NoPeers
                    } else {
                        PsbtFailure::BroadcastRejected { reason }
                    }
                    .into())
                }
            }
        })
        .await
    }
}
