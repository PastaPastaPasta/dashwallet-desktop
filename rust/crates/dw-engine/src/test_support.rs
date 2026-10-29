//! Requests through a session's own SDK, built from its options as every
//! session's is, for other crates' tests (MG-03 D37), and DP5-02's
//! invitation maker for its T2. Compiled only with the `test-support`
//! feature in a debug build: dwcli's dev-dependency and its
//! `invitation-create-test` feature are all that enable it, so a debug
//! build of dw-engine with it has this API (hidden from its docs), and a
//! release or dist build, which has no `debug_assertions`, never does
//! (D40; dwcli refuses the feature in such a build, DP5-02 D18).

use dash_sdk::platform::Fetch;
use dash_sdk::query_types::IdentityBalance;
use dpp::prelude::Identifier;

use crate::{EngineError, NetworkSession};

/// The credits read's request (an identity balance, MG-03 §6) with no
/// masternode list: whether it succeeded.
pub async fn fetch_identity_balance(
    session: &NetworkSession,
    identity: [u8; 32],
) -> Result<bool, EngineError> {
    let sdk = session.manager()?.sdk_arc();
    let fetch = async move {
        Ok(IdentityBalance::fetch(&sdk, Identifier::from(identity))
            .await
            .is_ok())
    };
    session.on_runtime(fetch).await
}

/// The shared-masternode facade with sending set by hand (MG-04b r2-m4,
/// DEC-678): whatever `SHARED_SENDING` says, the calls that produce bytes
/// the network accepts send or refuse `unsupported` as `can_send` says, for
/// the in-process session tests of other crates. Compiled only where this
/// module is (a debug build with `test-support`); the release check
/// (`tests/mg04b_release_check.rs`) proves it absent from a release build.
pub fn shared_masternodes_with_send_override(
    session: &std::sync::Arc<NetworkSession>,
    can_send: bool,
) -> crate::masternodes::Masternodes {
    crate::masternodes::Masternodes::with_shared_send(session, can_send)
}

impl crate::TestSupportProbe {
    /// Present only where this module is compiled.
    pub const SHARED_SEND_OVERRIDE: bool = true;
}

/// A DashPay invitation of `duffs` funded by wallet `wallet` (DP5-02 T2,
/// dwcli's test-only `invite create`): the library builds, tracks and
/// broadcasts the voucher lock at the DIP-13 invitation account, signed by
/// the vault under a `Registration` lease of `grant`, and waits for its
/// proof (InstantSend, or a ChainLock after the library's window). The
/// link carries that proof and the voucher key, derived here from `phrase`
/// at the lock's path: the engine's signers export no private key.
/// `inviter` is the link's `du`. Returns the link, a bearer secret.
pub async fn create_invitation(
    session: &std::sync::Arc<NetworkSession>,
    wallet: crate::WalletId,
    grant: String,
    duffs: u64,
    inviter: Option<String>,
    phrase: &[u8],
) -> Result<zeroize::Zeroizing<String>, crate::platform::PlatformError> {
    use crate::platform::PlatformError;
    use dashcore::secp256k1::SecretKey;
    use key_wallet::bip32::ExtendedPrivKey;
    use platform_wallet::AssetLockFundingType;
    use platform_wallet::wallet::identity::crypto::invitation::{
        InviterInfo, encode_invitation_uri,
    };
    use platform_wallet::wallet::identity::network::{MAX_INVITATION_DUFFS, MIN_INVITATION_DUFFS};

    if !(MIN_INVITATION_DUFFS..=MAX_INVITATION_DUFFS).contains(&duffs) {
        return Err(PlatformError::InvalidArgument {
            detail: format!(
                "an invitation is {MIN_INVITATION_DUFFS}..={MAX_INVITATION_DUFFS} duffs"
            ),
        });
    }
    let secret = dw_vault::mnemonic::derive_secret(phrase, b"", false).map_err(|e| {
        PlatformError::InvalidArgument {
            detail: e.to_string(),
        }
    })?;
    let lease = session
        .begin_lease(wallet, crate::platform::FlowKind::Registration, &[grant])
        .await?;
    let signer = lease.uncounted_funding_signer()?;
    let (_, lib) = crate::platform::registration_flow::PwLib::open(session, wallet).await?;
    let network = lib.sdk.network;
    let created = session
        .on_runtime(async move {
            Ok(lib
                .wallet
                .asset_locks()
                .create_funded_asset_lock_proof(
                    duffs,
                    0,
                    AssetLockFundingType::IdentityInvitation,
                    0,
                    &signer,
                )
                .await)
        })
        .await?;
    let (proof, path, _) = created?;
    let mut master = ExtendedPrivKey::new_master(network, &secret.seed[..]).map_err(|e| {
        PlatformError::Internal {
            detail: e.to_string(),
        }
    })?;
    let child = master.derive_priv(&path);
    master.private_key.non_secure_erase();
    let mut child = child.map_err(|e| PlatformError::Internal {
        detail: e.to_string(),
    })?;
    let mut voucher: SecretKey = child.private_key;
    child.private_key.non_secure_erase();
    let inviter = inviter.map(|username| InviterInfo {
        username: Some(username),
        display_name: None,
        avatar_url: None,
    });
    let uri = encode_invitation_uri(&voucher, network, &proof, inviter.as_ref());
    voucher.non_secure_erase();
    Ok(zeroize::Zeroizing::new(uri?))
}
