use std::fmt;

use key_wallet::Network;

use crate::EngineError;

/// A Dash network the engine can open a session for. Each network has its own
/// data directory, so wallets never cross networks (dash-qt QT-002).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DashNetwork {
    Mainnet,
    Testnet,
    Devnet { name: String },
    Regtest,
}

impl DashNetwork {
    /// The rust-dashcore network this maps to.
    pub fn core_network(&self) -> Network {
        match self {
            DashNetwork::Mainnet => Network::Mainnet,
            DashNetwork::Testnet => Network::Testnet,
            DashNetwork::Devnet { .. } => Network::Devnet,
            DashNetwork::Regtest => Network::Regtest,
        }
    }

    /// Directory name under the data root: `mainnet`, `testnet`,
    /// `devnet-<name>`, `regtest` (DESIGN-opus §1.7).
    pub fn dir_name(&self) -> String {
        match self {
            DashNetwork::Mainnet => "mainnet".to_string(),
            DashNetwork::Testnet => "testnet".to_string(),
            DashNetwork::Devnet { name } => format!("devnet-{name}"),
            DashNetwork::Regtest => "regtest".to_string(),
        }
    }

    pub fn devnet_name(&self) -> Option<&str> {
        match self {
            DashNetwork::Devnet { name } => Some(name),
            _ => None,
        }
    }

    /// Rejects devnet names that are empty or could escape the data root.
    /// Same character rule as the trusted quorum provider's devnet validator.
    pub fn validate(&self) -> Result<(), EngineError> {
        if let DashNetwork::Devnet { name } = self {
            let ok = !name.is_empty()
                && name.len() <= 64
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && !name.starts_with('-')
                && !name.ends_with('-');
            if !ok {
                return Err(EngineError::InvalidArgument(format!(
                    "invalid devnet name {name:?}: use 1-64 ASCII letters, digits or inner hyphens"
                )));
            }
        }
        Ok(())
    }
}

impl fmt::Display for DashNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.dir_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_names_are_distinct_per_network() {
        assert_eq!(DashNetwork::Mainnet.dir_name(), "mainnet");
        assert_eq!(DashNetwork::Testnet.dir_name(), "testnet");
        assert_eq!(DashNetwork::Regtest.dir_name(), "regtest");
        assert_eq!(
            DashNetwork::Devnet {
                name: "paloma".into()
            }
            .dir_name(),
            "devnet-paloma"
        );
    }

    #[test]
    fn devnet_name_cannot_escape_data_root() {
        for bad in ["", "../x", "a/b", "-a", "a-", "a b"] {
            assert!(
                DashNetwork::Devnet { name: bad.into() }.validate().is_err(),
                "{bad:?}"
            );
        }
        assert!(
            DashNetwork::Devnet {
                name: "my-devnet1".into()
            }
            .validate()
            .is_ok()
        );
    }
}
