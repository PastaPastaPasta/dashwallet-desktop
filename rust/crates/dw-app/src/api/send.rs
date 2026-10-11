//! Sending: drafts, prepared transactions, PSBTs, coins and fees, and the
//! `dash:` payment URI helpers.
//!
//! A `TxDraft` lives in `state.drafts` and crosses as a `u64` handle
//! (`new_tx_draft`); every draft command names it as `draft`. `draft_prepare`
//! signs and reserves the inputs, stores the `PreparedTx` with the id of its
//! draft in `state.prepared` and returns its handle; `draft_broadcast` and
//! `draft_abandon` take that handle and refuse one that belongs to another
//! draft. `close_draft` drops a draft and its prepared transactions;
//! closing the network drops them all. A pending prepared transaction that
//! is dropped releases its inputs (the engine's `Drop`); one whose
//! broadcast outcome is unknown keeps them reserved, by design.
//!
//! PSBTs cross as base64 strings (`dw_psbt::parse` also tolerates
//! surrounding whitespace), raw transactions as lower-case hex, txids and
//! outpoint txids as lower-case hex, amounts as duffs.

mod coins;
mod draft;
mod uri;

use std::sync::Arc;

use dw_engine::{EngineError, PreparedTx, PsbtFailure, TxDraft};

use self::coins::{CoinSelectionSummary, FeePolicy, OutPoint, Utxo, UtxoFilter};
use self::draft::{
    BroadcastOutcome, ChangePolicy, CoinSource, FeeMode, PreparedTxHandle, PreparedTxSummary,
    PsbtAnalysis, Recipient, TxEstimate,
};
use self::uri::{AddressClass, QrMatrix, UriParse};
use crate::error::AppError;
use crate::ipc::commands;
use crate::state::{AppState, parse_wallet_id};

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

/// The draft behind a handle.
fn draft_of(state: &AppState, draft: u64) -> Result<Arc<TxDraft>, AppError> {
    state.drafts.get(draft)
}

/// The prepared transaction behind a handle, which must be one of `draft`'s.
fn prepared_of(state: &AppState, draft: u64, prepared: u64) -> Result<Arc<PreparedTx>, AppError> {
    let (owner, tx) = state.prepared.get(prepared)?;
    if owner != draft {
        return Err(AppError::invalid_argument(format!(
            "prepared transaction {prepared} belongs to draft {owner}, not draft {draft}"
        )));
    }
    Ok(tx)
}

/// A PSBT argument: base64 text (`dw_psbt::parse` also reads binary, which a
/// string cannot carry).
fn parse_psbt(psbt: &str) -> Result<dw_psbt::PartiallySignedTransaction, AppError> {
    dw_psbt::parse(psbt.as_bytes())
        .map_err(|e| AppError::from(EngineError::from(PsbtFailure::from(e))))
}

commands! {
    /// A new, empty draft for a wallet (source `any`, fee `recommended` for 6
    /// blocks, change `auto`). Returns its handle.
    #[facade("NetworkSession::new_tx_draft")]
    fn new_tx_draft(state, wallet_id: String) -> u64 {
        let id = parse_wallet_id(&wallet_id)?;
        let draft = state.session().await?.new_tx_draft(id)?;
        state.drafts.insert(draft)
    }

    /// Drops a draft and every prepared transaction made from it. A pending
    /// prepared transaction releases its reserved inputs; one handed to the
    /// network keeps them until the wallet sees them spent. `false` when the
    /// handle was not open.
    #[facade()]
    fn close_draft(state, draft: u64) -> bool {
        let open = state.drafts.remove(draft).is_some();
        state.prepared.remove_where(|(owner, _)| *owner == draft);
        Ok(open)
    }

    /// The spendable amount of `source` for dash-qt's "Use available
    /// balance": enter this minus the other recipients' amounts and set
    /// `subtractFeeFromAmount`. `fee` is validated only.
    #[facade("NetworkSession::max_spendable")]
    fn max_spendable(state, wallet_id: String, source: CoinSource, fee: FeeMode) -> u64 {
        let id = parse_wallet_id(&wallet_id)?;
        let source = source.into_engine()?;
        Ok(state.session().await?.max_spendable(id, source, fee.into()).await?)
    }

    /// The wallet (lower-case hex) a draft pays from.
    #[facade("TxDraft::wallet_id")]
    fn draft_wallet_id(state, draft: u64) -> String {
        Ok(draft_of(&state, draft)?.wallet_id().to_string())
    }

    /// Replaces the recipient list. Validates addresses (network, Platform
    /// rejection), amounts and duplicates; errors carry the recipient index.
    #[facade("TxDraft::set_recipients")]
    fn draft_set_recipients(state, draft: u64, recipients: Vec<Recipient>) -> () {
        let recipients = recipients.into_iter().map(Into::into).collect();
        Ok(draft_of(&state, draft)?.set_recipients(recipients)?)
    }

    /// Sets the coin source. Whether coin-control outpoints are spendable is
    /// checked by `draftEstimate` and `draftPrepare`.
    #[facade("TxDraft::set_source")]
    fn draft_set_source(state, draft: u64, source: CoinSource) -> () {
        let source = source.into_engine()?;
        Ok(draft_of(&state, draft)?.set_source(source)?)
    }

    #[facade("TxDraft::set_fee")]
    fn draft_set_fee(state, draft: u64, fee: FeeMode) -> () {
        Ok(draft_of(&state, draft)?.set_fee(fee.into())?)
    }

    #[facade("TxDraft::set_change")]
    fn draft_set_change(state, draft: u64, change: ChangePolicy) -> () {
        Ok(draft_of(&state, draft)?.set_change(change.into())?)
    }

    /// Coin selection and fee for the draft; nothing is signed or reserved.
    #[facade("TxDraft::estimate")]
    fn draft_estimate(state, draft: u64) -> TxEstimate {
        Ok(draft_of(&state, draft)?.estimate().await?.into())
    }

    /// Selects coins, builds, signs through the vault with a `Spend` grant
    /// (or a lease) that caps what leaves the wallet, and reserves the
    /// inputs. Never broadcasts. Returns the prepared transaction's handle
    /// and its summary.
    #[facade("TxDraft::prepare", "PreparedTx::summary")]
    fn draft_prepare(state, draft: u64, grant_id: String) -> PreparedTxHandle {
        let prepared = draft_of(&state, draft)?.prepare(grant_id).await?;
        let summary = PreparedTxSummary::from(prepared.summary().clone());
        let handle = state.prepared.insert((draft, prepared))?;
        Ok(PreparedTxHandle { handle, summary })
    }

    /// Announces a prepared transaction and waits for the network's
    /// acceptance (up to about a minute on SPV). The handle is dropped once
    /// the network accepted it; after `send.broadcast_unknown` it stays, so
    /// the call can be repeated.
    #[facade("TxDraft::broadcast")]
    fn draft_broadcast(state, draft: u64, prepared: u64) -> BroadcastOutcome {
        let d = draft_of(&state, draft)?;
        let tx = prepared_of(&state, draft, prepared)?;
        let outcome = d.broadcast(tx).await?;
        state.prepared.remove(prepared);
        Ok(outcome.into())
    }

    /// Discards a prepared transaction and releases its reserved inputs;
    /// idempotent, and refused once it was handed to the network. The
    /// prepared handle is dropped, the draft stays open and editable (use
    /// `closeDraft` to drop it).
    #[facade("TxDraft::abandon")]
    fn draft_abandon(state, draft: u64, prepared: u64) -> () {
        let d = draft_of(&state, draft)?;
        let tx = prepared_of(&state, draft, prepared)?;
        d.abandon(tx).await?;
        state.prepared.remove(prepared);
        Ok(())
    }

    /// The summary of a prepared transaction.
    #[facade("PreparedTx::summary")]
    fn prepared_summary(state, prepared: u64) -> PreparedTxSummary {
        let (_, tx) = state.prepared.get(prepared)?;
        Ok(tx.summary().clone().into())
    }

    /// The signed transaction, consensus-encoded as lower-case hex; `null`
    /// once the engine released it.
    #[facade("PreparedTx::raw")]
    fn prepared_raw(state, prepared: u64) -> Option<String> {
        let (_, tx) = state.prepared.get(prepared)?;
        Ok(tx.raw().map(hex::encode))
    }

    /// dash-qt "Create Unsigned" (QT-076/077): the draft as a base64 PSBT
    /// with the wallet's input data. Signs and reserves nothing and needs no
    /// grant, so watch-only wallets can use it.
    #[facade("TxDraft::create_unsigned")]
    fn draft_create_unsigned(state, draft: u64) -> String {
        let psbt = draft_of(&state, draft)?.create_unsigned().await?;
        Ok(dw_psbt::to_base64(&psbt))
    }

    /// The PSBT Operations dialog content for a base64 PSBT seen from a
    /// wallet (`null`: "no wallet is loaded").
    #[facade("NetworkSession::analyze_psbt")]
    fn analyze_psbt(state, wallet_id: Option<String>, psbt: String) -> PsbtAnalysis {
        let id = wallet_id.as_deref().map(parse_wallet_id).transpose()?;
        let psbt = parse_psbt(&psbt)?;
        Ok(state.session().await?.analyze_psbt(id, psbt).await?.into())
    }

    /// "Sign Tx": signs every input the wallet owns and returns the new
    /// base64 PSBT. Needs a `Spend` grant covering the analysis' `total`.
    #[facade("NetworkSession::sign_psbt")]
    fn sign_psbt(state, wallet_id: String, psbt: String, grant_id: String) -> String {
        let id = parse_wallet_id(&wallet_id)?;
        let psbt = parse_psbt(&psbt)?;
        let signed = state.session().await?.sign_psbt(id, psbt, grant_id).await?;
        Ok(dw_psbt::to_base64(&signed))
    }

    /// "Broadcast Tx": finalizes a complete base64 PSBT, checks the fee rate
    /// cap and broadcasts it. Returns the txid.
    #[facade("NetworkSession::broadcast_psbt")]
    fn broadcast_psbt(state, psbt: String) -> String {
        let psbt = parse_psbt(&psbt)?;
        Ok(state.session().await?.broadcast_psbt(psbt).await?.to_string())
    }

    /// The coin control list of a wallet, largest first.
    #[facade("NetworkSession::utxos")]
    fn utxos(state, wallet_id: String, filter: UtxoFilter) -> Vec<Utxo> {
        let id = parse_wallet_id(&wallet_id)?;
        let rows = state.session().await?.utxos(id, filter.into()).await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// "Lock unspent": excludes outpoints from coin selection. Persisted.
    #[facade("NetworkSession::lock_outpoints")]
    fn lock_outpoints(state, wallet_id: String, outpoints: Vec<OutPoint>) -> () {
        let id = parse_wallet_id(&wallet_id)?;
        let outpoints = OutPoint::to_core_all(&outpoints)?;
        Ok(state.session().await?.lock_outpoints(id, outpoints).await?)
    }

    /// Deletes user locks and releases dust locks.
    #[facade("NetworkSession::unlock_outpoints")]
    fn unlock_outpoints(state, wallet_id: String, outpoints: Vec<OutPoint>) -> () {
        let id = parse_wallet_id(&wallet_id)?;
        let outpoints = OutPoint::to_core_all(&outpoints)?;
        Ok(state.session().await?.unlock_outpoints(id, outpoints).await?)
    }

    /// The outpoints currently locked (user or dust), oldest first.
    #[facade("NetworkSession::locked_outpoints")]
    fn locked_outpoints(state, wallet_id: String) -> Vec<OutPoint> {
        let id = parse_wallet_id(&wallet_id)?;
        let locked = state.session().await?.locked_outpoints(id).await?;
        Ok(locked.into_iter().map(Into::into).collect())
    }

    /// The dust attack protection threshold in duffs; `null` = off (QT-075).
    #[facade("NetworkSession::dust_protection")]
    fn dust_protection(state) -> Option<u64> {
        Ok(state.session().await?.dust_protection().await?)
    }

    /// Turns dust protection on (1..=1,000,000 duffs) or off (`null`).
    #[facade("NetworkSession::set_dust_protection")]
    fn set_dust_protection(state, threshold: Option<u64>) -> () {
        Ok(state.session().await?.set_dust_protection(threshold).await?)
    }

    /// The fee rules of this network (constant on SPV).
    #[facade("dw_engine::fee_policy")]
    fn fee_policy(state) -> FeePolicy {
        Ok(dw_engine::fee_policy().into())
    }

    /// dash-qt's coin-control panel values for `outpoints` paying
    /// `payAmounts` at `fee` (QT-072/074). `allChangeToFee` is the CoinJoin
    /// page rule. Signs and reserves nothing.
    #[facade("NetworkSession::coin_selection_summary")]
    fn coin_selection_summary(
        state,
        wallet_id: String,
        outpoints: Vec<OutPoint>,
        pay_amounts: Vec<u64>,
        fee: FeeMode,
        all_change_to_fee: bool,
    ) -> CoinSelectionSummary {
        let id = parse_wallet_id(&wallet_id)?;
        let outpoints = OutPoint::to_core_all(&outpoints)?;
        let session = state.session().await?;
        let summary = session
            .coin_selection_summary(id, outpoints, pay_amounts, fee.into(), all_change_to_fee)
            .await?;
        Ok(summary.into())
    }

    /// Parses a `dash:` URI for a network (a directory name, as
    /// `NetworkInfo.dirName`) as dash-qt's URI handler does (QT-054,
    /// QT-149). A string that is not an acceptable URI comes back as
    /// `rejected` with the reason.
    #[facade()]
    fn parse_payment_uri(state, network: String, text: String) -> UriParse {
        uri::parse_payment_uri(&network, &text)
    }

    /// Writes a `dash:` URI as dash-qt's `formatBitcoinURI` (QT-085). The
    /// address is not validated; the amount, in duffs, is at most 21 million
    /// DASH.
    #[facade()]
    fn build_payment_uri(
        state,
        address: String,
        amount: Option<u64>,
        label: Option<String>,
        message: Option<String>,
    ) -> String {
        uri::build_payment_uri(address, amount, label, message)
    }

    /// What a pasted or scanned destination is on a network (IOS-042).
    #[facade()]
    fn classify_address(state, network: String, text: String) -> AddressClass {
        uri::classify_address(&network, &text)
    }

    /// The QR module matrix for a text of at most 255 characters (QT-084).
    #[facade()]
    fn qr_matrix(state, text: String) -> QrMatrix {
        uri::qr_matrix(&text)
    }
}
