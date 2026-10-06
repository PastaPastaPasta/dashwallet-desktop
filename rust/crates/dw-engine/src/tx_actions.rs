//! Transaction actions and exports (QT-031…033, QT-075, QT-091…093,
//! IOS-031, IOS-034; docs/contracts/m2-engine.md §2.2).

/// Why `abandon_transaction` / `resend_transaction` refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TxActionRefusal {
    Confirmed,
    InstantLocked,
    AlreadyAbandoned,
    Coinbase,
    InMempool,
    NotSentByWallet,
}
