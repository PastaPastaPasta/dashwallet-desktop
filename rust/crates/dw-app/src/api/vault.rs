//! The vault: lock state, unlocking and locking, credentials and grants,
//! quick unlock, recovery, and the BIP39 helpers hosts call before a wallet
//! exists.
//!
//! Every call that runs Argon2id or writes the vault file goes through
//! `NetworkSession::vault_op` (blocking pool, lock-state event on change);
//! the ones that end the vault's epoch on purpose go through
//! `revoking_vault_op` or `change_passphrase`, as dw-ffi's `Vault` does.
//!
//! Secrets the renderer sends (passphrases, the quick-unlock wrap key,
//! phrases) are moved into `Zeroizing` buffers as soon as the command body
//! starts; the renderer's own copies (DOM values, JS strings) cannot be
//! zeroed (G-01 H-01). Secrets returned to the renderer (a revealed phrase,
//! a fresh quick-unlock key) are plain strings for the same reason.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use dw_engine::EngineError;
use dw_engine::platform::RevokeCause;
use dw_vault::mnemonic::{Checksum, Language};
use dw_vault::{Credential, GrantPurpose, MnemonicError, UnlockScope};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use zeroize::Zeroizing;

use crate::dto::platform::LockOutcome;
use crate::dto::{LockStatus, mirror_enum};
use crate::error::{AppError, EngineCode};
use crate::ipc::commands;
use crate::state::{AppState, parse_wallet_id};

pub(crate) const EXCLUDED: &[(&str, &str)] = &[
    (
        "Vault::stash_link",
        "internal: the engine's `NetworkSession::stash_link` calls it after classifying the link itself (DP5-01 D3)",
    ),
    (
        "Vault::stashed_links",
        "internal: reached through `NetworkSession::stashed_links` (`stashed_links`)",
    ),
    (
        "Vault::take_stashed_link",
        "internal: reached through `NetworkSession::take_stashed_link` (`open_stashed_link`), which passes only the kinds it hands back",
    ),
    (
        "Vault::forget_stashed_link",
        "internal: reached through `NetworkSession::forget_invitation` (`forget_invitation`) and `NetworkSession::forget_stashed_link` (`dismiss_stashed_link`)",
    ),
    (
        "NetworkSession::lock_vault_sync",
        "internal: superseded by `lock` (`lock_vault`, async, awaits the lease drain); dw-ffi's synchronous `Vault::lock` maps to it for the frozen Swift shells, and the console and the relock timer use it",
    ),
    (
        "NetworkSession::masternode_keys",
        "deferred (MG): masternode key export is bound with the Masternodes domain",
    ),
    (
        "NetworkSession::reveal_masternode_key",
        "deferred (MG): masternode key reveal is bound with the Masternodes domain",
    ),
    (
        "NetworkSession::masternodes",
        "deferred (MG): the masternodes facade (mn-engine.md) is bound with the Masternodes domain (MG-03 D8)",
    ),
    (
        "Vault::with_revealed_seed",
        "internal: a closure over the seed for the engine's own key derivations (masternode keys, MG), never a host call",
    ),
    (
        "Vault::store_masternode_key",
        "internal: reached through `Masternodes::attach_key`, which checks the key first (MG-03 D8, MG-01 D7)",
    ),
    (
        "Vault::masternode_key",
        "deferred (MG-03/MG-05): reading an attached key is bound with the Masternodes domain and its grant purposes (MG-01 D11)",
    ),
    (
        "Vault::masternode_key_roles",
        "internal: reached through the masternodes facade's rows (`MasternodeRoles::attached`, MG-03 D8, MG-01 D15)",
    ),
    (
        "Vault::masternode_key_id",
        "internal: the engine records an attached key's id from it after a vault write whose outcome it couldn't tell, and compares every recorded id with it once a session (MG-03 D31, D34)",
    ),
    (
        "Vault::delete_masternode_keys",
        "internal: reached through `Masternodes::untrack` and `detach_key` (MG-03 D8, MG-00 D22)",
    ),
    (
        "Vault::grant_identity_key_matches",
        "internal: the engine's contact-request writes ask it before they redeem a grant (DP2-02); the UI sees the refusal",
    ),
    (
        "Vault::redeem_grant_holding_key",
        "internal: the engine's contact-request writes redeem their grant through it (DP2-02); the grant comes from `authorize` as for any write",
    ),
    (
        "Vault::open",
        "internal: the engine opens the vault when it opens a network",
    ),
    (
        "Vault::dir",
        "internal: a path the engine owns",
    ),
    (
        "Vault::vault_id",
        "internal: keys the engine's per-vault settings (`paymentAuthentication`); the id itself means nothing to the UI",
    ),
    (
        "Vault::network",
        "internal: `openNetwork` returns the network",
    ),
    (
        "Vault::lock",
        "internal: `lock` goes through NetworkSession::lock_vault, which revokes the leases around the vault's own lock",
    ),
    (
        "Vault::lock_state",
        "internal: `Vault::status` carries the state",
    ),
    (
        "Vault::change_passphrase",
        "internal: `change_passphrase` goes through NetworkSession::change_passphrase, which verifies before it revokes (DEC-134)",
    ),
    (
        "Vault::epoch",
        "internal: the lease table compares epochs around `vault_op`",
    ),
    (
        "Vault::purge_expired_grants",
        "internal: the engine purges grants on its own events",
    ),
    (
        "Vault::check_grant",
        "internal: the engine checks a grant before it spends or signs",
    ),
    (
        "Vault::grant_purpose",
        "internal: the engine reads a grant's purpose to bind it to a lease",
    ),
    (
        "Vault::redeem_grant",
        "internal: the engine redeems the grants the commands pass it",
    ),
    (
        "Vault::hold_key",
        "internal: key holds back the lease table's grants",
    ),
    (
        "Vault::signer",
        "internal: signers never cross the IPC boundary; the engine builds them under a redeemed grant",
    ),
    (
        "Vault::signer_held",
        "internal: signers never cross the IPC boundary; the lease table builds them",
    ),
    (
        "Vault::platform_signer",
        "internal: Platform signers are built by the engine under a PlatformOp grant",
    ),
    (
        "Vault::platform_signer_held",
        "internal: Platform signers are built by the lease table",
    ),
    (
        "Vault::vote_signer",
        "internal: the vote signer is built by the engine under a Governance grant (MG-05)",
    ),
    (
        "Vault::vote_signer_held",
        "internal: the vote signer is built by the lease table (MG-05)",
    ),
    (
        "Vault::masternode_signer",
        "internal: masternode signers are built by the engine under a MasternodeOp grant (MG-05)",
    ),
    (
        "Vault::masternode_signer_held",
        "internal: masternode signers are built by the lease table (MG-05)",
    ),
    (
        "Vault::dashpay_crypto_signer",
        "internal: the DashPay signer is built by the engine's bring-up and leases",
    ),
    (
        "Vault::mixing_signer",
        "internal: the CoinJoin session builds its mixing signer",
    ),
    (
        "Vault::mixing_funding_signer",
        "internal: the CoinJoin session builds its funding signer",
    ),
    (
        "Vault::scan_key",
        "internal: the identity scan holds the master key inside the engine",
    ),
    (
        "Vault::scan_key_held",
        "internal: the identity scan holds the master key inside the engine",
    ),
    (
        "Vault::store_wallet_secret",
        "internal: `importWallet` and the backup restore store seeds, never the host directly",
    ),
    (
        "Vault::delete_wallet_secret",
        "internal: `removeWallet` and a failed import delete a wallet's records",
    ),
    (
        "Vault::wipe_wallet_secret",
        "internal: `removeWallet` redeems the Wipe grant and calls it",
    ),
    (
        "Vault::has_wallet_secret",
        "internal: `WalletSummary.watchOnly` and `LockStatus.walletsWithSecrets` report it",
    ),
    (
        "Vault::has_wallet_mnemonic",
        "internal: `WalletSummary.hasMnemonic` reports it",
    ),
    (
        "Vault::seed_derivation",
        "internal: the engine reads how a stored seed was derived (strict BIP39 or Dash Core) when it restores keys",
    ),
    (
        "Vault::export_wallet_secret",
        "internal: the Dash Core export reads the seed inside the engine",
    ),
    (
        "Vault::core_mnemonic_check",
        "internal: the Dash Core compatibility checks run inside the engine",
    ),
    (
        "Vault::backup_bundle",
        "internal: backup bundles are written by the engine's backup scheduler",
    ),
    (
        "Vault::open_backup_bundle",
        "internal: backup bundles are read by the engine's restore",
    ),
    (
        "Vault::wrote_bundle",
        "internal: the engine's backup bookkeeping",
    ),
    (
        "Vault::own_key_only",
        "internal: the engine's backup bookkeeping",
    ),
];

/// Dash Core caps `walletpassphrase`'s timeout at this many seconds.
const MAX_RELOCK_SECS: u64 = 100_000_000;

/// What a grant authorizes (E0-04 design §3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum GrantPurposeSpec {
    /// `TxDraft::prepare`; the transaction may debit at most `max_duffs`.
    Spend { max_duffs: u64 },
    /// Revealing the recovery phrase.
    RevealSecret,
    /// `signMessage`.
    SignMessage,
    /// Passphrase change, vault encryption, quick-unlock enrolment and policy.
    /// Not bound to a wallet.
    ChangeCredential,
    /// `removeWallet`.
    Wipe,
    /// Platform writes: Core funding up to `max_duffs`, credits up to `max_credits`.
    PlatformOp {
        max_duffs: u64,
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        max_credits: u64,
    },
    /// The identity scan, the only grant that releases the wallet's master key.
    IdentityScan,
    /// Governance votes with the wallet's voting keys; signs no transaction.
    Governance,
    /// Masternode transactions (ProRegTx, ProUpRegTx): each may take at most
    /// `max_duffs` out of the wallet, fee included.
    MasternodeOp { max_duffs: u64 },
}

impl From<GrantPurposeSpec> for GrantPurpose {
    fn from(p: GrantPurposeSpec) -> Self {
        use GrantPurposeSpec as S;
        match p {
            S::Spend { max_duffs } => GrantPurpose::Spend { max_duffs },
            S::RevealSecret => GrantPurpose::RevealSecret,
            S::SignMessage => GrantPurpose::SignMessage,
            S::ChangeCredential => GrantPurpose::ChangeCredential,
            S::Wipe => GrantPurpose::Wipe,
            S::PlatformOp {
                max_duffs,
                max_credits,
            } => GrantPurpose::PlatformOp {
                max_duffs,
                max_credits,
            },
            S::IdentityScan => GrantPurpose::IdentityScan,
            S::Governance => GrantPurpose::Governance,
            S::MasternodeOp { max_duffs } => GrantPurpose::MasternodeOp { max_duffs },
        }
    }
}

impl From<GrantPurpose> for GrantPurposeSpec {
    fn from(p: GrantPurpose) -> Self {
        use GrantPurpose as P;
        match p {
            P::Spend { max_duffs } => Self::Spend { max_duffs },
            P::RevealSecret => Self::RevealSecret,
            P::SignMessage => Self::SignMessage,
            P::ChangeCredential => Self::ChangeCredential,
            P::Wipe => Self::Wipe,
            P::PlatformOp {
                max_duffs,
                max_credits,
            } => Self::PlatformOp {
                max_duffs,
                max_credits,
            },
            P::IdentityScan => Self::IdentityScan,
            P::Governance => Self::Governance,
            P::MasternodeOp { max_duffs } => Self::MasternodeOp { max_duffs },
        }
    }
}

/// The credential presented to `authorize`.
#[derive(Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum CredentialSpec {
    /// The vault passphrase.
    Passphrase { passphrase: String },
    /// The 32-byte key the OS biometric store released, base64.
    QuickUnlock {
        #[ts(type = "string")]
        #[serde(with = "crate::dto::base64")]
        wrap_key: Vec<u8>,
    },
    /// No credential: an unencrypted vault, or a full-scope unlocked one for
    /// purposes that do not need the passphrase.
    Unencrypted,
}

impl std::fmt::Debug for CredentialSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CredentialSpec::Passphrase { .. } => "Passphrase(<redacted>)",
            CredentialSpec::QuickUnlock { .. } => "QuickUnlock(<redacted>)",
            CredentialSpec::Unencrypted => "Unencrypted",
        })
    }
}

/// The credential with its bytes in a zeroing buffer.
enum OwnedCredential {
    Passphrase(Zeroizing<Vec<u8>>),
    QuickUnlock(Zeroizing<Vec<u8>>),
    None,
}

impl From<CredentialSpec> for OwnedCredential {
    fn from(c: CredentialSpec) -> Self {
        match c {
            CredentialSpec::Passphrase { passphrase } => {
                Self::Passphrase(Zeroizing::new(passphrase.into_bytes()))
            }
            CredentialSpec::QuickUnlock { wrap_key } => Self::QuickUnlock(Zeroizing::new(wrap_key)),
            CredentialSpec::Unencrypted => Self::None,
        }
    }
}

/// "Require authentication for every payment" at the IPC boundary: while
/// it is on, an unlocked encrypted vault issues `spend`, `platformOp`,
/// `masternodeOp` and `governance` grants only against the passphrase or
/// the quick-unlock key, never `unencrypted`, so a compromised renderer
/// cannot obtain a payment or voting grant without a prompt (MG-05 D11). The vault itself takes `Credential::None` for them
/// once unlocked (dash-qt behaviour, the setting off); a locked vault
/// refuses it itself (`vault.locked`).
fn check_payment_auth(
    vault: &dw_vault::Vault,
    credential: &OwnedCredential,
    required: bool,
) -> Result<(), dw_vault::VaultError> {
    let status = vault.status();
    if required
        && matches!(credential, OwnedCredential::None)
        && status.encrypted
        && status.state != dw_vault::LockState::Locked
    {
        return Err(dw_vault::VaultError::CredentialRequired);
    }
    Ok(())
}

/// Issues grants through `issue` under "require authentication for every
/// payment" (D11). The setting and its generation are read together under
/// [`PaymentPolicy::gate`](crate::state::PaymentPolicy); once the grants
/// exist the generation is compared again. If it moved (the setter
/// advances it before it writes, under the same gate) or the vault is no
/// longer the one the setting was read for, the stricter rule decides, and
/// a refusal revokes what was issued before the call fails (Sol r2/r3
/// F1b). So a grant issued once "on" is readable was not judged under
/// "off"; one issued before the change keeps its 120 s, as D11 says.
async fn issue_grants(
    state: &AppState,
    purposes: &[GrantPurpose],
    credential: OwnedCredential,
    issue: impl FnOnce(&dw_vault::Vault, Credential<'_>) -> Result<Vec<dw_vault::AuthGrant>, dw_vault::VaultError>
    + Send
    + 'static,
) -> Result<Vec<dw_vault::AuthGrant>, AppError> {
    let session = state.session().await?;
    let payment_policy = Arc::clone(&state.payment_policy);
    let (read_at, policy) = if pays(purposes) {
        #[cfg(test)]
        crate::state::Seam::pass_slot(&state.payment_gate_seam).await;
        let _gate = payment_policy.gate.lock().await;
        let read_at = payment_policy.generation.load(Ordering::SeqCst);
        (read_at, Some(session.payment_authentication().await?))
    } else {
        (0, None)
    };
    #[cfg(test)]
    if policy.is_some() {
        crate::state::Seam::pass_slot(&state.payment_seam).await;
    }
    let grants = session
        .vault_op(move |v| {
            check_payment_auth(v, &credential, policy.as_ref().is_some_and(|p| p.on))?;
            let grants = issue(v, credential.as_credential())?;
            if let Some(policy) = &policy
                && (payment_policy.generation.load(Ordering::SeqCst) != read_at
                    || v.vault_id() != policy.vault_id)
                && let Err(e) = check_payment_auth(v, &credential, true)
            {
                for grant in &grants {
                    v.revoke_grant(&grant.id);
                }
                return Err(e);
            }
            Ok(grants)
        })
        .await?;
    Ok(grants)
}

/// Whether the setting applies: a purpose that moves funds (`spend`,
/// `platformOp`, `masternodeOp`) among `purposes`, or casts votes
/// (`governance`: MG-00's "N votes, one passphrase", MG-05 D11).
fn pays(purposes: &[GrantPurpose]) -> bool {
    purposes.iter().any(|p| {
        matches!(
            p,
            GrantPurpose::Spend { .. }
                | GrantPurpose::PlatformOp { .. }
                | GrantPurpose::MasternodeOp { .. }
                | GrantPurpose::Governance
        )
    })
}

impl OwnedCredential {
    fn as_credential(&self) -> Credential<'_> {
        match self {
            Self::Passphrase(p) => Credential::Passphrase(p),
            Self::QuickUnlock(k) => Credential::QuickUnlock(k),
            Self::None => Credential::None,
        }
    }
}

/// A single authorization. Grants live in engine memory only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AuthGrant {
    /// Opaque id passed to the call the grant authorizes.
    pub id: String,
    pub purpose: GrantPurposeSpec,
    /// The wallet the grant is bound to; `null` for vault-wide purposes.
    pub wallet_id: Option<String>,
    /// UNIX seconds after which the grant is refused.
    pub expires_at: u64,
    /// The grant is consumed by its first successful use.
    pub single_use: bool,
}

impl From<dw_vault::AuthGrant> for AuthGrant {
    fn from(g: dw_vault::AuthGrant) -> Self {
        let dw_vault::AuthGrant {
            id,
            purpose,
            wallet,
            expires_at,
            single_use,
        } = g;
        AuthGrant {
            id,
            purpose: purpose.into(),
            wallet_id: wallet.map(hex::encode),
            expires_at,
            single_use,
        }
    }
}

/// A revealed recovery phrase. Both fields are secret.
#[derive(Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RevealedMnemonic {
    /// Space-separated phrase.
    pub phrase: String,
    /// The BIP39 passphrase ("25th word"); empty when none.
    pub bip39_passphrase: String,
}

impl std::fmt::Debug for RevealedMnemonic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RevealedMnemonic(<redacted>)")
    }
}

impl RevealedMnemonic {
    /// Text for the renderer. The vault's copies are zeroed on drop; the
    /// BIP39 passphrase of a Dash Core import can be any bytes, and text
    /// cannot carry those unchanged.
    fn from_vault(revealed: dw_vault::RevealedMnemonic) -> Result<Self, AppError> {
        let dw_vault::RevealedMnemonic {
            phrase,
            bip39_passphrase,
        } = revealed;
        let text = |bytes: &[u8], what: &str| {
            std::str::from_utf8(bytes).map(str::to_owned).map_err(|_| {
                AppError::new(
                    EngineCode::Internal,
                    format!("the stored {what} is not UTF-8 and cannot be shown as text"),
                )
            })
        };
        Ok(RevealedMnemonic {
            phrase: text(&phrase, "recovery phrase")?,
            bip39_passphrase: text(&bip39_passphrase, "BIP39 passphrase")?,
        })
    }
}

/// The biometric quick-unlock rules (IOS-011/016): a quick-unlock
/// credential issues a grant only up to `spendLimitDuffs`, and only while
/// the passphrase was entered within `passphraseMaxAgeSecs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct QuickUnlockPolicy {
    pub enrolled: bool,
    pub spend_limit_duffs: u64,
    pub passphrase_max_age_secs: u64,
    /// UNIX seconds of the last successful passphrase entry.
    pub last_passphrase_at: Option<u64>,
}

impl From<dw_vault::QuickUnlockPolicy> for QuickUnlockPolicy {
    fn from(p: dw_vault::QuickUnlockPolicy) -> Self {
        let dw_vault::QuickUnlockPolicy {
            enrolled,
            spend_limit_duffs,
            passphrase_max_age_secs,
            last_passphrase_at,
        } = p;
        QuickUnlockPolicy {
            enrolled,
            spend_limit_duffs,
            passphrase_max_age_secs,
            last_passphrase_at,
        }
    }
}

/// Outcome of `recoverWithMnemonic`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct VaultRecovery {
    /// The new vault, unlocked.
    pub status: LockStatus,
    /// Wallets whose secrets were in the old vault and are now watch-only:
    /// import their phrases again to restore their keys.
    pub wallets_without_secrets: Vec<String>,
}

/// BIP39 wordlists (IOS-007: restore in 10 languages).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum MnemonicLanguageName {
    English,
    ChineseSimplified,
    ChineseTraditional,
    Czech,
    French,
    Italian,
    Japanese,
    Korean,
    Portuguese,
    Spanish,
}

mirror_enum!(Language => MnemonicLanguageName { English, ChineseSimplified, ChineseTraditional, Czech, French, Italian, Japanese, Korean, Portuguese, Spanish });
mirror_enum!(MnemonicLanguageName => Language { English, ChineseSimplified, ChineseTraditional, Czech, French, Italian, Japanese, Korean, Portuguese, Spanish });

/// Checksum verdict of `checkMnemonic`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum MnemonicChecksumName {
    /// A valid BIP39 checksum.
    Valid,
    /// Invalid under BIP39 but accepted by Dash Core's weak check (QT-104);
    /// importable only with `coreCompat`.
    CoreOnly,
    Invalid,
}

mirror_enum!(Checksum => MnemonicChecksumName { Valid, CoreOnly, Invalid });

/// Word-by-word validation for the restore screen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MnemonicCheck {
    pub word_count: u32,
    /// Zero-based positions of words not in the detected wordlist.
    pub unknown_word_indices: Vec<u32>,
    /// `null` when no wordlist contains every word.
    pub language: Option<MnemonicLanguageName>,
    pub checksum: MnemonicChecksumName,
}

impl From<dw_vault::mnemonic::MnemonicCheck> for MnemonicCheck {
    fn from(c: dw_vault::mnemonic::MnemonicCheck) -> Self {
        let dw_vault::mnemonic::MnemonicCheck {
            word_count,
            unknown_word_indices,
            language,
            checksum,
        } = c;
        MnemonicCheck {
            word_count,
            unknown_word_indices,
            language: language.map(Into::into),
            checksum: checksum.into(),
        }
    }
}

/// The error codes the engine gives these failures when `importWallet` and
/// `createWallet` hit them.
fn mnemonic_error(e: MnemonicError) -> AppError {
    match e {
        MnemonicError::Invalid(detail) => EngineError::InvalidMnemonic(detail),
        MnemonicError::UnsupportedWordCount(n) => {
            EngineError::InvalidArgument(format!("unsupported word count {n}"))
        }
        MnemonicError::PassphraseNotUtf8 => {
            EngineError::InvalidArgument("BIP39 passphrase is not UTF-8".into())
        }
        MnemonicError::Entropy(detail) => EngineError::Internal(format!("entropy: {detail}")),
    }
    .into()
}

/// A fresh phrase as text; the vault-side buffer is zeroed on drop.
fn generated_phrase(word_count: u8, language: Language) -> Result<String, AppError> {
    let phrase = dw_vault::mnemonic::generate(word_count.into(), language).map_err(mnemonic_error)?;
    std::str::from_utf8(&phrase).map(str::to_owned).map_err(|_| {
        AppError::new(EngineCode::Internal, "the generated phrase is not UTF-8")
    })
}

commands! {
    #[facade("NetworkSession::vault", "Vault::status")]
    fn lock_status(state) -> LockStatus {
        Ok(state.session().await?.vault().status().into())
    }

    /// Full unlock with the vault passphrase. The copy this process owns is
    /// zeroed on drop; the renderer's (a DOM value, a JS string) cannot be
    /// (G-01 H-01).
    #[facade("NetworkSession::vault_op", "Vault::unlock")]
    fn unlock(state, passphrase: String) -> LockStatus {
        let passphrase = Zeroizing::new(passphrase);
        let session = state.session().await?;
        let status = session
            .vault_op(move |v| v.unlock(passphrase.as_bytes(), UnlockScope::Full))
            .await?;
        Ok(status.into())
    }

    /// dash-qt "Unlock for mixing only" (QT-112): the data key is in memory
    /// but signing is limited to CoinJoin mixing.
    #[facade("NetworkSession::vault_op", "Vault::unlock")]
    fn unlock_mixing_only(state, passphrase: String) -> LockStatus {
        let passphrase = Zeroizing::new(passphrase.into_bytes());
        let session = state.session().await?;
        let status = session
            .vault_op(move |v| v.unlock(&passphrase, UnlockScope::MixingOnly))
            .await?;
        Ok(status.into())
    }

    /// Dash Core's `walletpassphrase` (QT-145): unlocks, then locks again
    /// `timeoutSecs` from now. A later call replaces the timer; `lock` and
    /// closing the network cancel it.
    #[facade("NetworkSession::vault_op", "Vault::unlock", "NetworkSession::relock_after")]
    fn unlock_for(state, passphrase: String, timeout_secs: u64, mixing_only: bool) -> LockStatus {
        let passphrase = Zeroizing::new(passphrase.into_bytes());
        let scope = if mixing_only {
            UnlockScope::MixingOnly
        } else {
            UnlockScope::Full
        };
        let session = state.session().await?;
        let status = session
            .vault_op(move |v| v.unlock(&passphrase, scope))
            .await?;
        session.relock_after(Duration::from_secs(timeout_secs.min(MAX_RELOCK_SECS)));
        Ok(status.into())
    }

    /// Whether a `unlockFor` timer is pending.
    #[facade("NetworkSession::relock_pending")]
    fn relock_pending(state) -> bool {
        Ok(state.session().await?.relock_pending())
    }

    /// The full lock (E0-04 §8.1): revokes leases and returns after their
    /// drain, with its report (each flow it cut and how that ended, bounded
    /// as in D3); `lockProgress` events report it too.
    #[facade("NetworkSession::lock_vault")]
    fn lock(state) -> LockOutcome {
        Ok(state.session().await?.lock_vault().await?.into())
    }

    /// The report of the last revocation of every lease on this network
    /// since its session opened, or `null`: whatever revoked them (`lock`,
    /// the relock timer, the console's `walletlock`, `encryptVault`,
    /// `changePassphrase`, `recoverWithMnemonic`, `destroyVault`), it is what that
    /// `lockProgress` `done` said, even if an overflow dropped it (D3).
    /// Kept apart from the event queue.
    #[facade()]
    fn last_lock_report(state) -> Option<LockOutcome> {
        let network = state.session().await?.network().dir_name();
        Ok(state.memory.last_lock_report(&network))
    }

    /// Creates the vault and leaves it unlocked. A passphrase makes it
    /// encrypted (Argon2id); `null` keeps the key in the OS secret store.
    #[facade("NetworkSession::vault_op", "Vault::create")]
    fn create_vault(state, passphrase: Option<String>) -> LockStatus {
        let passphrase = passphrase.map(|p| Zeroizing::new(p.into_bytes()));
        let session = state.session().await?;
        let status = session
            .vault_op(move |v| v.create(passphrase.as_ref().map(|p| &p[..])))
            .await?;
        Ok(status.into())
    }

    /// dash-qt "Encrypt Wallet" (QT-111): adds the passphrase slot to an
    /// unencrypted vault. Needs a `changeCredential` grant. Revokes every
    /// lease and leaves the vault locked.
    #[facade("NetworkSession::revoking_vault_op", "Vault::encrypt")]
    fn encrypt_vault(state, new_passphrase: String, grant_id: String) -> LockStatus {
        let new_passphrase = Zeroizing::new(new_passphrase.into_bytes());
        let session = state.session().await?;
        let status = session
            .revoking_vault_op(RevokeCause::Lock, move |v| {
                v.encrypt(&new_passphrase, &grant_id)
            })
            .await?;
        Ok(status.into())
    }

    /// Re-wraps the data key under a new passphrase; the seed is unchanged
    /// (QT-111). A wrong old passphrase revokes nothing (DEC-134); a correct
    /// one revokes every lease.
    #[facade(
        "NetworkSession::change_passphrase",
        "Vault::check_passphrase_change",
        "Vault::apply_passphrase_change"
    )]
    fn change_passphrase(state, old_passphrase: String, new_passphrase: String) -> LockStatus {
        let old = Zeroizing::new(old_passphrase.into_bytes());
        let new = Zeroizing::new(new_passphrase.into_bytes());
        let status = state.session().await?.change_passphrase(old, new).await?;
        Ok(status.into())
    }

    /// Checks `credential` and issues a single-use grant for `purpose`.
    ///
    /// `walletId` binds the grant to its wallet: required for every purpose
    /// except `changeCredential`, which must pass `null`. A passphrase
    /// credential does not change the lock state. While "require
    /// authentication for every payment" is on, `spend` and `platformOp` on
    /// an encrypted vault need a passphrase or quick-unlock credential
    /// (`vault.credential_required`).
    #[facade("NetworkSession::vault_op", "Vault::authorize")]
    fn authorize(
        state,
        purpose: GrantPurposeSpec,
        wallet_id: Option<String>,
        credential: CredentialSpec
    ) -> AuthGrant {
        // The secret first: an early return must not drop it unwrapped.
        let credential = OwnedCredential::from(credential);
        let wallet = wallet_id.as_deref().map(parse_wallet_id).transpose()?;
        let purpose = GrantPurpose::from(purpose);
        let mut grants = issue_grants(&state, &[purpose], credential, move |v, credential| {
            v.authorize(purpose, wallet.as_ref().map(|w| &w.0), credential)
                .map(|g| vec![g])
        })
        .await?;
        let grant = grants
            .pop()
            .ok_or_else(|| AppError::new(EngineCode::Internal, "no grant issued"))?;
        Ok(grant.into())
    }

    /// One credential check for several grants (E0-04 §3.3), one Argon2id
    /// run and one throttle count: "Accept and pay" asks for a `platformOp`
    /// and a `spend` together. Grants come back in the order of `purposes`;
    /// every purpose must be allowed or none is issued; `authorize`'s
    /// payment rule applies to the set.
    #[facade("NetworkSession::vault_op", "Vault::authorize_set")]
    fn authorize_set(
        state,
        purposes: Vec<GrantPurposeSpec>,
        wallet_id: Option<String>,
        credential: CredentialSpec
    ) -> Vec<AuthGrant> {
        // The secret first: an early return must not drop it unwrapped.
        let credential = OwnedCredential::from(credential);
        let wallet = wallet_id.as_deref().map(parse_wallet_id).transpose()?;
        let purposes: Vec<GrantPurpose> = purposes.into_iter().map(Into::into).collect();
        let issue = purposes.clone();
        let grants = issue_grants(&state, &purposes, credential, move |v, credential| {
            v.authorize_set(&issue, wallet.as_ref().map(|w| &w.0), credential)
        })
        .await?;
        Ok(grants.into_iter().map(AuthGrant::from).collect())
    }

    /// Invalidates a grant before it expires. Unknown ids are ignored.
    #[facade("NetworkSession::vault", "Vault::revoke_grant")]
    fn revoke_grant(state, grant_id: String) -> () {
        state.session().await?.vault().revoke_grant(&grant_id);
        Ok(())
    }

    /// The recovery phrase and BIP39 passphrase of a wallet. Needs a
    /// `revealSecret` grant for it. Both results are secret.
    #[facade("NetworkSession::vault_op", "Vault::reveal_mnemonic")]
    fn reveal_mnemonic(state, wallet_id: String, grant_id: String) -> RevealedMnemonic {
        let id = parse_wallet_id(&wallet_id)?;
        let session = state.session().await?;
        let revealed = session
            .vault_op(move |v| v.reveal_mnemonic(&id.0, &grant_id))
            .await?;
        RevealedMnemonic::from_vault(revealed)
    }

    /// Enrols the biometric slot (IOS-011) and returns the 32-byte wrap key,
    /// base64, for the host to keep in the OS biometric store. Needs an
    /// encrypted vault and a `changeCredential` grant. Only macOS has such
    /// a store today; elsewhere `vault.quick_unlock_unavailable`.
    #[facade("NetworkSession::vault_op", "Vault::enroll_quick_unlock")]
    fn enroll_quick_unlock(state, grant_id: String) -> String {
        use ::base64::Engine as _;
        let session = state.session().await?;
        if !cfg!(target_os = "macos") {
            return Err(dw_vault::VaultError::QuickUnlockUnavailable.into());
        }
        let key = session
            .vault_op(move |v| v.enroll_quick_unlock(&grant_id))
            .await?;
        Ok(::base64::engine::general_purpose::STANDARD.encode(&key[..]))
    }

    /// Deletes the biometric slot. Idempotent; the spending limit is kept.
    /// The host deletes its OS item.
    #[facade("NetworkSession::vault_op", "Vault::remove_quick_unlock")]
    fn remove_quick_unlock(state) -> LockStatus {
        let session = state.session().await?;
        let status = session.vault_op(|v| v.remove_quick_unlock()).await?;
        Ok(status.into())
    }

    /// The quick-unlock rules. In-memory read; works while locked.
    #[facade("NetworkSession::vault", "Vault::quick_unlock_policy")]
    fn quick_unlock_policy(state) -> QuickUnlockPolicy {
        Ok(state.session().await?.vault().quick_unlock_policy().into())
    }

    /// "Require authentication for every payment" (default on), as stored
    /// for this network next to its vault.
    #[facade("NetworkSession::payment_authentication")]
    fn payment_authentication(state) -> bool {
        let session = state.session().await?;
        let _gate = state.payment_policy.gate.lock().await;
        Ok(session.payment_authentication().await?.on)
    }

    /// Turns "require authentication for every payment" on or off. Needs a
    /// `changeCredential` grant either way, so the passphrase; refused on a
    /// vault without one (`vault.not_encrypted`), where there is nothing to
    /// ask for. Grants issued and flows started before the change keep
    /// working until they are used or expire.
    #[facade("NetworkSession::set_payment_authentication")]
    fn set_payment_authentication(state, on: bool, grant_id: String) -> () {
        let session = state.session().await?;
        let _gate = state.payment_policy.gate.lock().await;
        // Before the write, which makes the value readable: a grant read
        // under the old value sees the change when it is issued (D11). A
        // write that then fails only makes such a grant ask again.
        state.payment_policy.generation.fetch_add(1, Ordering::SeqCst);
        #[cfg(test)]
        state.pause_payment_set(crate::state::SetPause::AfterBump).await;
        session.set_payment_authentication(on, grant_id).await?;
        #[cfg(test)]
        state.pause_payment_set(crate::state::SetPause::AfterWrite).await;
        Ok(())
    }

    /// Sets the biometric spending limit (IOS-016) to one of 0, 0.1, 0.5, 1
    /// or 5 DASH in duffs. Needs a `changeCredential` grant.
    #[facade("NetworkSession::vault_op", "Vault::set_quick_unlock_spend_limit")]
    fn set_quick_unlock_spend_limit(
        state,
        grant_id: String,
        spend_limit_duffs: u64
    ) -> QuickUnlockPolicy {
        let session = state.session().await?;
        let policy = session
            .vault_op(move |v| v.set_quick_unlock_spend_limit(&grant_id, spend_limit_duffs))
            .await?;
        Ok(policy.into())
    }

    /// Forgot passphrase (IOS-014): checks that the phrase and BIP39
    /// passphrase derive the wallet (`vault.recovery_mismatch` otherwise),
    /// then replaces the vault with one encrypted with `newPassphrase`
    /// holding only that wallet's secrets. Other wallets become watch-only
    /// and are listed in the result. Revokes every lease; the new vault is
    /// unlocked.
    #[facade("NetworkSession::revoking_vault_op", "Vault::recover_with_mnemonic")]
    fn recover_with_mnemonic(
        state,
        wallet_id: String,
        mnemonic: String,
        bip39_passphrase: String,
        new_passphrase: String
    ) -> VaultRecovery {
        let mnemonic = Zeroizing::new(mnemonic.into_bytes());
        let bip39_passphrase = Zeroizing::new(bip39_passphrase.into_bytes());
        let new_passphrase = Zeroizing::new(new_passphrase.into_bytes());
        let id = parse_wallet_id(&wallet_id)?;
        let session = state.session().await?;
        session.wallet_info(&id)?;
        let lost = session
            .revoking_vault_op(RevokeCause::Lock, move |v| {
                v.recover_with_mnemonic(&id.0, &mnemonic, &bip39_passphrase, &new_passphrase)
            })
            .await?;
        Ok(VaultRecovery {
            status: session.vault().status().into(),
            wallets_without_secrets: lost.iter().map(hex::encode).collect(),
        })
    }

    /// Deletes the vault once it holds no secrets (`vault.not_empty`): the
    /// last step of "Delete All" after every wallet was removed. The
    /// credential must be the passphrase on an encrypted vault. Idempotent.
    #[facade("NetworkSession::revoking_vault_op", "Vault::destroy")]
    fn destroy_vault(state, credential: CredentialSpec) -> LockStatus {
        let credential = OwnedCredential::from(credential);
        let session = state.session().await?;
        let status = session
            .revoking_vault_op(RevokeCause::Lock, move |v| {
                v.destroy(credential.as_credential())
            })
            .await?;
        Ok(status.into())
    }

    /// A fresh BIP39 phrase of 12, 15, 18, 21 or 24 words. Nothing is
    /// stored: the UI shows it, checks the user wrote it down (QT-103,
    /// IOS-004) and passes it to `importWallet`.
    #[facade("dw_vault::mnemonic::generate")]
    fn generate_mnemonic(state, word_count: u8, language: MnemonicLanguageName) -> String {
        generated_phrase(word_count, language.into())
    }

    /// Validates a typed or pasted phrase without importing it.
    #[facade("dw_vault::mnemonic::check")]
    fn check_mnemonic(state, phrase: String) -> MnemonicCheck {
        let phrase = Zeroizing::new(phrase.into_bytes());
        Ok(dw_vault::mnemonic::check(&phrase).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABANDON_12: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn grant_purposes_round_trip_with_credits_as_strings() {
        let platform = GrantPurposeSpec::PlatformOp {
            max_duffs: 5,
            max_credits: 123_456_789_012_345_678,
        };
        let json = serde_json::to_value(platform).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "kind": "platformOp",
                "maxDuffs": 5,
                "maxCredits": "123456789012345678"
            })
        );
        let back: GrantPurposeSpec = serde_json::from_value(json).unwrap();
        assert_eq!(back, platform);
        assert_eq!(
            serde_json::to_value(GrantPurposeSpec::IdentityScan).unwrap(),
            serde_json::json!({ "kind": "identityScan" })
        );
        let all = [
            GrantPurposeSpec::Spend { max_duffs: 7 },
            GrantPurposeSpec::RevealSecret,
            GrantPurposeSpec::SignMessage,
            GrantPurposeSpec::ChangeCredential,
            GrantPurposeSpec::Wipe,
            platform,
            GrantPurposeSpec::IdentityScan,
        ];
        for p in all {
            assert_eq!(GrantPurposeSpec::from(GrantPurpose::from(p)), p);
        }
    }

    #[test]
    fn credentials_parse_and_never_print() {
        let c: CredentialSpec =
            serde_json::from_str(r#"{"kind":"passphrase","passphrase":"hunter2"}"#).unwrap();
        assert!(!format!("{c:?}").contains("hunter2"));
        assert!(matches!(
            OwnedCredential::from(c).as_credential(),
            Credential::Passphrase(p) if p == b"hunter2"
        ));
        let c: CredentialSpec =
            serde_json::from_str(r#"{"kind":"quickUnlock","wrapKey":"AQID"}"#).unwrap();
        assert!(matches!(
            OwnedCredential::from(c).as_credential(),
            Credential::QuickUnlock(k) if k == [1, 2, 3]
        ));
        let c: CredentialSpec = serde_json::from_str(r#"{"kind":"unencrypted"}"#).unwrap();
        assert!(matches!(
            OwnedCredential::from(c).as_credential(),
            Credential::None
        ));
        assert!(serde_json::from_str::<CredentialSpec>(r#"{"kind":"bogus"}"#).is_err());
    }

    #[test]
    fn a_grant_maps_every_field() {
        let wallet = dw_engine::WalletId([0xab; 32]);
        let grant = AuthGrant::from(dw_vault::AuthGrant {
            id: "g1".into(),
            purpose: GrantPurpose::Wipe,
            wallet: Some(wallet.0),
            expires_at: 99,
            single_use: true,
        });
        assert_eq!(grant.wallet_id.as_deref(), Some("ab".repeat(32).as_str()));
        assert_eq!(grant.purpose, GrantPurposeSpec::Wipe);
        assert_eq!((grant.expires_at, grant.single_use), (99, true));
    }

    #[test]
    fn mnemonics_are_generated_checked_and_refused() {
        let phrase = generated_phrase(24, Language::English).unwrap();
        let check = MnemonicCheck::from(dw_vault::mnemonic::check(phrase.as_bytes()));
        assert_eq!(check.word_count, 24);
        assert_eq!(check.language, Some(MnemonicLanguageName::English));
        assert_eq!(check.checksum, MnemonicChecksumName::Valid);

        let err = generated_phrase(13, Language::English).unwrap_err();
        assert_eq!(err.code_str(), "invalid_argument");

        let bad = ABANDON_12.replace("about", "zzz");
        let check = MnemonicCheck::from(dw_vault::mnemonic::check(bad.as_bytes()));
        assert_eq!(check.unknown_word_indices, vec![11]);
        assert_eq!(check.checksum, MnemonicChecksumName::Invalid);
        assert_eq!(
            serde_json::to_value(&check).unwrap()["checksum"],
            serde_json::json!("invalid")
        );

        assert_eq!(
            mnemonic_error(MnemonicError::Invalid("x".into())).code_str(),
            "invalid_mnemonic"
        );
        assert_eq!(
            mnemonic_error(MnemonicError::Entropy("x".into())).code_str(),
            "internal"
        );
    }

    #[test]
    fn a_revealed_phrase_is_text_or_an_error() {
        let ok = RevealedMnemonic::from_vault(dw_vault::RevealedMnemonic {
            phrase: Zeroizing::new(ABANDON_12.as_bytes().to_vec()),
            bip39_passphrase: Zeroizing::new(Vec::new()),
        })
        .unwrap();
        assert_eq!(ok.phrase, ABANDON_12);
        assert!(ok.bip39_passphrase.is_empty());
        assert_eq!(format!("{ok:?}"), "RevealedMnemonic(<redacted>)");

        let err = RevealedMnemonic::from_vault(dw_vault::RevealedMnemonic {
            phrase: Zeroizing::new(ABANDON_12.as_bytes().to_vec()),
            bip39_passphrase: Zeroizing::new(vec![0xff, 0xfe]),
        })
        .unwrap_err();
        assert_eq!(err.code_str(), "internal");
    }
}
