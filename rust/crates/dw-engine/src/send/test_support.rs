//! Send helpers shared by the engine's tests.

use super::{Recipient, SendFailure};
use crate::EngineError;

/// Someone else's regtest address (dashd `signmessagewithprivkey` vector).
pub(crate) const FOREIGN: &str = "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n";

pub(crate) fn pay(address: &str, amount: u64) -> Recipient {
    Recipient {
        address: address.into(),
        amount,
        subtract_fee_from_amount: false,
        label: None,
        message: None,
    }
}

pub(crate) fn send_failure<T: std::fmt::Debug>(r: Result<T, EngineError>) -> SendFailure {
    match r {
        Err(EngineError::Send(f)) => f,
        other => panic!("expected a send failure, got {other:?}"),
    }
}
