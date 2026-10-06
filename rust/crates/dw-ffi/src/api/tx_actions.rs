//! M2 transaction actions and exports: dash-qt detail fields, abandon and
//! resend, drop unconfirmed, CSV export and notification rows. Owner: R1
//! (engine-tools). Contract: docs/contracts/m2-engine.md §2.2.

use crate::api::common::{OutPoint, domain_error_common, parse_txid, parse_wallet_id};
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

domain_error_common!(@not_implemented TxActionError);

impl From<dw_engine::EngineError> for TxActionError {
    fn from(e: dw_engine::EngineError) -> Self {
        use dw_engine::EngineError as E;
        let detail = e.to_string();
        match e {
            E::TxNotFound(txid) => Self::TxNotFound { txid },
            E::TxActionRefused(r) => Self::Refused { refusal: r.into() },
            E::SpvNotRunning => Self::SpvNotRunning,
            E::NoPeers => Self::NoPeers,
            E::InvalidConfig(_) | E::InvalidArgument(_) => Self::InvalidArgument { detail },
            E::NetworkNotOpen(_) => Self::NetworkNotOpen { detail },
            E::WalletNotFound(_) => Self::WalletNotFound { detail },
            E::StorageInUse(_) | E::Storage(_) | E::Io(_) => Self::Storage { detail },
            E::NotImplemented(call) => Self::NotImplemented { call },
            _ => Self::Internal { detail },
        }
    }
}

impl From<dw_engine::TxActionRefusal> for TxActionRefusal {
    fn from(r: dw_engine::TxActionRefusal) -> Self {
        use dw_engine::TxActionRefusal as R;
        match r {
            R::Confirmed => Self::Confirmed,
            R::InstantLocked => Self::InstantLocked,
            R::AlreadyAbandoned => Self::AlreadyAbandoned,
            R::Coinbase => Self::Coinbase,
            R::InMempool => Self::InMempool,
            R::NotSentByWallet => Self::NotSentByWallet,
        }
    }
}
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
        let id = parse_wallet_id(&wallet_id)?;
        let txid = parse_txid(&txid)?;
        let x = self.inner.tx_detail_extras(id, txid).await?;
        Ok(TxDetailExtras {
            txid: x.txid,
            is_coinbase: x.is_coinbase,
            total_credit: x.total_credit,
            total_debit: x.total_debit,
            net: x.net,
            matures_in: x.matures_in,
            in_mempool: x.in_mempool,
            abandoned: x.abandoned,
            can_abandon: x.can_abandon,
            can_resend: x.can_resend,
            dust_locked_outputs: x.dust_locked_outputs.into_iter().map(Into::into).collect(),
            last_announced_at: x.last_announced_at,
        })
    }

    /// dash-qt "Abandon transaction" (QT-091): marks it abandoned, releases
    /// its inputs for new payments and excludes it from balances (status
    /// `Abandoned`, amount in brackets). Its recorded descendants are
    /// abandoned with it. SPV cannot prove the transaction is in no mempool,
    /// so it may still confirm; the engine then shows it confirmed again.
    /// The spent coins return through a rescan of their funding blocks.
    /// Emits `HistoryChanged` and `Balances`.
    pub async fn abandon_transaction(
        &self,
        wallet_id: String,
        txid: String,
    ) -> Result<(), TxActionError> {
        let id = parse_wallet_id(&wallet_id)?;
        let txid = parse_txid(&txid)?;
        Ok(self.inner.abandon_transaction(id, txid).await?)
    }

    /// dash-qt "Resend transaction" (QT-091): hands the stored transaction
    /// to the SPV client again. Returns once handed over; there is no
    /// acceptance verdict (unlike `TxDraft.broadcast`).
    pub async fn resend_transaction(
        &self,
        wallet_id: String,
        txid: String,
    ) -> Result<(), TxActionError> {
        let id = parse_wallet_id(&wallet_id)?;
        let txid = parse_txid(&txid)?;
        Ok(self.inner.resend_transaction(id, txid).await?)
    }

    /// iOS "Remove unconfirmed" / bulk drop (IOS-034): abandons every
    /// unconfirmed, non-InstantSend-locked, not yet abandoned transaction of
    /// `wallet_id` (all wallets when `None`), then rescans from the lowest
    /// funding height of the coins they spent. Needs a running SPV client.
    /// Returns how many transactions were dropped.
    pub async fn drop_unconfirmed(&self, wallet_id: Option<String>) -> Result<u32, TxActionError> {
        let id = wallet_id.as_deref().map(parse_wallet_id).transpose()?;
        Ok(self.inner.drop_unconfirmed(id).await?)
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
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self
            .inner
            .export_history_csv(
                id,
                filter.into(),
                sort.into(),
                unit.into(),
                type_names,
                utc_offset_secs,
            )
            .await?)
    }

    /// Notification rows for transactions named by a `NewTransactions`
    /// event (QT-031). Unknown txids are skipped.
    pub async fn tx_notices(
        &self,
        wallet_id: String,
        txids: Vec<String>,
    ) -> Result<Vec<TxNotice>, HistoryError> {
        let id = parse_wallet_id(&wallet_id)?;
        let txids = txids
            .iter()
            .map(|t| parse_txid(t))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(self
            .inner
            .tx_notices(id, txids)
            .await?
            .into_iter()
            .map(|n| TxNotice {
                txid: n.txid,
                record_index: n.record_index,
                amount: n.amount,
                timestamp: n.timestamp,
                tx_type: n.tx_type.into(),
                address: n.address,
                label: n.label,
                coinjoin_internal: n.coinjoin_internal,
            })
            .collect())
    }
}
