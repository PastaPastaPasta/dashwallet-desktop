//! Shared fixture of the dw-vault integration tests: a vault directory in a
//! temp dir, an in-memory OS store, a settable clock and cheap Argon2id
//! parameters (`insecure-test-kdf`, regtest only).

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dw_vault::{
    Clock, KdfParams, KdfPolicy, MemoryOsStore, SeedDerivation, Vault, VaultConfig, WalletId,
    WalletSecret,
};
use key_wallet::Network;
use zeroize::Zeroizing;

pub const PASS: &[u8] = b"correct horse";
pub const OTHER: &[u8] = b"battery staple";
pub const TTL: u64 = 120;
/// Start time of the test clock (UNIX seconds).
pub const T0: u64 = 1_800_000_000;

/// A clock the test moves by hand.
#[derive(Debug)]
pub struct TestClock(AtomicU64);

impl TestClock {
    pub fn advance(&self, secs: u64) {
        self.0.fetch_add(secs, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now_secs(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub clock: Arc<TestClock>,
    pub os: Arc<MemoryOsStore>,
    pub kdf: KdfParams,
}

impl Fixture {
    pub fn new() -> Self {
        Self::with_kdf(KdfParams::TEST)
    }

    pub fn with_kdf(kdf: KdfParams) -> Self {
        Self {
            dir: tempfile::tempdir().expect("temp dir"),
            clock: Arc::new(TestClock(AtomicU64::new(T0))),
            os: Arc::new(MemoryOsStore::new()),
            kdf,
        }
    }

    pub fn config(&self) -> VaultConfig {
        VaultConfig {
            kdf: KdfPolicy::Fixed(self.kdf),
            os_store: self.os.clone(),
            clock: self.clock.clone(),
            grant_ttl_secs: TTL,
        }
    }

    pub fn vault_dir(&self) -> PathBuf {
        self.dir.path().join("vault")
    }

    /// Opens the vault as a fresh process would (no in-memory state).
    pub fn open(&self) -> Vault {
        Vault::open(self.vault_dir(), Network::Regtest, "regtest", self.config()).expect("open")
    }

    pub fn file_path(&self) -> PathBuf {
        self.vault_dir().join("vault.dwv")
    }

    pub fn read_json(&self) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(self.file_path()).expect("read vault file"))
            .expect("vault file is JSON")
    }

    pub fn write_json(&self, v: &serde_json::Value) {
        std::fs::write(self.file_path(), serde_json::to_vec_pretty(v).unwrap()).unwrap();
    }

    pub fn raw(&self) -> Vec<u8> {
        std::fs::read(self.file_path()).expect("read vault file")
    }

    pub fn restore_raw(&self, bytes: &[u8]) {
        std::fs::write(self.file_path(), bytes).unwrap();
    }
}

pub fn wallet(n: u8) -> WalletId {
    [n; 32]
}

/// A distinct secret per `n`: phrase `phrase-<n>`, seed filled with `n`.
pub fn secret(n: u8) -> WalletSecret {
    WalletSecret {
        mnemonic: Zeroizing::new(format!("phrase-{n}").into_bytes()),
        mnemonic_passphrase: Zeroizing::new(format!("bip39-{n}").into_bytes()),
        seed: Zeroizing::new([n; 64]),
        derivation: SeedDerivation::Bip39,
    }
}

/// Flips one hex digit of a hex string field, keeping it valid hex.
pub fn flip_hex(s: &str) -> String {
    let mut chars: Vec<char> = s.chars().collect();
    chars[0] = if chars[0] == '0' { '1' } else { '0' };
    chars.into_iter().collect()
}
