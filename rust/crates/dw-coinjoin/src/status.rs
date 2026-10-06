//! What a mixing session reports (Dash Core `src/coinjoin/coinjoin.h`
//! `PoolMessage`/`PoolState`, and the `strAutoDenomResult` texts of
//! `src/coinjoin/client.cpp`). The engine reports these codes; the host
//! holds the English texts (Rust never produces user-facing English).

/// Core `PoolState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PoolState {
    Idle,
    Queue,
    AcceptingEntries,
    Signing,
    Error,
}

/// Core `PoolMessage`, the masternode's reply codes, in wire order. The two
/// values Core marks "not used" (`ERR_NON_STANDARD_PUBKEY`, `ERR_NOT_A_MN`)
/// are kept so wire values map one to one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PoolMessage {
    AlreadyHave,
    Denom,
    EntriesFull,
    ExistingTx,
    Fees,
    InvalidCollateral,
    InvalidInput,
    InvalidScript,
    InvalidTx,
    Maximum,
    MnList,
    Mode,
    NonStandardPubkey,
    NotAMasternode,
    QueueFull,
    Recent,
    Session,
    MissingTx,
    Version,
    NoError,
    Success,
    EntriesAdded,
    SizeMismatch,
}

impl PoolMessage {
    /// Every value in wire order (the wire value is the index).
    pub const ALL: [PoolMessage; 23] = [
        Self::AlreadyHave,
        Self::Denom,
        Self::EntriesFull,
        Self::ExistingTx,
        Self::Fees,
        Self::InvalidCollateral,
        Self::InvalidInput,
        Self::InvalidScript,
        Self::InvalidTx,
        Self::Maximum,
        Self::MnList,
        Self::Mode,
        Self::NonStandardPubkey,
        Self::NotAMasternode,
        Self::QueueFull,
        Self::Recent,
        Self::Session,
        Self::MissingTx,
        Self::Version,
        Self::NoError,
        Self::Success,
        Self::EntriesAdded,
        Self::SizeMismatch,
    ];

    /// The message for a wire value; `None` outside Core's range.
    pub fn from_wire(value: i32) -> Option<Self> {
        usize::try_from(value)
            .ok()
            .and_then(|i| Self::ALL.get(i).copied())
    }
}

/// The client's overall status line (`coinjoin status`, research 02 §9.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusCode {
    /// "CoinJoin is idle."
    Idle,
    /// "Can't mix while sync in progress."
    SyncInProgress,
    /// "Wallet is locked."
    WalletLocked,
    /// "Mixing in progress…"
    MixingInProgress,
    /// "No Masternodes detected."
    NoMasternodes,
    /// "Not enough funds to mix."
    NotEnoughFunds,
    /// "Found unconfirmed denominated outputs, will wait till they confirm
    /// to continue."
    UnconfirmedDenominated,
    /// "No compatible Masternode found."
    NoCompatibleMasternode,
    /// "Can't mix: no compatible inputs found!"
    NoCompatibleInputs,
    /// "Trying to connect…"
    TryingToConnect,
    /// "Failed to find mixing queue to join"
    NoQueueToJoin,
    /// "Can't find random Masternode."
    NoRandomMasternode,
    /// "Failed to start a new mixing queue"
    FailedToStartQueue,
    /// "Submitted to masternode, waiting in queue …"
    WaitingInQueue,
    /// "Found enough users, signing…"
    Signing,
    /// "Masternode: <pool message>"
    Masternode(PoolMessage),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_message_wire_values_follow_core() {
        assert_eq!(PoolMessage::from_wire(0), Some(PoolMessage::AlreadyHave));
        assert_eq!(PoolMessage::from_wire(20), Some(PoolMessage::Success));
        assert_eq!(PoolMessage::from_wire(22), Some(PoolMessage::SizeMismatch));
        assert_eq!(PoolMessage::from_wire(23), None);
        assert_eq!(PoolMessage::from_wire(-1), None);
    }
}
