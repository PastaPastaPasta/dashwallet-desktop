//! Building a ProTx (MG-04 §4): the funding plan, the BIP69 assembly, the
//! payload's finalization and the inputs' signatures. Pure functions where
//! the wallet isn't needed, so the oracle and the mutants reach them.
//!
//! The wallet signs the collateral's message (an existing coin of its own),
//! the inputs, and (slice 4) a ProUpRegTx's owner signature, all with the
//! `MasternodeOwner` signer. Nothing here signs as operator.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use dashcore::address::Payload;
use dashcore::blockdata::transaction::special_transaction::TransactionPayload;
use dashcore::blockdata::transaction::special_transaction::provider_registration::ProviderRegistrationPayload;
use dashcore::hashes::Hash;
use dashcore::{Address, Network, OutPoint, PubkeyHash, ScriptBuf, Transaction, TxIn, TxOut};
use dw_protx::params::{MAX_OPERATOR_REWARD, collateral_amount, default_ports};
use dw_protx::payloads::{
    self, CollateralSource, PlatformPorts, RegistrationSigner, RegistrationTerms,
};
use dw_protx::rules::{self, ListedMasternode};
use dw_protx::service::{self, ServiceError};
use dw_vault::VaultSigner;
use key_wallet::DerivationPath;
use key_wallet::Utxo;
use key_wallet::wallet::managed_wallet_info::ManagedWalletInfo;
use key_wallet::wallet::managed_wallet_info::fee::FeeRate;
use platform_wallet::masternode::MasternodeListSummary;
use platform_wallet::wallet::platform_wallet::PlatformWallet;

use super::super::aggregate::OwnedMasternode;
use super::super::follow::{ListView, read_list, wallet_masternodes};
use super::super::protx_records::{
    CollateralChoice, CollateralKind, EvonodeTerms, OperatorChoice, PayoutChoice, PlatformFacts,
    PlatformNodeChoice, ProTxOperationState, ProTxTerms, RegistrationRequest, VotingChoice,
};
use super::super::records::MasternodeType;
use super::super::{MasternodesError, outpoint_string, parse_pro_tx_hash};
use super::keys::{Claim, InUse, parse_outpoint};
use super::ops::state_of;
use crate::coinjoin::{SIGHASH_ALL, sign_p2pkh_inputs};
use crate::coins::{CoinFence, CoinSnapshot, own_address_info, read_coins, wallet_owns};
use crate::send::plan::{
    INPUT_SIZE, MAX_INPUTS, dust_threshold, fee_for, output_size, varint_size,
};
use crate::send::{
    CoinSource, MAX_TX_FEE, MIN_FEE_PER_KB, PreparedInput, PreparedOutput, PreparedSummary,
    SendFailure, l1_address,
};
use crate::{EngineError, NetworkSession, WalletId};

/// `nSequence` of every input: final, as the wallet's ordinary send sets it
/// (key-wallet `TransactionBuilder`), not Core's `SEQUENCE_FINAL - 1` (rc.3
/// `wallet/spend.cpp:1025-1029`; D25).
pub(crate) const SEQUENCE: u32 = 0xffff_ffff;

/// What pays for a ProTx: the inputs, the fee, and the change (D2, D22).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Funding {
    pub inputs: Vec<Utxo>,
    pub fee: u64,
    pub change: Option<u64>,
}

/// Serialized size of a special transaction: version and type, the
/// inputs (148 bytes each, key-wallet's P2PKH budget), the outputs, the
/// lock time and the payload with its length.
pub(crate) fn tx_size(inputs: usize, outputs: &[usize], payload: usize) -> usize {
    4 + varint_size(inputs)
        + inputs * INPUT_SIZE
        + varint_size(outputs.len())
        + outputs.iter().map(|l| output_size(*l)).sum::<usize>()
        + 4
        + varint_size(payload)
        + payload
}

/// Largest-first selection over `candidates` for `outputs` and a payload of
/// `payload` bytes, at the relay minimum (D22). Change pays a script of
/// `change_len` bytes; a remainder at or below its dust threshold goes to
/// the fee, as `send::plan` does.
pub(crate) fn plan_funding(
    candidates: &[Utxo],
    outputs: &[TxOut],
    payload: usize,
    change_len: usize,
) -> Result<Funding, SendFailure> {
    let rate = FeeRate::new(MIN_FEE_PER_KB);
    let target: u64 = outputs.iter().map(|o| o.value).sum();
    let lens: Vec<usize> = outputs.iter().map(|o| o.script_pubkey.len()).collect();
    let mut with_change = lens.clone();
    with_change.push(change_len);
    let mut sorted: Vec<&Utxo> = candidates.iter().collect();
    sorted.sort_by(|a, b| b.value().cmp(&a.value()).then(a.outpoint.cmp(&b.outpoint)));
    let available: u64 = sorted.iter().map(|u| u.value()).sum();
    let mut chosen = Vec::new();
    let mut total = 0u64;
    for coin in sorted {
        if chosen.len() == MAX_INPUTS {
            return Err(SendFailure::TxTooLarge);
        }
        chosen.push(coin.clone());
        total = total.saturating_add(coin.value());
        let n = chosen.len();
        let bare = fee_for(rate, tx_size(n, &lens, payload)).ok_or(SendFailure::TxTooLarge)?;
        if total < target.saturating_add(bare) {
            continue;
        }
        let fee =
            fee_for(rate, tx_size(n, &with_change, payload)).ok_or(SendFailure::TxTooLarge)?;
        let change = total
            .checked_sub(target.saturating_add(fee))
            .filter(|c| *c > dust_threshold(change_len));
        let fee = match change {
            Some(_) => fee,
            None => total - target,
        };
        // Core's `-maxtxfee` (`CreateTransaction`), as the wallet's send:
        // never reached at the relay minimum within `MAX_INPUTS`.
        if fee > MAX_TX_FEE {
            return Err(SendFailure::AbsurdFee { fee });
        }
        return Ok(Funding {
            inputs: chosen,
            fee,
            change,
        });
    }
    let fee = fee_for(rate, tx_size(chosen.len().max(1), &lens, payload)).unwrap_or(0);
    Err(if available < target {
        SendFailure::AmountExceedsBalance { available }
    } else {
        SendFailure::AmountWithFeeExceedsBalance { fee, available }
    })
}

/// Core's `CompareInputBIP69`: the previous txid's bytes in display order,
/// then the index (rc.3 `primitives/transaction.h:411-426`).
fn input_order(a: &OutPoint, b: &OutPoint) -> Ordering {
    display_bytes(&a.txid)
        .cmp(&display_bytes(&b.txid))
        .then(a.vout.cmp(&b.vout))
}

/// Core's `CompareOutputBIP69`: the value, then the script, and a script
/// compares by length before its bytes (`prevector::operator<`, rc.3
/// `prevector.h:518-538`) (D3).
fn output_order(a: &TxOut, b: &TxOut) -> Ordering {
    a.value
        .cmp(&b.value)
        .then(a.script_pubkey.len().cmp(&b.script_pubkey.len()))
        .then(a.script_pubkey.as_bytes().cmp(b.script_pubkey.as_bytes()))
}

/// A txid's bytes in display order.
fn display_bytes(txid: &dashcore::Txid) -> [u8; 32] {
    let mut b = txid.to_byte_array();
    b.reverse();
    b
}

/// The unsigned special transaction: inputs and outputs sorted BIP69 (D3),
/// `nSequence` [`SEQUENCE`], and lock time 0, as the wallet's ordinary send
/// builds them, so a ProTx looks like the wallet's other transactions; Core's
/// wallet sets the tip height instead (`DiscourageFeeSniping`, rc.3
/// `wallet/spend.cpp:722-766,1031`; D25).
pub(crate) fn assemble(
    inputs: &[Utxo],
    mut outputs: Vec<TxOut>,
    payload: TransactionPayload,
) -> Transaction {
    let mut prevouts: Vec<OutPoint> = inputs.iter().map(|u| u.outpoint).collect();
    prevouts.sort_by(input_order);
    outputs.sort_by(output_order);
    Transaction {
        version: 3,
        lock_time: 0,
        input: prevouts
            .into_iter()
            .map(|previous_output| TxIn {
                previous_output,
                script_sig: ScriptBuf::new(),
                sequence: SEQUENCE,
                witness: Default::default(),
            })
            .collect(),
        output: outputs,
        special_transaction_payload: Some(payload),
    }
}

/// The P2PKH inputs of `tx` to sign: `(index, script, path)` from the
/// wallet's coins. An input whose key isn't a BIP44 or BIP32 account
/// address the `MasternodeOwner` signer may use is an internal error:
/// the candidates were filtered by [`signable`].
pub(crate) fn to_sign(
    tx: &Transaction,
    coins: &[(Utxo, DerivationPath)],
) -> Result<Vec<(usize, ScriptBuf, DerivationPath)>, EngineError> {
    tx.input
        .iter()
        .enumerate()
        .map(|(i, input)| {
            coins
                .iter()
                .find(|(u, _)| u.outpoint == input.previous_output)
                .map(|(u, p)| (i, u.txout.script_pubkey.clone(), p.clone()))
                .ok_or_else(|| {
                    EngineError::Internal(format!("no key for input {}", input.previous_output))
                })
        })
        .collect()
}

/// Whether the `MasternodeOwner` signer signs for a coin at `path`: a
/// BIP44 or BIP32 account address (MG-05 §1). DashPay and CoinJoin coins
/// don't fund a ProTx.
pub(crate) fn signable(path: &DerivationPath, network: dashcore::Network) -> bool {
    dw_vault::paths::is_bip44_address(path, network) || dw_vault::paths::is_bip32_address(path)
}

/// What a ProTx's confirm screen and `PreparedTx` show, read from the
/// signed transaction. `mine`: whether a script is the wallet's; the
/// change output is the one paying `change`.
pub(crate) fn summarize(
    tx: &Transaction,
    inputs: &[Utxo],
    fee: u64,
    change: Option<&ScriptBuf>,
    mine: impl Fn(&ScriptBuf) -> bool,
    network: dashcore::Network,
) -> PreparedSummary {
    let size = dashcore::consensus::serialize(tx).len() as u32;
    let outputs: Vec<PreparedOutput> = tx
        .output
        .iter()
        .map(|o| PreparedOutput {
            address: Address::from_script(&o.script_pubkey, network)
                .ok()
                .map(|a| a.to_string()),
            amount: o.value,
            is_change: change == Some(&o.script_pubkey),
            is_mine: mine(&o.script_pubkey),
            label: None,
        })
        .collect();
    let external_sent: u64 = outputs
        .iter()
        .filter(|o| !o.is_mine)
        .map(|o| o.amount)
        .sum();
    PreparedSummary {
        txid: tx.txid().to_string(),
        fee,
        fee_rate_per_kb: MIN_FEE_PER_KB,
        size_bytes: size,
        inputs: tx
            .input
            .iter()
            .map(|i| {
                let coin = inputs.iter().find(|u| u.outpoint == i.previous_output);
                PreparedInput {
                    outpoint: i.previous_output,
                    address: coin.map(|u| u.address.to_string()),
                    amount: coin.map_or(0, Utxo::value),
                }
            })
            .collect(),
        total_sent: outputs
            .iter()
            .filter(|o| !o.is_change)
            .map(|o| o.amount)
            .sum(),
        outputs,
        external_sent,
        total_debit: external_sent + fee,
    }
}

// ---------------------------------------------------------------------------
// A registration against the wallet, the list and its operations
// ---------------------------------------------------------------------------

/// The 0-value `OP_RETURN` output Core funds a transaction without outputs
/// with, removed again when change was added (rc.3
/// `wallet/interfaces.cpp:460-505`, D26).
pub(crate) fn dummy_output() -> TxOut {
    TxOut {
        value: 0,
        script_pubkey: ScriptBuf::from_bytes(vec![0x6a]),
    }
}

/// A stand-in P2PKH for a preview's addresses that are issued only at
/// registration (or, in an update, a `NewAddress` payout): same size, never
/// a key the wallet uses.
pub(crate) fn preview_script() -> ScriptBuf {
    ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array([0xff; 20]))
}

/// What the wallet, the synced list and its operations hold (D5, D13, D23).
pub(crate) struct Context {
    pub listed: Vec<ListedMasternode>,
    /// The synced list's entries, which a ProUpRegTx keeps keys from (D10).
    pub entries: Vec<MasternodeListSummary>,
    /// The list's height (MG-03 D3): an operation confirmed above it isn't
    /// in the list yet (D43).
    pub list_height: Option<u32>,
    pub used: InUse,
    /// The wallet's operations that may still go out or be out, and aren't
    /// in the list yet (D51).
    pub pending: Vec<Claim>,
    /// The collateral scripts of the wallet's `External` registrations by
    /// ProTx hash (wire order): a coin the wallet doesn't hold (D40).
    pub external_collaterals: HashMap<[u8; 32], ScriptBuf>,
}

impl Context {
    /// D51: refuses, as Core's mempool does (`protx-dup`, rc.3
    /// `validation.cpp:1105-1106`), an operator key, platform node id or
    /// service that one of the wallet's pending operations names (rc.3
    /// `txmempool.cpp:1601-1622` for a ProRegTx, `:1691-1692` for a
    /// ProUpRegTx). Core never tells an SPV wallet: the operation would sit
    /// `sent` until abandoned.
    /// The refusal names the operation holding the key, so the user can
    /// end it (discard it if never sent, else abandon its transaction).
    pub(crate) fn check_pending(&self, claim: &Claim) -> Result<(), MasternodesError> {
        let clash = (self.pending.iter())
            .filter(|p| p.id != claim.id)
            .find(|p| p.clashes(claim));
        match clash {
            Some(p) => Err(MasternodesError::ProtxRefused {
                reason: "protx-dup".into(),
                operation: p.id.clone(),
            }),
            None => Ok(()),
        }
    }

    /// [`Self::check_pending`] for the pending row `id` about to go out
    /// (an `External` registration's), against the others.
    pub(crate) fn check_pending_row(&self, id: &str) -> Result<(), MasternodesError> {
        match self.pending.iter().find(|p| p.id.as_deref() == Some(id)) {
            Some(own) => self.check_pending(own),
            None => Ok(()),
        }
    }
}

/// Where a resolved registration's collateral is.
#[derive(Debug, Clone)]
pub(crate) enum Collateral {
    /// Paid by the registration to `address` (`None` in a preview).
    FundNew {
        address: Option<Address>,
        script: ScriptBuf,
    },
    /// A coin of this wallet, checked under the coin fence ([`existing_coin`]).
    Existing { outpoint: OutPoint },
    /// Another wallet's coin at a P2PKH address (D4).
    External {
        outpoint: OutPoint,
        address: Address,
    },
}

/// A request resolved into payload terms: keys picked, addresses issued
/// (or stood in for, in a preview), the Core rules of the placeholder run.
#[derive(Debug, Clone)]
pub(crate) struct Registration {
    pub terms: RegistrationTerms,
    pub display: ProTxTerms,
    pub collateral: Collateral,
}

impl Registration {
    pub fn evo(&self) -> bool {
        self.terms.evo
    }

    /// The placeholder payload with a 65-byte signature where the
    /// collateral key signs, the size Core funds it at.
    pub fn sized_payload(&self) -> usize {
        let mut p = payloads::registration_placeholder(&self.terms);
        if !matches!(self.collateral, Collateral::FundNew { .. }) {
            p.signature = vec![0; 65];
        }
        dw_protx::shared::payload_bytes(&p).len()
    }

    /// The outputs other than change.
    pub fn outputs(&self) -> Vec<TxOut> {
        match &self.collateral {
            Collateral::FundNew { script, .. } => vec![TxOut {
                value: collateral_amount(self.evo()),
                script_pubkey: script.clone(),
            }],
            _ => vec![dummy_output()],
        }
    }
}

/// Voting never shares the owner's key (D52), though Core lets it (rc.3
/// `evo/providertx_service.cpp:521` defaults to it).
pub(crate) fn shared_voting() -> MasternodesError {
    invalid("the voting key is the owner key: give another")
}

pub(crate) fn refused(reason: rules::Reject) -> MasternodesError {
    MasternodesError::ProtxRefused {
        reason: reason.into(),
        operation: None,
    }
}

pub(crate) fn invalid(detail: &str) -> MasternodesError {
    EngineError::InvalidArgument(detail.into()).into()
}

/// A service only a version-3 payload can carry: a host name, an IPv6
/// address, or several entries (D1).
fn needs_v3_service(text: &str) -> bool {
    let t = text.trim();
    t.bytes()
        .any(|b| b.is_ascii_alphabetic() || b == b'[' || b == b',' || b == b' ')
        || t.matches(':').count() > 1
}

/// The service address of a request (D17), with Core's reject reasons.
pub(crate) fn parse_service(text: &str, network: Network) -> Result<SocketAddr, MasternodesError> {
    if text.trim().is_empty() {
        return Err(invalid("a service address is required"));
    }
    if needs_v3_service(text) {
        return Err(MasternodesError::Unsupported {
            feature: "protx_v3".into(),
            field: Some("service".into()),
        });
    }
    service::parse_service(text, network).map_err(|e| match e {
        ServiceError::BadPort(_) => refused("bad-protx-netinfo-port"),
        ServiceError::NotRoutable(_) => refused("bad-protx-netinfo-addr-unroutable"),
        ServiceError::BadAddress(_) => refused("bad-protx-netinfo-addr"),
        _ => invalid("the service is not an IPv4 address with an optional port"),
    })
}

/// The list as Core's duplicate checks compare it; the wallet's own
/// masternodes carry their collateral (the list has none).
fn listed(list: &ListView, owned: &[OwnedMasternode]) -> Vec<ListedMasternode> {
    list.entries
        .iter()
        .map(|e| ListedMasternode {
            pro_tx_hash: e.pro_tx_hash,
            service: e.service_address,
            operator_public_key: e.operator_public_key,
            voting_key_id: e.voting_key_id,
            platform_node_id: e.platform_node_id,
            collateral: owned
                .iter()
                .find(|o| o.record.pro_tx_hash == e.pro_tx_hash)
                .and_then(|o| o.collateral),
        })
        .collect()
}

/// The evonode terms and the operator reward of `req`.
fn check_shape(req: &RegistrationRequest) -> Result<(), MasternodesError> {
    if (req.kind == MasternodeType::Evonode) != req.evonode.is_some() {
        return Err(invalid("evonode terms are given exactly for an evonode"));
    }
    if req.operator_reward > MAX_OPERATOR_REWARD {
        return Err(invalid("the operator reward is at most 10000 basis points"));
    }
    Ok(())
}

/// The checks of `req` that need nothing but its text and the network:
/// run before the list is read, so a malformed request is refused as
/// `invalid_argument` (or the service's own refusal) first, as MG-03's
/// calls refuse their arguments.
pub(crate) fn check_request(
    req: &RegistrationRequest,
    network: Network,
) -> Result<(), MasternodesError> {
    check_shape(req)?;
    parse_service(&req.service, network)?;
    match &req.collateral {
        CollateralChoice::FundNew => {}
        CollateralChoice::ExistingUtxo { outpoint } => {
            parse_outpoint(outpoint)?;
        }
        CollateralChoice::External { outpoint, address } => {
            parse_outpoint(outpoint)?;
            if super::keys::p2pkh_id(address.trim(), network).is_none() {
                return Err(invalid("the collateral address is not a P2PKH address"));
            }
        }
    }
    if let VotingChoice::Address { address } = &req.voting
        && super::keys::p2pkh_id(address, network).is_none()
    {
        return Err(invalid("the voting key is not a P2PKH address"));
    }
    if let OperatorChoice::Given { public_key } = &req.operator {
        super::keys::parse_operator_key(public_key)?;
    }
    if let Some(EvonodeTerms {
        node: PlatformNodeChoice::Given { node_id },
        ..
    }) = &req.evonode
    {
        super::keys::parse_node_id(node_id)?;
    }
    if let PayoutChoice::Address { address } = &req.payout {
        l1_address(address.trim(), network)
            .map_err(|_| invalid("the payout is not a P2PKH or P2SH address"))?;
    }
    Ok(())
}

impl NetworkSession {
    /// The context of a ProTx: the synced list (`ListUnavailable`
    /// otherwise), the wallet's masternodes and operations, and every key
    /// and collateral they use.
    pub(crate) async fn protx_context(
        &self,
        wallet: WalletId,
    ) -> Result<Context, MasternodesError> {
        let network = self.network.core_network();
        let manager = self.manager()?;
        #[cfg(test)]
        let seamed = (self.masternodes.protx_seam()).and_then(|s| {
            let height = *s.list_height.lock().unwrap();
            (s.list.lock().unwrap().clone()).map(|entries| ListView {
                synced: true,
                height: Some(height),
                entries,
            })
        });
        #[cfg(not(test))]
        let seamed = None;
        let list = match seamed {
            Some(list) => list,
            None => read_list(self, &manager).await,
        };
        if !list.synced {
            return Err(MasternodesError::ListUnavailable);
        }
        let hub = Arc::clone(&self.hub);
        let owned = tokio::task::spawn_blocking(move || {
            wallet_masternodes(&manager, &hub.history, &wallet).map(|(o, _)| o)
        })
        .await?
        .unwrap_or_default();
        let w = wallet.to_string();
        let rows = self.appdb_op(move |db| db.masternode_ops(&w)).await?;
        let (history, unspent) = self.op_view(wallet).await?;
        let mut pending = Vec::new();
        let mut used = InUse::default();
        let mut external_collaterals = HashMap::new();
        for o in &owned {
            let r = &o.record;
            used.owner.extend(r.owner_key_hash);
            used.voting.extend(r.voting_key_hash);
            used.operator.extend(r.operator_public_key);
            used.node_ids.extend(r.platform_node_id);
            used.collateral.extend(o.collateral);
        }
        for e in &list.entries {
            used.voting.insert(e.voting_key_id);
            used.operator.insert(e.operator_public_key);
            used.node_ids.extend(e.platform_node_id);
        }
        for row in &rows {
            let Ok(terms) = serde_json::from_str::<ProTxTerms>(&row.terms) else {
                continue;
            };
            let state = state_of(row, &history, &unspent);
            // A block spends a conflicted row's input: it never confirms,
            // so its keys and collateral are free again, as Core frees a
            // ProRegTx's when it leaves the mempool (D54).
            if state == ProTxOperationState::Conflicted {
                continue;
            }
            used.add_terms(&terms, network);
            // Whatever may still go out, or is out and not in the list
            // (D43's window).
            let unlisted = match state {
                ProTxOperationState::Confirmed { height } => list.height.is_none_or(|l| l < height),
                _ => true,
            };
            if unlisted {
                pending.push(Claim::of_row(row, &terms));
            }
            if let (Some(CollateralKind::External), Some(address), Some(hash)) = (
                terms.collateral,
                terms.collateral_address.as_deref(),
                row.pro_tx_hash.as_deref(),
            ) && let (Ok(address), Ok(hash)) =
                (l1_address(address, network), parse_pro_tx_hash(hash))
            {
                external_collaterals.insert(hash, address.script_pubkey());
            }
        }
        // A shared session's keys are the wallet's too (MG-04b D10).
        self.shared_keys_in_use(wallet, &mut used).await?;
        Ok(Context {
            listed: listed(&list, &owned),
            list_height: list.height,
            entries: list.entries,
            used,
            pending,
            external_collaterals,
        })
    }

    /// Resolves `req` (MG-04 §4 steps 1, 2 and 4). `commit`: issue the
    /// fresh payout and collateral addresses; a preview stands in for them.
    pub(crate) async fn resolve_registration(
        &self,
        wallet: WalletId,
        req: &RegistrationRequest,
        ctx: &Context,
        commit: bool,
    ) -> Result<Registration, MasternodesError> {
        let network = self.network.core_network();
        check_shape(req)?;
        let evo = req.kind == MasternodeType::Evonode;
        let service = parse_service(&req.service, network)?;
        let collateral = match &req.collateral {
            CollateralChoice::FundNew => Collateral::FundNew {
                address: None,
                script: preview_script(),
            },
            CollateralChoice::ExistingUtxo { outpoint } => {
                let outpoint = parse_outpoint(outpoint)?;
                if ctx.used.collateral.contains(&outpoint) {
                    return Err(MasternodesError::CollateralUnavailable);
                }
                Collateral::Existing { outpoint }
            }
            CollateralChoice::External { outpoint, address } => {
                let outpoint = parse_outpoint(outpoint)?;
                let address = l1_address(address.trim(), network)
                    .ok()
                    .filter(|a| matches!(a.payload(), Payload::PubkeyHash(_)))
                    .ok_or_else(|| invalid("the collateral address is not a P2PKH address"))?;
                if ctx.used.collateral.contains(&outpoint) {
                    return Err(MasternodesError::CollateralUnavailable);
                }
                let w = self.wallet(&wallet).await?;
                let state = w.state().await;
                if wallet_owns(&state.core_wallet, &address)
                    || read_coins(&state.core_wallet)
                        .coins
                        .iter()
                        .any(|c| c.utxo.outpoint == outpoint)
                {
                    return Err(invalid(
                        "the collateral is this wallet's: register it as an existing coin",
                    ));
                }
                Collateral::External { outpoint, address }
            }
        };
        let owner = self.pick_owner(wallet, req.owner_index, &ctx.used).await?;
        let voting = self.pick_voting(wallet, &req.voting, &ctx.used).await?;
        if voting.key_id == owner.key_id {
            return Err(shared_voting());
        }
        let operator = self.pick_operator(wallet, &req.operator, &ctx.used).await?;
        let platform = match &req.evonode {
            Some(t) => {
                let node = self.pick_platform_node(wallet, &t.node, &ctx.used).await?;
                let ports = default_ports(network);
                Some((
                    node.index,
                    PlatformPorts {
                        node_id: node.node_id,
                        p2p_port: t.p2p_port.unwrap_or(ports.platform_p2p),
                        http_port: t.http_port.unwrap_or(ports.platform_https),
                    },
                ))
            }
            None => None,
        };
        let payout = match &req.payout {
            PayoutChoice::Address { address } => Some(
                l1_address(address.trim(), network)
                    .map_err(|_| invalid("the payout is not a P2PKH or P2SH address"))?,
            ),
            PayoutChoice::NewAddress if commit => {
                Some(self.issued(wallet, "Masternode payout").await?)
            }
            PayoutChoice::NewAddress => None,
        };
        let collateral = match collateral {
            Collateral::FundNew { .. } if commit => {
                let address = self.issued(wallet, "Masternode collateral").await?;
                Collateral::FundNew {
                    script: address.script_pubkey(),
                    address: Some(address),
                }
            }
            other => other,
        };
        let payout_script = payout
            .as_ref()
            .map_or_else(preview_script, Address::script_pubkey);
        let terms = RegistrationTerms {
            evo,
            collateral: match &collateral {
                Collateral::FundNew { script, .. } => CollateralSource::FundNew {
                    script: script.clone(),
                    amount: collateral_amount(evo),
                },
                Collateral::Existing { outpoint } | Collateral::External { outpoint, .. } => {
                    CollateralSource::Existing(*outpoint)
                }
            },
            service,
            owner_key_hash: owner.key_id,
            voting_key_hash: voting.key_id,
            operator_public_key: operator.public_key,
            operator_reward: req.operator_reward,
            payout_script,
            platform: platform.map(|(_, p)| p),
        };
        // Core's rules a placeholder can be held to (D5): the payload's own
        // validity, its network fields and the list's duplicates.
        let placeholder = payloads::registration_placeholder(&terms);
        rules::check_registration_trivial(&placeholder).map_err(refused)?;
        rules::check_network_fields(&placeholder, network).map_err(refused)?;
        let listed_collateral = match &collateral {
            Collateral::Existing { outpoint } | Collateral::External { outpoint, .. } => *outpoint,
            Collateral::FundNew { .. } => OutPoint::null(),
        };
        rules::check_registration_list(&placeholder, listed_collateral, &ctx.listed)
            .map_err(refused)?;
        ctx.check_pending(&Claim {
            operator: Some(operator.public_key),
            node_id: platform.map(|(_, p)| p.node_id),
            service: Some(service.to_string()),
            ..Claim::default()
        })?;
        if let Collateral::External { address, .. } = &collateral {
            rules::check_collateral_key_safe(&address.script_pubkey(), owner.key_id, voting.key_id)
                .map_err(refused)?;
        }
        let display = ProTxTerms {
            masternode_type: req.kind,
            version: placeholder.version,
            service: service.to_string(),
            owner_address: owner.address,
            owner_index: owner.index,
            voting_address: voting.address,
            voting_index: voting.index,
            operator_public_key: hex::encode(operator.public_key),
            operator_index: operator.index,
            operator_reward: req.operator_reward,
            payout_address: payout.map(|a| a.to_string()).unwrap_or_default(),
            collateral: Some(req.collateral.kind()),
            collateral_outpoint: match &collateral {
                Collateral::Existing { outpoint } | Collateral::External { outpoint, .. } => {
                    Some(outpoint_string(outpoint))
                }
                Collateral::FundNew { .. } => None,
            },
            collateral_address: match &collateral {
                Collateral::FundNew { address, .. } => address.as_ref().map(|a| a.to_string()),
                Collateral::External { address, .. } => Some(address.to_string()),
                Collateral::Existing { .. } => None,
            },
            platform: platform.map(|(_, p)| PlatformFacts {
                node_id: hex::encode(p.node_id),
                p2p_port: p.p2p_port,
                http_port: p.http_port,
            }),
            platform_node_index: platform.and_then(|(i, _)| i),
        };
        Ok(Registration {
            terms,
            display,
            collateral,
        })
    }

    /// A fresh receive address, never handed out before (D2's addresses).
    pub(crate) async fn issued(
        &self,
        wallet: WalletId,
        label: &str,
    ) -> Result<Address, MasternodesError> {
        let info = self.issue_address(wallet, Some(label.into())).await?;
        Address::from_str(&info.address)
            .ok()
            .and_then(|a| a.require_network(self.network.core_network()).ok())
            .ok_or_else(|| EngineError::Internal("an issued address did not parse".into()).into())
    }

    /// The wallet's coins a ProTx may spend, with their key paths: the
    /// automatic-selection candidates (`CoinSource::Any`: not locked,
    /// reserved or immature) the `MasternodeOwner` signer signs for,
    /// without `exclude`.
    pub(crate) async fn protx_candidates(
        &self,
        fence: &CoinFence,
        wallet: &PlatformWallet,
        exclude: &HashSet<OutPoint>,
    ) -> Result<Vec<(Utxo, DerivationPath)>, MasternodesError> {
        let network = self.network.core_network();
        let (coins, _) = self
            .source_coins(fence, &CoinSource::Any, &HashSet::new())
            .await?;
        let state = wallet.state().await;
        let picked = coins
            .into_iter()
            .filter(|u| !exclude.contains(&u.outpoint))
            .filter_map(|u| {
                let path = own_address_info(&state.core_wallet, &u.address)?.path;
                signable(&path, network).then_some((u, path))
            })
            .collect();
        Ok(picked)
    }
}

/// An `External` registration rebuilt from the terms a row stored, for a
/// refresh: the same keys, payout, service and collateral.
pub(crate) fn registration_of(
    t: &ProTxTerms,
    network: Network,
) -> Result<Registration, MasternodesError> {
    let stored = || MasternodesError::from(EngineError::Internal("stored ProTx terms".into()));
    let id = |a: &str| super::keys::p2pkh_id(a, network).ok_or_else(stored);
    let outpoint = parse_outpoint(t.collateral_outpoint.as_deref().ok_or_else(stored)?)?;
    let address = l1_address(t.collateral_address.as_deref().ok_or_else(stored)?, network)
        .map_err(|_| stored())?;
    let platform = match &t.platform {
        Some(p) => Some(PlatformPorts {
            node_id: super::keys::parse_node_id(&p.node_id)?,
            p2p_port: p.p2p_port,
            http_port: p.http_port,
        }),
        None => None,
    };
    let terms = RegistrationTerms {
        evo: t.masternode_type == MasternodeType::Evonode,
        collateral: CollateralSource::Existing(outpoint),
        service: t.service.parse().map_err(|_| stored())?,
        owner_key_hash: id(&t.owner_address)?,
        voting_key_hash: id(&t.voting_address)?,
        operator_public_key: super::keys::parse_operator_key(&t.operator_public_key)?,
        operator_reward: t.operator_reward,
        payout_script: l1_address(&t.payout_address, network)
            .map_err(|_| stored())?
            .script_pubkey(),
        platform,
    };
    Ok(Registration {
        terms,
        display: t.clone(),
        collateral: Collateral::External { outpoint, address },
    })
}

/// The `ExistingUtxo` collateral from a coin snapshot taken under the
/// fence: an unspent, mature, unreserved coin of the wallet at a P2PKH
/// address of a BIP44 or BIP32 account, of exactly the type's amount
/// (`CollateralUnavailable` otherwise, §6). A coin the user locked may be
/// a collateral, as in Core; one an unseen proposal fee spends
/// (`fee_held`, D47) may not.
pub(crate) fn existing_coin(
    snapshot: &CoinSnapshot,
    fee_held: &HashSet<OutPoint>,
    info: &ManagedWalletInfo,
    outpoint: OutPoint,
    evo: bool,
    network: Network,
) -> Result<(Utxo, DerivationPath, bool), MasternodesError> {
    let coin = snapshot
        .coins
        .iter()
        .find(|c| c.utxo.outpoint == outpoint)
        .filter(|c| {
            c.send_account
                && !c.reserved
                && !fee_held.contains(&outpoint)
                && c.utxo.is_spendable(snapshot.height)
                && c.utxo.value() == collateral_amount(evo)
                && c.utxo.txout.script_pubkey.is_p2pkh()
        })
        .ok_or(MasternodesError::CollateralUnavailable)?;
    let path = own_address_info(info, &coin.utxo.address)
        .map(|i| i.path)
        .filter(|p| signable(p, network))
        .ok_or(MasternodesError::CollateralUnavailable)?;
    Ok((coin.utxo.clone(), path, coin.user_locked))
}

/// The unsigned ProRegTx of `reg` over `funding`, change to `change`: its
/// payload committed to the inputs (`registration_with_inputs`), and for a
/// fund-new collateral its output named after the sort (§4 step 6).
pub(crate) fn unsigned_registration(
    reg: &Registration,
    funding: &Funding,
    change: Option<&Address>,
) -> Result<Transaction, MasternodesError> {
    let mut outputs = reg.outputs();
    if let (Some(value), Some(address)) = (funding.change, change) {
        outputs.retain(|o| *o != dummy_output());
        outputs.push(TxOut {
            value,
            script_pubkey: address.script_pubkey(),
        });
    }
    let placeholder = payloads::registration_placeholder(&reg.terms);
    let mut tx = assemble(
        &funding.inputs,
        outputs,
        TransactionPayload::ProviderRegistrationPayloadType(placeholder),
    );
    let payload = match &reg.collateral {
        Collateral::FundNew { script, .. } => payloads::finalize_registration(
            &tx,
            &RegistrationSigner::FundNew {
                script: script.clone(),
                amount: collateral_amount(reg.evo()),
            },
        ),
        _ => payloads::registration_with_inputs(&tx)
            .map(TransactionPayload::ProviderRegistrationPayloadType),
    }
    .map_err(|e| EngineError::Internal(format!("payload: {e}")))?;
    tx.special_transaction_payload = Some(payload);
    Ok(tx)
}

/// The registration payload of `tx`.
pub(crate) fn registration_payload(tx: &Transaction) -> Option<&ProviderRegistrationPayload> {
    match &tx.special_transaction_payload {
        Some(TransactionPayload::ProviderRegistrationPayloadType(p)) => Some(p),
        _ => None,
    }
}

/// Core's `MakeSignString` of `tx`'s payload: what the collateral key signs.
pub(crate) fn sign_message(tx: &Transaction, network: Network) -> Result<String, MasternodesError> {
    let p =
        registration_payload(tx).ok_or_else(|| EngineError::Internal("not a ProRegTx".into()))?;
    payloads::registration_sign_message(p, network).map_err(|_| refused("bad-protx-payee-dest"))
}

/// Puts the collateral key's `signature` into `tx`'s payload, runs every
/// Core rule of the finished payload (§4 step 7) and signs the inputs.
/// `existing`: the collateral's script and (a wallet coin) its value.
pub(crate) async fn finish_registration(
    mut tx: Transaction,
    signature: Option<[u8; 65]>,
    existing: Option<(&ScriptBuf, Option<u64>)>,
    coins: &[(Utxo, DerivationPath)],
    listed: &[ListedMasternode],
    signer: &VaultSigner,
    network: Network,
) -> Result<Transaction, MasternodesError> {
    if let (Some(sig), Some(TransactionPayload::ProviderRegistrationPayloadType(p))) =
        (signature, tx.special_transaction_payload.as_mut())
    {
        p.signature = sig.to_vec();
    }
    let p = registration_payload(&tx)
        .ok_or_else(|| EngineError::Internal("not a ProRegTx".into()))?
        .clone();
    rules::check_registration(&tx, &p, existing, listed, network).map_err(refused)?;
    let to_sign = to_sign(&tx, coins)?;
    sign_p2pkh_inputs(signer, &mut tx, &to_sign, SIGHASH_ALL).await?;
    Ok(tx)
}

/// The base64 `signmessage` signature as its 65 bytes.
pub(crate) fn decode_signature(b64: &str) -> Option<[u8; 65]> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .ok()?
        .try_into()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dashcore::Txid;

    fn coin(seed: u8, vout: u32, value: u64) -> Utxo {
        let script = ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array([seed; 20]));
        let address = Address::from_script(&script, dashcore::Network::Regtest).unwrap();
        Utxo::new(
            OutPoint {
                txid: Txid::from_byte_array([seed; 32]),
                vout,
            },
            TxOut {
                value,
                script_pubkey: script,
            },
            address,
            1,
            false,
        )
    }

    fn out(value: u64, script: Vec<u8>) -> TxOut {
        TxOut {
            value,
            script_pubkey: ScriptBuf::from_bytes(script),
        }
    }

    #[test]
    fn outputs_sort_as_core_compares_scripts_length_first() {
        let mut v = [
            out(5, vec![0x02, 0x01]),
            out(5, vec![0xff]),
            out(1, vec![9; 3]),
        ];
        v.sort_by(output_order);
        // Equal values: the 1-byte script sorts first although 0xff > 0x02.
        assert_eq!(v[0].value, 1);
        assert_eq!(v[1].script_pubkey.as_bytes(), &[0xff]);
        assert_eq!(v[2].script_pubkey.as_bytes(), &[0x02, 0x01]);
    }

    #[test]
    fn inputs_sort_by_display_order_txid_then_index() {
        let mut a = [0u8; 32];
        a[0] = 0xff; // last byte in display order
        let mut b = [0u8; 32];
        b[31] = 0x01; // first byte in display order
        let (a, b) = (Txid::from_byte_array(a), Txid::from_byte_array(b));
        let mut v = vec![
            OutPoint { txid: b, vout: 0 },
            OutPoint { txid: a, vout: 2 },
            OutPoint { txid: a, vout: 1 },
        ];
        v.sort_by(input_order);
        assert_eq!(
            v,
            vec![
                OutPoint { txid: a, vout: 1 },
                OutPoint { txid: a, vout: 2 },
                OutPoint { txid: b, vout: 0 },
            ]
        );
    }

    #[test]
    fn plan_takes_largest_first_and_drops_dust_change_into_the_fee() {
        let coins = [coin(1, 0, 600), coin(2, 0, 2_000_000), coin(3, 0, 50_000)];
        let collateral = out(1_000_000, vec![0x76; 25]);
        let f = plan_funding(&coins, std::slice::from_ref(&collateral), 200, 25).unwrap();
        assert_eq!(f.inputs.len(), 1);
        assert_eq!(f.inputs[0].value(), 2_000_000);
        let size = tx_size(1, &[25, 25], 200) as u64;
        assert_eq!(f.fee, size);
        assert_eq!(f.change, Some(2_000_000 - 1_000_000 - size));

        // Exactly enough but for dust: no change, the remainder is fee.
        let bare = tx_size(1, &[25], 200) as u64;
        let tight = [coin(4, 0, 1_000_000 + bare + 100)];
        let f = plan_funding(&tight, std::slice::from_ref(&collateral), 200, 25).unwrap();
        assert_eq!((f.change, f.fee), (None, bare + 100));

        let none = plan_funding(&[], std::slice::from_ref(&collateral), 200, 25).unwrap_err();
        assert_eq!(none, SendFailure::AmountExceedsBalance { available: 0 });
        let short = [coin(5, 0, 1_000_000)];
        assert!(matches!(
            plan_funding(&short, std::slice::from_ref(&collateral), 200, 25).unwrap_err(),
            SendFailure::AmountWithFeeExceedsBalance { .. }
        ));
    }
}
