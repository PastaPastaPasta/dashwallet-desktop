//! M2 transaction actions and exports: dash-qt detail fields, abandon and
//! resend, drop unconfirmed, CSV export and notification rows. Owner: R1
//! (engine-tools). Contract: docs/contracts/m2-engine.md §2.2.

use crate::api::common::{
    OutPoint, domain_error_common, ensure_open, not_implemented, parse_txid, parse_wallet_id,
};
use crate::{DisplayUnit, HistoryError, HistoryFilter, HistorySort, NetworkSession, TxType};

/// The dash-qt details-dialog fields `TxDetail` lacks (QT-092 §4.6, QT-091
/// enablement, IOS-031).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TxDetailExtras {
    pub txid: String,
    pub is_coinbase: bool,
    /// Sum of outputs paying this wallet ("Total credit").
    pub total_credit: u64,
    /// Sum of this wallet's inputs ("Total debit"); `None` when an input
    /// value is unknown.
    pub total_debit: Option<u64>,
    /// "Net amount".
    pub net: i64,
    /// Immature coinbase: blocks until mature ("matures in %n more block(s)").
    pub matures_in: Option<u32>,
    /// Whether a peer has the transaction in its mempool. SPV cannot see
    /// mempools: `None` unless the engine saw an announcement since start
    /// (DESIGN-opus §1.14). Never guessed.
    pub in_mempool: Option<bool>,
    pub abandoned: bool,
    /// dash-qt "Abandon transaction" enablement (§4.5 item 8): not abandoned,
    /// unconfirmed, no InstantSend lock, and not known to be in a mempool.
    pub can_abandon: bool,
    /// dash-qt "Resend transaction" enablement (§4.5 item 9): unconfirmed,
    /// not abandoned, not coinbase, no InstantSend lock, sent by this wallet.
    pub can_resend: bool,
    /// Dust-locked outputs of this transaction ("Unlock dust UTXO" on
    /// DustReceive rows, QT-075); release with `unlock_outpoints`.
    pub dust_locked_outputs: Vec<OutPoint>,
    /// When this engine last announced the transaction (send or resend).
    pub last_announced_at: Option<u64>,
}

/// Why `abandon_transaction` / `resend_transaction` refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum TxActionRefusal {
    Confirmed,
    InstantLocked,
    AlreadyAbandoned,
    Coinbase,
    /// A peer announced it after the last send: it may still confirm.
    InMempool,
    /// The wallet did not fund it (resend only).
    NotSentByWallet,
}

/// One row of a transaction notification (QT-031): one per dash-qt record,
/// so a payment to two recipients gives two rows.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TxNotice {
    pub txid: String,
    pub record_index: u32,
    /// `amount > 0` = "Incoming transaction", otherwise "Sent transaction".
    pub amount: i64,
    pub timestamp: Option<u64>,
    pub tx_type: TxType,
    pub address: Option<String>,
    pub label: Option<String>,
    /// A CoinJoin mixing, denomination or collateral transaction; hidden
    /// unless "show CoinJoin popups" is on (QT-033).
    pub coinjoin_internal: bool,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum TxActionError {
    /// Code `tx_action.tx_not_found`.
    #[error("transaction {txid} not found")]
    TxNotFound { txid: String },
    /// Code `tx_action.refused`: the transaction's state forbids the action.
    #[error("refused: {refusal:?}")]
    Refused { refusal: TxActionRefusal },
    /// Code `tx_action.spv_not_running`: resend needs a running SPV client.
    #[error("spv not running")]
    SpvNotRunning,
    /// Code `tx_action.no_peers`: no connected peer to announce to.
    #[error("no peers")]
    NoPeers,
    /// Code `invalid_argument`.
    #[error("invalid argument: {detail}")]
    InvalidArgument { detail: String },
    /// Code `network_not_open`.
    #[error("network not open: {detail}")]
    NetworkNotOpen { detail: String },
    /// Code `wallet_not_found`.
    #[error("wallet not found: {detail}")]
    WalletNotFound { detail: String },
    /// Code `storage`.
    #[error("storage: {detail}")]
    Storage { detail: String },
    /// Code `not_implemented`.
    #[error("not implemented: {call}")]
    NotImplemented { call: String },
    /// Code `internal`.
    #[error("internal: {detail}")]
    Internal { detail: String },
}

domain_error_common!(TxActionError);
crate::api::common::export_error_code!(TxActionError);

impl TxActionError {
    /// Stable code (docs/contracts/m2-engine.md §4).
    fn code_str(&self) -> &'static str {
        match self {
            Self::TxNotFound { .. } => "tx_action.tx_not_found",
            Self::Refused { .. } => "tx_action.refused",
            Self::SpvNotRunning => "tx_action.spv_not_running",
            Self::NoPeers => "tx_action.no_peers",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NetworkNotOpen { .. } => "network_not_open",
            Self::WalletNotFound { .. } => "wallet_not_found",
            Self::Storage { .. } => "storage",
            Self::NotImplemented { .. } => "not_implemented",
            Self::Internal { .. } => "internal",
        }
    }
}

#[uniffi::export]
impl NetworkSession {
    /// dash-qt detail fields and action enablement for one transaction.
    pub async fn tx_detail_extras(
        &self,
        wallet_id: String,
        txid: String,
    ) -> Result<TxDetailExtras, HistoryError> {
        parse_wallet_id(&wallet_id)?;
        parse_txid(&txid)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.tx_detail_extras")
    }

    /// dash-qt "Abandon transaction" (QT-091): marks it abandoned, releases
    /// its inputs for new payments and excludes it from balances (status
    /// `Abandoned`, amount in brackets). SPV cannot prove the transaction is
    /// in no mempool, so it may still confirm; the engine then shows it
    /// confirmed again. Emits `HistoryChanged` and `Balances`.
    pub async fn abandon_transaction(
        &self,
        wallet_id: String,
        txid: String,
    ) -> Result<(), TxActionError> {
        parse_wallet_id(&wallet_id)?;
        parse_txid(&txid)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.abandon_transaction")
    }

    /// dash-qt "Resend transaction" (QT-091): announces the stored
    /// transaction to the connected peers again. Returns once announced;
    /// there is no acceptance verdict (unlike `TxDraft.broadcast`).
    pub async fn resend_transaction(
        &self,
        wallet_id: String,
        txid: String,
    ) -> Result<(), TxActionError> {
        parse_wallet_id(&wallet_id)?;
        parse_txid(&txid)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.resend_transaction")
    }

    /// iOS "Remove unconfirmed" / bulk drop (IOS-034): abandons every
    /// unconfirmed, non-InstantSend-locked transaction of `wallet_id` (all
    /// wallets when `None`) that no peer announced since start, then
    /// schedules a rescan from the oldest dropped transaction's first-seen
    /// height. Returns how many were dropped.
    pub async fn drop_unconfirmed(&self, wallet_id: Option<String>) -> Result<u32, TxActionError> {
        if let Some(id) = &wallet_id {
            parse_wallet_id(id)?;
        }
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.drop_unconfirmed")
    }

    /// dash-qt CSV export of the filtered view (QT-093, §4.7), exact bytes:
    /// header `Confirmed`, `Watch-only` (only for watch-only wallets), `Date`
    /// (`yyyy-MM-ddTHH:mm:ss`, local time of `utc_offset_secs`), `Type`,
    /// `Label`, `Address`, `Amount (<unit name>)`, `ID`; every field quoted,
    /// quotes doubled, `,` separators, `\n` line ends; amounts signed, no
    /// separators, in `unit`. `type_names` are the 19 `TxType` display strings
    /// in `TxType` order (the host's localization); empty = dash-qt English.
    pub async fn export_history_csv(
        &self,
        wallet_id: String,
        filter: HistoryFilter,
        sort: HistorySort,
        unit: DisplayUnit,
        type_names: Vec<String>,
        utc_offset_secs: i32,
    ) -> Result<String, HistoryError> {
        let _ = (filter, sort, unit, type_names, utc_offset_secs);
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.export_history_csv")
    }

    /// Notification rows for transactions named by a `NewTransactions`
    /// event (QT-031). Unknown txids are skipped.
    pub async fn tx_notices(
        &self,
        wallet_id: String,
        txids: Vec<String>,
    ) -> Result<Vec<TxNotice>, HistoryError> {
        parse_wallet_id(&wallet_id)?;
        for txid in &txids {
            parse_txid(txid)?;
        }
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.tx_notices")
    }
}
