//! The network the user chose to open (release 0.1, m4 §8): mainnet or
//! testnet, kept in `<data root>/network.json` next to the app's display
//! settings, so onboarding and Settings choose it and the next start opens
//! it. One file per data root; nothing secret is kept. A missing or
//! unreadable file reads as no choice (the app then opens mainnet); a
//! write replaces the file atomically. The desktop app and `dwcli network`
//! share it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{DashNetwork, EngineError};

/// The file's name in the data root.
pub const NETWORK_CHOICE_FILE: &str = "network.json";

/// The networks a user chooses between; the developer networks (regtest,
/// devnets) are launch flags only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkChoice {
    #[default]
    Mainnet,
    Testnet,
}

impl NetworkChoice {
    pub fn network(self) -> DashNetwork {
        match self {
            NetworkChoice::Mainnet => DashNetwork::Mainnet,
            NetworkChoice::Testnet => DashNetwork::Testnet,
        }
    }

    /// The choice that opens `network`; `None` for a developer network.
    pub fn of(network: &DashNetwork) -> Option<Self> {
        match network {
            DashNetwork::Mainnet => Some(NetworkChoice::Mainnet),
            DashNetwork::Testnet => Some(NetworkChoice::Testnet),
            DashNetwork::Devnet { .. } | DashNetwork::Regtest => None,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct File {
    network: NetworkChoice,
}

fn path(root: &Path) -> PathBuf {
    root.join(NETWORK_CHOICE_FILE)
}

/// The saved choice in `root`; `None` when there is no readable one.
pub fn read_network_choice(root: &Path) -> Option<NetworkChoice> {
    let text = std::fs::read_to_string(path(root)).ok()?;
    match serde_json::from_str::<File>(&text) {
        Ok(file) => Some(file.network),
        Err(e) => {
            tracing::warn!(error = %e, "{NETWORK_CHOICE_FILE} is unreadable; no network chosen");
            None
        }
    }
}

/// Saves `choice` in `root`, replacing the file atomically.
pub fn write_network_choice(root: &Path, choice: NetworkChoice) -> Result<(), EngineError> {
    let json = serde_json::to_string_pretty(&File { network: choice })
        .map_err(|e| EngineError::Internal(e.to_string()))?;
    let tmp = root.join(format!("{NETWORK_CHOICE_FILE}.tmp"));
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path(root))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_or_damaged_file_is_no_choice() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_network_choice(dir.path()), None);
        std::fs::write(path(dir.path()), "{\"network\":\"regtest\"}").unwrap();
        assert_eq!(read_network_choice(dir.path()), None);
    }

    #[test]
    fn a_choice_reads_back_and_replaces_the_last() {
        let dir = tempfile::tempdir().unwrap();
        write_network_choice(dir.path(), NetworkChoice::Testnet).unwrap();
        assert_eq!(
            read_network_choice(dir.path()),
            Some(NetworkChoice::Testnet)
        );
        write_network_choice(dir.path(), NetworkChoice::Mainnet).unwrap();
        assert_eq!(
            read_network_choice(dir.path()),
            Some(NetworkChoice::Mainnet)
        );
        assert!(!dir.path().join("network.json.tmp").exists());
    }

    #[test]
    fn only_mainnet_and_testnet_are_choices() {
        assert_eq!(NetworkChoice::default().network(), DashNetwork::Mainnet);
        assert_eq!(
            NetworkChoice::of(&DashNetwork::Testnet),
            Some(NetworkChoice::Testnet)
        );
        assert_eq!(NetworkChoice::of(&DashNetwork::Regtest), None);
    }
}
