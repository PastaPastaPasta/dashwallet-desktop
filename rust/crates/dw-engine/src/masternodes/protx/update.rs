//! Building a ProUpRegTx (MG-04 §4, rc.3 `evo/providertx_service.cpp:864-953`):
//! the masternode's keys kept from the synced list or set, the payout
//! always named (D10), the owner key found in the keychain or attached
//! (D12), the payload signed by the vault's owner payload signer (D11).
//! Nothing here signs as operator: the operator key is a public key, kept
//! from the list, given, or the keychain's.

use dashcore::address::Payload;
use dashcore::blockdata::transaction::special_transaction::TransactionPayload;
use dashcore::blockdata::transaction::special_transaction::provider_update_registrar::ProviderUpdateRegistrarPayload;
use dashcore::hashes::Hash;
use dashcore::{Address, Network, PubkeyHash, ScriptBuf, Transaction, TxOut, Txid};
use dw_appdb::MasternodeOpRow;
use dw_protx::payloads;
use dw_protx::rules;
use dw_vault::{OwnerKey, VaultSigner};
use key_wallet::{DerivationPath, Utxo};

use super::super::protx_records::{
    KeepOr, OperatorChoice, OwnerKeySource, PayoutChoice, ProTxOperationState, ProTxTerms,
    UpdateRegistrarRequest, VotingChoice,
};
use super::super::reads::owner_facts;
use super::super::records::MasternodeType;
use super::super::{MasternodesError, display_hex, parse_pro_tx_hash};
use super::build::{
    Context, Funding, assemble, dummy_output, invalid, preview_script, refused, shared_voting,
    to_sign,
};
use super::keys::{Claim, p2pkh_id, parse_operator_key};
use super::ops::state_of;
use crate::coinjoin::{SIGHASH_ALL, sign_p2pkh_inputs};
use crate::send::l1_address;
use crate::{EngineError, NetworkSession, WalletId};

/// A request resolved into payload terms (§4 steps 1, 2 and 4).
#[derive(Debug, Clone)]
pub(crate) struct Update {
    pub placeholder: ProviderUpdateRegistrarPayload,
    /// The masternode as it reads after the update (D41).
    pub display: ProTxTerms,
    pub owner: OwnerKey,
    pub owner_source: OwnerKeySource,
    pub owner_key_id: [u8; 20],
    /// The collateral's script, when the wallet knows it (D40).
    pub collateral_script: Option<ScriptBuf>,
    pub operator_changes: bool,
    pub voting_changes: bool,
    pub payout_changes: Option<bool>,
}

impl Update {
    /// The placeholder with a 65-byte owner signature, the size Core funds
    /// it at (`:946`).
    pub fn sized_payload(&self) -> usize {
        let mut p = self.placeholder.clone();
        p.payload_sig = vec![0; 65];
        dw_protx::shared::payload_bytes(&p).len()
    }

    /// No output but change: Core's 0-value `OP_RETURN` until change
    /// replaces it (D26).
    pub fn outputs(&self) -> Vec<TxOut> {
        vec![dummy_output()]
    }
}

/// The checks of `req` that need nothing but its text and the network,
/// before the list is read (D33).
pub(crate) fn check_update_request(
    req: &UpdateRegistrarRequest,
    network: Network,
) -> Result<(), MasternodesError> {
    parse_pro_tx_hash(&req.pro_tx_hash)?;
    if let KeepOr::Set(OperatorChoice::Given { public_key }) = &req.operator {
        parse_operator_key(public_key)?;
    }
    if let KeepOr::Set(VotingChoice::Address { address }) = &req.voting
        && p2pkh_id(address, network).is_none()
    {
        return Err(invalid("the voting key is not a P2PKH address"));
    }
    if let PayoutChoice::Address { address } = &req.payout {
        l1_address(address.trim(), network)
            .map_err(|_| invalid("the payout is not a P2PKH or P2SH address"))?;
    }
    Ok(())
}

/// Whether `new` is another operator key than the listed `old`, under
/// either serialization.
fn operator_differs(new: &[u8; 48], old: &[u8; 48]) -> bool {
    new != old && dw_protx::bls::legacy_of(new).as_ref() != Some(old)
}

fn key_address(key_id: [u8; 20], network: Network) -> Address {
    Address::new(
        network,
        Payload::PubkeyHash(PubkeyHash::from_byte_array(key_id)),
    )
}

impl NetworkSession {
    /// Every loaded wallet's operations on masternode `shown` (display
    /// order), newest first, with their states: an attached owner key
    /// updates a masternode from any loaded wallet (D24, D43).
    async fn ops_on(
        &self,
        shown: &str,
    ) -> Result<Vec<(MasternodeOpRow, ProTxOperationState)>, MasternodesError> {
        let mut out = Vec::new();
        for id in self.manager()?.wallet_ids().await {
            let wallet = WalletId(id);
            let w = wallet.to_string();
            let rows = self.appdb_op(move |db| db.masternode_ops(&w)).await?;
            let rows: Vec<_> = rows
                .into_iter()
                .filter(|r| r.pro_tx_hash.as_deref() == Some(shown))
                .collect();
            if rows.is_empty() {
                continue;
            }
            // A wallet unloaded since `wallet_ids` has no view to judge by.
            let (history, unspent) = match self.op_view(wallet).await {
                Err(MasternodesError::Engine(EngineError::WalletNotFound(_))) => continue,
                view => view?,
            };
            out.extend(rows.into_iter().map(|r| {
                let state = state_of(&r, &history, &unspent);
                (r, state)
            }));
        }
        out.sort_by_key(|a| std::cmp::Reverse(a.0.created_at));
        Ok(out)
    }

    /// The id of an operation on masternode `hash` (wire order) that the
    /// synced list at `list_height` may not show yet: signed, not sent
    /// (resendable) or sent, or confirmed above the list's height (D43,
    /// D49). A conflicted one never will be, and holds nothing.
    async fn unlisted_operation(
        &self,
        hash: [u8; 32],
        list_height: Option<u32>,
    ) -> Result<Option<String>, MasternodesError> {
        Ok(self
            .ops_on(&display_hex(&hash))
            .await?
            .into_iter()
            .find(|(_, state)| match state {
                ProTxOperationState::Signed
                | ProTxOperationState::NotSent
                | ProTxOperationState::Sent => true,
                ProTxOperationState::Confirmed { height } => {
                    list_height.is_none_or(|l| l < *height)
                }
                _ => false,
            })
            .map(|(r, _)| r.id))
    }

    /// What a resend of the update `row` would race (D43, D49): another
    /// update of its masternode saved no earlier (`created_at` is in
    /// seconds: two of one second block each other) that is or may go out,
    /// which the resend would undo (`UpdateSuperseded`); or an earlier one
    /// handed off and not in a block, which it could be mined before
    /// (`UpdatePending`).
    pub(crate) async fn resend_conflict(
        &self,
        row: &MasternodeOpRow,
    ) -> Result<Option<MasternodesError>, MasternodesError> {
        let Some(shown) = row.pro_tx_hash.as_deref() else {
            return Err(
                EngineError::Internal("an update row without its ProTx hash".into()).into(),
            );
        };
        for (r, state) in self.ops_on(shown).await? {
            if r.id == row.id || r.kind != row.kind {
                continue;
            }
            let later = r.created_at >= row.created_at;
            if later
                && matches!(
                    state,
                    ProTxOperationState::Signed
                        | ProTxOperationState::NotSent
                        | ProTxOperationState::Sent
                        | ProTxOperationState::Confirmed { .. }
                )
            {
                return Ok(Some(MasternodesError::UpdateSuperseded { operation: r.id }));
            }
            if !later && state == ProTxOperationState::Sent {
                return Ok(Some(MasternodesError::UpdatePending { operation: r.id }));
            }
        }
        Ok(None)
    }

    /// Resolves `req` against the synced list and the wallet (§4 steps 1,
    /// 2 and 4). `commit`: issue a `NewAddress` payout; a preview stands
    /// in for it.
    pub(crate) async fn resolve_update(
        &self,
        wallet: WalletId,
        req: &UpdateRegistrarRequest,
        ctx: &Context,
        commit: bool,
    ) -> Result<Update, MasternodesError> {
        let network = self.network.core_network();
        let hash = parse_pro_tx_hash(&req.pro_tx_hash)?;
        let entry = (ctx.entries.iter())
            .find(|e| e.pro_tx_hash == hash)
            .ok_or(MasternodesError::NotFound)?;
        let facts = owner_facts(self, wallet, hash).await?;
        // Core refuses a shared masternode (`:876-879`); the wallet can't
        // read a version-3 one's payouts (MG-03 D25).
        if facts.shared || entry.has_extended_net_info {
            return Err(MasternodesError::Unsupported {
                feature: "shared_masternodes".into(),
                field: None,
            });
        }
        let owner_key_id = facts.owner_key_id.ok_or(MasternodesError::NoOwnerKey)?;
        let (owner, owner_source, owner_index) =
            match self.keychain_owner(wallet, owner_key_id).await? {
                Some(index) => (
                    OwnerKey::Keychain { index },
                    OwnerKeySource::Keychain { index },
                    Some(index),
                ),
                None if facts.attached_owner => (
                    OwnerKey::Attached { pro_tx_hash: hash },
                    OwnerKeySource::Attached,
                    None,
                ),
                None => return Err(MasternodesError::NoOwnerKey),
            };
        // Refused while an operation on this masternode may not be in the
        // list yet: a kept key is the list's (D43), and two updates out at
        // once may be mined in either order (D49).
        if let Some(operation) = self.unlisted_operation(hash, ctx.list_height).await? {
            return Err(MasternodesError::UpdatePending { operation });
        }

        let listed_voting = key_address(entry.voting_key_id, network);
        let (voting_id, voting_address, voting_index) = match &req.voting {
            KeepOr::Keep => (entry.voting_key_id, listed_voting.to_string(), None),
            KeepOr::Set(choice) => {
                let p = self.pick_voting(wallet, choice, &ctx.used).await?;
                if p.key_id == owner_key_id {
                    return Err(shared_voting());
                }
                (p.key_id, p.address, p.index)
            }
        };
        let listed_operator = dw_protx::bls::basic_form(&entry.operator_public_key)
            .ok_or_else(|| refused("bad-protx-key-null"))?;
        let (operator, operator_index) = match &req.operator {
            KeepOr::Keep => (listed_operator, None),
            KeepOr::Set(choice) => {
                let p = self.pick_operator(wallet, choice, &ctx.used).await?;
                ctx.check_pending(&Claim {
                    operator: Some(p.public_key),
                    ..Claim::default()
                })?;
                (p.public_key, p.index)
            }
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
        let payout_script = payout
            .as_ref()
            .map_or_else(preview_script, Address::script_pubkey);
        let placeholder = payloads::update_registrar_placeholder(
            Txid::from_byte_array(hash),
            operator,
            voting_id,
            payout_script.clone(),
        );
        let collateral_script = facts
            .collateral_script
            .clone()
            .or_else(|| ctx.external_collaterals.get(&hash).cloned());
        rules::check_update_registrar_terms(
            &placeholder,
            owner_key_id,
            collateral_script.as_ref(),
            &ctx.listed,
        )
        .map_err(refused)?;

        let display = ProTxTerms {
            masternode_type: if entry.is_evonode {
                MasternodeType::Evonode
            } else {
                MasternodeType::Regular
            },
            version: placeholder.version,
            service: entry
                .service_address
                .map(|s| s.to_string())
                .unwrap_or_default(),
            owner_address: key_address(owner_key_id, network).to_string(),
            owner_index,
            voting_address,
            voting_index,
            operator_public_key: hex::encode(operator),
            operator_index,
            operator_reward: facts.operator_reward.unwrap_or_default(),
            payout_address: payout.map(|a| a.to_string()).unwrap_or_default(),
            collateral: None,
            collateral_outpoint: None,
            collateral_address: None,
            platform: None,
            platform_node_index: None,
        };
        Ok(Update {
            operator_changes: operator_differs(&operator, &entry.operator_public_key),
            voting_changes: voting_id != entry.voting_key_id,
            // A fresh address (or a preview's stand-in) is never the
            // current payout.
            payout_changes: facts.payout_script.map(|current| current != payout_script),
            placeholder,
            display,
            owner,
            owner_source,
            owner_key_id,
            collateral_script,
        })
    }
}

/// The unsigned ProUpRegTx of `update` over `funding`, change to `change`,
/// its payload committed to the inputs and not yet signed.
pub(crate) fn unsigned_update(
    update: &Update,
    funding: &Funding,
    change: Option<&Address>,
) -> Result<Transaction, MasternodesError> {
    let outputs = match (funding.change, change) {
        (Some(value), Some(address)) => vec![TxOut {
            value,
            script_pubkey: address.script_pubkey(),
        }],
        _ => update.outputs(),
    };
    let mut tx = assemble(
        &funding.inputs,
        outputs,
        TransactionPayload::ProviderUpdateRegistrarPayloadType(update.placeholder.clone()),
    );
    let payload = payloads::update_registrar_with_inputs(&tx)
        .map_err(|e| EngineError::Internal(format!("payload: {e}")))?;
    tx.special_transaction_payload = Some(TransactionPayload::ProviderUpdateRegistrarPayloadType(
        payload,
    ));
    Ok(tx)
}

/// The update payload of `tx`.
#[cfg(test)]
pub(crate) fn update_payload(tx: &Transaction) -> Option<&ProviderUpdateRegistrarPayload> {
    match &tx.special_transaction_payload {
        Some(TransactionPayload::ProviderUpdateRegistrarPayloadType(p)) => Some(p),
        _ => None,
    }
}

/// Signs the payload with the owner key (§4 step 6, D11), runs every Core
/// rule of the finished payload and signs the inputs (step 7).
pub(crate) async fn finish_update(
    mut tx: Transaction,
    update: &Update,
    coins: &[(Utxo, DerivationPath)],
    ctx: &Context,
    signer: &VaultSigner,
) -> Result<Transaction, MasternodesError> {
    let Some(TransactionPayload::ProviderUpdateRegistrarPayloadType(p)) =
        tx.special_transaction_payload.as_mut()
    else {
        return Err(EngineError::Internal("not a ProUpRegTx".into()).into());
    };
    let sig = signer
        .sign_update_registrar(&update.owner, p)
        .map_err(EngineError::from)?;
    p.payload_sig = sig.to_vec();
    let p = p.clone();
    rules::check_update_registrar(
        &tx,
        &p,
        update.owner_key_id,
        update.collateral_script.as_ref(),
        &ctx.listed,
    )
    .map_err(refused)?;
    let to_sign = to_sign(&tx, coins)?;
    sign_p2pkh_inputs(signer, &mut tx, &to_sign, SIGHASH_ALL).await?;
    Ok(tx)
}
