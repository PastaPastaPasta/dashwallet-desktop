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

/// Saves `choice` in `root`, replacing the file atomically and durably: a
/// temporary file of this write's own (the app and `dwcli` may write at
/// once), synced, renamed over the file, and the directory synced.
pub fn write_network_choice(root: &Path, choice: NetworkChoice) -> Result<(), EngineError> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static WRITES: AtomicU64 = AtomicU64::new(0);

    let json = serde_json::to_string_pretty(&File { network: choice })
        .map_err(|e| EngineError::Internal(e.to_string()))?;
    let tmp = root.join(format!(
        "{NETWORK_CHOICE_FILE}.{}.{}.tmp",
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path(root))
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written?;
    #[cfg(unix)]
    std::fs::File::open(root)?.sync_all()?;
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
        let names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec![NETWORK_CHOICE_FILE.to_owned()],
            "no temporary file left"
        );
    }

    /// Two writers at once (the app and `dwcli`) each replace the whole
    /// file: neither fails on the other's temporary file.
    #[test]
    fn concurrent_writes_each_replace_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let writers: Vec<_> = (0..8)
            .map(|i| {
                let root = root.clone();
                std::thread::spawn(move || {
                    let choice = if i % 2 == 0 {
                        NetworkChoice::Testnet
                    } else {
                        NetworkChoice::Mainnet
                    };
                    for _ in 0..20 {
                        write_network_choice(&root, choice).unwrap();
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        assert!(read_network_choice(&root).is_some());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
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
