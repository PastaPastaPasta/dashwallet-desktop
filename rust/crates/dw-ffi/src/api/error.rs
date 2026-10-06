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
            E::Storage(_) => Self::Storage { detail },
            E::WalletNotFound(_) => Self::WalletNotFound { detail },
            E::InvalidMnemonic(_) => Self::InvalidMnemonic { detail },
            E::WalletAlreadyExists(_) => Self::WalletAlreadyExists { detail },
            E::Wallet(_) => Self::Wallet { detail },
            E::Sdk(_) => Self::Sdk { detail },
            E::Spv(_) => Self::Spv { detail },
            E::Io(_) => Self::Io { detail },
            E::NotImplemented(_) => Self::NotImplemented { detail },
            E::SpvNotRunning => Self::Spv { detail },
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The §4 row of `EngineError` lists exactly the codes its variants have.
    #[test]
    fn engine_error_codes_match_the_contract() {
        let contract = include_str!("../../../../../docs/contracts/m1-engine.md");
        let row = contract
            .lines()
            .find(|l| l.starts_with("| `EngineError` (M0 calls) |"))
            .expect("EngineError row in §4");
        let mut listed: Vec<&str> = row.split('`').skip(3).step_by(2).collect();
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
