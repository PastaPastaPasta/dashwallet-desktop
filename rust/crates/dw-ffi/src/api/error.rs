/// Engine-level error of the M0 calls (engine, session, SPV start/stop,
/// `balances`). M1 domain calls use their own
/// error enums (`VaultError`, `WalletError`, …). Swift maps the case to
/// localized copy; `detail` is diagnostic text for logs only.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum EngineError {
    #[error("invalid configuration: {detail}")]
    InvalidConfig { detail: String },
    #[error("invalid argument: {detail}")]
    InvalidArgument { detail: String },
    #[error("network not open: {detail}")]
    NetworkNotOpen { detail: String },
    #[error("storage in use: {detail}")]
    StorageInUse { detail: String },
    #[error("storage: {detail}")]
    Storage { detail: String },
    #[error("wallet not found: {detail}")]
    WalletNotFound { detail: String },
    #[error("invalid mnemonic: {detail}")]
    InvalidMnemonic { detail: String },
    #[error("wallet already exists: {detail}")]
    WalletAlreadyExists { detail: String },
    #[error("wallet: {detail}")]
    Wallet { detail: String },
    #[error("sdk: {detail}")]
    Sdk { detail: String },
    #[error("spv: {detail}")]
    Spv { detail: String },
    #[error("io: {detail}")]
    Io { detail: String },
    #[error("not implemented: {detail}")]
    NotImplemented { detail: String },
    #[error("internal: {detail}")]
    Internal { detail: String },
}

crate::api::common::export_error_code!(EngineError);

impl EngineError {
    /// Stable code (docs/contracts/m1-engine.md "Error codes").
    fn code_str(&self) -> &'static str {
        match self {
            Self::InvalidConfig { .. } => "invalid_config",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NetworkNotOpen { .. } => "network_not_open",
            Self::StorageInUse { .. } => "storage_in_use",
            Self::Storage { .. } => "storage",
            Self::WalletNotFound { .. } => "wallet_not_found",
            Self::InvalidMnemonic { .. } => "invalid_mnemonic",
            Self::WalletAlreadyExists { .. } => "wallet_already_exists",
            Self::Wallet { .. } => "wallet",
            Self::Sdk { .. } => "sdk",
            Self::Spv { .. } => "spv",
            Self::Io { .. } => "io",
            Self::NotImplemented { .. } => "not_implemented",
            Self::Internal { .. } => "internal",
        }
    }
}

impl From<dw_engine::EngineError> for EngineError {
    fn from(e: dw_engine::EngineError) -> Self {
        use dw_engine::EngineError as E;
        let detail = e.to_string();
        match e {
            E::InvalidConfig(_) => Self::InvalidConfig { detail },
            E::InvalidArgument(_) => Self::InvalidArgument { detail },
            E::NetworkNotOpen(_) => Self::NetworkNotOpen { detail },
            E::StorageInUse(_) => Self::StorageInUse { detail },
            // The frozen Swift shells have no code for it: `storage`, with
            // the newer-build text as its detail (DP3-04b D35).
            E::Storage(_) | E::StorageTooNew(_) => Self::Storage { detail },
            E::WalletNotFound(_) => Self::WalletNotFound { detail },
            E::InvalidMnemonic(_) => Self::InvalidMnemonic { detail },
            E::WalletAlreadyExists(_) => Self::WalletAlreadyExists { detail },
            E::Wallet(_) => Self::Wallet { detail },
            // Platform calls carry their own domain error (E0-08); M0 calls
            // never produce it.
            E::Sdk(_) | E::InsufficientCredits { .. } => Self::Sdk { detail },
            E::Spv(_) => Self::Spv { detail },
            E::Io(_) => Self::Io { detail },
            E::NotImplemented(_) => Self::NotImplemented { detail },
            E::SpvNotRunning
            | E::SpvRunning
            | E::RescanInProgress
            | E::RescanInterrupted
            | E::NoPeers => Self::Spv { detail },
            E::InvalidXpub(_) => Self::InvalidArgument { detail },
            E::TxActionRefused(_) => Self::Wallet { detail },
            E::HeightOutOfRange(_) | E::InvalidQuery(_) | E::StaleCursor => {
                Self::InvalidArgument { detail }
            }
            E::TxNotFound(_) | E::GapLimit | E::RequestNotFound(_) | E::NameRejected(_) => {
                Self::Wallet { detail }
            }
            E::InvalidAddress(_) | E::AddressNoKey(_) => Self::InvalidArgument { detail },
            E::AddressNotMine(_) | E::Vault(_) | E::Signer(_) => Self::Wallet { detail },
            E::Internal(_) => Self::Internal { detail },
            E::Send(_) | E::Labels(_) => Self::Wallet { detail },
            E::OutpointNotFound(_) => Self::InvalidArgument { detail },
            // Their domains (CompatError, BackupError, PsbtError, the M3
            // CoinJoinError, MasternodeError and DP3-01's contact payments)
            // carry the codes; M0 calls never produce them.
            E::Compat(_)
            | E::Backup(_)
            | E::Psbt(_)
            | E::CoinJoin(_)
            | E::Masternode(_)
            | E::Contact(_) => Self::Wallet { detail },
            // DP1-02's Repair facade carries the refusal's own code; no M0
            // call produces it.
            E::StuckFunding(
                dw_engine::platform::lease::StuckFundingRefusal::JournalUnavailable,
            ) => Self::Storage { detail },
            E::StuckFunding(_) => Self::InvalidArgument { detail },
            // The frozen Swift shells have no shielded funds, so no wallet
            // of theirs holds a shielded spend (0.1 E1, DEC-1046).
            E::ShieldedSpendUnresolved => Self::Wallet { detail },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The codes of the §4 `EngineError` row, each with whether its note
    /// marks it "not in the frozen dw-ffi" (an engine code dw-ffi, frozen
    /// in 0.1, reports under another, DEC-1046). The row's second cell is
    /// a comma-separated list of `` `code` `` with an optional note in
    /// parentheses, whose own backticked words are not codes.
    fn contract_engine_codes() -> Vec<(String, bool)> {
        let contract = include_str!("../../../../../docs/contracts/m1-engine.md");
        let row = contract
            .lines()
            .find(|l| l.starts_with("| `EngineError` (M0 calls) |"))
            .expect("EngineError row in §4");
        let cell = row.split(" | ").nth(1).expect("the codes cell");
        let mut items = Vec::new();
        let (mut depth, mut start) = (0usize, 0usize);
        for (i, c) in cell.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => {
                    items.push(&cell[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        items.push(&cell[start..]);
        (items.into_iter())
            .map(|item| {
                let code = item.split('`').nth(1).expect("a backticked code");
                (code.to_owned(), item.contains("not in the frozen dw-ffi"))
            })
            .collect()
    }

    /// The §4 row of `EngineError` lists exactly the codes its variants
    /// have, plus the engine codes it marks as not in the frozen dw-ffi
    /// (`shielded_spend_unresolved`, reported as `wallet`).
    #[test]
    fn engine_error_codes_match_the_contract() {
        let contract = contract_engine_codes();
        let engine_only: Vec<&str> = (contract.iter())
            .filter(|(_, engine_only)| *engine_only)
            .map(|(code, _)| code.as_str())
            .collect();
        assert_eq!(engine_only, ["shielded_spend_unresolved"]);
        // Reported as the row says.
        let mapped = EngineError::from(dw_engine::EngineError::ShieldedSpendUnresolved);
        assert_eq!(mapped.code(), "wallet");
        let mut listed: Vec<&str> = (contract.iter())
            .filter(|(_, engine_only)| !engine_only)
            .map(|(code, _)| code.as_str())
            .collect();
        listed.sort_unstable();
        let detail = String::new;
        let mut codes: Vec<String> = [
            EngineError::InvalidConfig { detail: detail() },
            EngineError::InvalidArgument { detail: detail() },
            EngineError::NetworkNotOpen { detail: detail() },
            EngineError::StorageInUse { detail: detail() },
            EngineError::Storage { detail: detail() },
            EngineError::WalletNotFound { detail: detail() },
            EngineError::InvalidMnemonic { detail: detail() },
            EngineError::WalletAlreadyExists { detail: detail() },
            EngineError::Wallet { detail: detail() },
            EngineError::Sdk { detail: detail() },
            EngineError::Spv { detail: detail() },
            EngineError::Io { detail: detail() },
            EngineError::NotImplemented { detail: detail() },
            EngineError::Internal { detail: detail() },
        ]
        .iter()
        .map(EngineError::code)
        .collect();
        codes.sort_unstable();
        assert_eq!(codes, listed);
    }
}
