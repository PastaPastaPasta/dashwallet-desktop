/// Error crossing the FFI. Swift maps the case to localized copy; `detail` is
/// diagnostic text for logs only.
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
            E::Wallet(_) => Self::Wallet { detail },
            E::Sdk(_) => Self::Sdk { detail },
            E::Spv(_) => Self::Spv { detail },
            E::Io(_) => Self::Io { detail },
            E::NotImplemented(_) => Self::NotImplemented { detail },
            E::Internal(_) => Self::Internal { detail },
        }
    }
}
