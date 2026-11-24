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

/// What [`hold_close_fence`] holds.
pub enum CloseFence {
    /// The load fence, as a wallet load in flight holds it.
    WalletLoad,
    /// The close preflight and an operation guard, as a signing shielded
    /// call holds them.
    ShieldedSpend,
    /// An operation guard, as any operation in flight holds it.
    Operation,
}

/// A fence held by [`hold_close_fence`]. Dropping it returns once the fence
/// is released, so a close asked right after is not refused by it.
pub struct HeldCloseFence {
    release: Option<tokio::sync::oneshot::Sender<()>>,
    released: std::sync::mpsc::Receiver<()>,
}

impl Drop for HeldCloseFence {
    fn drop(&mut self) {
        drop(self.release.take());
        let _ = self.released.recv();
    }
}

/// Holds `fence` of `session` until the returned guard is dropped, so an
/// in-app close of it is refused: `wallet` once it waited `load_wait`,
/// `shielded_spend_unresolved`, or (a switch's close) not drained. Call it
/// off the engine's runtime.
pub fn hold_close_fence(
    session: &std::sync::Arc<NetworkSession>,
    fence: CloseFence,
    load_wait: std::time::Duration,
) -> HeldCloseFence {
    *session.close_load_wait.lock().unwrap() = Some(load_wait);
    let (held_tx, held) = std::sync::mpsc::channel();
    let (done_tx, done) = std::sync::mpsc::channel::<()>();
    let (release, released) = tokio::sync::oneshot::channel::<()>();
    let s = std::sync::Arc::clone(session);
    session.rt.spawn(async move {
        let _fence = match fence {
            CloseFence::WalletLoad => Some(s.platform.wallet_loads.read().await),
            CloseFence::ShieldedSpend => Some(s.platform.close_preflight.read().await),
            CloseFence::Operation => None,
        };
        let _op = match fence {
            CloseFence::WalletLoad => None,
            _ => Some(s.enter().await.expect("the session is open")),
        };
        let _ = held_tx.send(());
        let _ = released.await;
        drop((_fence, _op));
        drop(done_tx);
    });
    held.recv().expect("the fence is held");
    HeldCloseFence {
        release: Some(release),
        released: done,
    }
}

/// Marks `wallet` mixing (or not) without starting its loop.
pub fn set_mixing(session: &NetworkSession, wallet: crate::WalletId, mixing: bool) {
    session.coinjoin.set_mixing_for_test(wallet, mixing);
}

/// Holds the next Platform send of `session` (`DashPay::platform_send`)
/// before it starts, until the returned sender is dropped. The receiver
/// hears once it is held.
pub fn hold_platform_send(
    session: &NetworkSession,
) -> (
    std::sync::mpsc::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
) {
    let (reached_tx, reached) = std::sync::mpsc::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    *session.hold_platform_send.lock().unwrap() = Some((reached_tx, released));
    (reached, release)
}

/// The txids `wallet`'s durable sent marks name
/// (`tx_actions::SENT_PREFIX`), straight from the app database.
pub fn sent_marks(session: &NetworkSession, wallet: crate::WalletId) -> Vec<String> {
    let live = session.live().expect("a live session");
    let prefix = crate::tx_actions::SENT_PREFIX;
    (live.appdb.settings_with_prefix(&wallet.to_string(), prefix))
        .expect("read the sent marks")
        .into_iter()
        .map(|(key, _)| key[prefix.len()..].to_string())
        .collect()
}

/// Ends every wallet's unfinished replay as SPV would with nothing new to
/// find: its checkpoint back at the replay's target, stored, and the record
/// settled. For a data root opened offline, which has no blocks to replay.
/// Each replay must hold the wallet's spends until then. Returns every
/// wallet with a replay record, cancelled or not.
pub async fn finish_replays(session: &NetworkSession) -> Vec<FinishedReplay> {
    use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
    use platform_wallet::changeset::{
        CoreChangeSet, PlatformWalletChangeSet, PlatformWalletPersistence,
    };
    use platform_wallet::key_wallet_manager::WalletInterface;
    let live = session.live().expect("a live session");
    let mut finished = Vec::new();
    for id in live.manager.wallet_ids().await {
        let record = (live.replay.get(crate::WalletId(id)).await).expect("read the replay record");
        let wallet = crate::WalletId(id);
        let Some(record) = record else {
            continue;
        };
        if record.cancelled {
            finished.push(FinishedReplay {
                wallet,
                from_birth: false,
                processed_rise: 0,
            });
            continue;
        }
        assert!(
            session.replay_guard(wallet, 0).await.is_err(),
            "a spend of {wallet} waits for its replay"
        );
        let birth = crate::keys::birth_checkpoint(&live.manager, wallet).await;
        let height = record.target;
        let processed_rise = {
            let wm = live.manager.wallet_manager_arc();
            let mut wm = wm.write().await;
            let before =
                (wm.get_wallet_info(&id)).map_or(height, |i| i.core_wallet.last_processed_height());
            wm.update_wallet_synced_height(&id, height);
            wm.update_wallet_last_processed_height(&id, height);
            height.saturating_sub(before)
        };
        (session.hub).apply_wallet_event(
            &platform_wallet::events::WalletEvent::SyncHeightAdvanced {
                wallet_id: id,
                height,
            },
        );
        let core = CoreChangeSet {
            last_processed_height: Some(height),
            synced_height: Some(height),
            ..Default::default()
        };
        let changeset = PlatformWalletChangeSet {
            core: Some(core),
            ..Default::default()
        };
        live.store
            .store(id, changeset)
            .expect("store the checkpoint");
        live.store.flush(id).expect("flush the checkpoint");
        finished.push(FinishedReplay {
            wallet,
            from_birth: record.reached <= birth,
            processed_rise,
        });
    }
    live.replay.settled().await;
    finished
}

/// A replay [`finish_replays`] ended.
pub struct FinishedReplay {
    pub wallet: crate::WalletId,
    /// It ran from the wallet's birth checkpoint or below.
    pub from_birth: bool,
    /// How far it raised the wallet's processed height, which coins'
    /// confirmations count from.
    pub processed_rise: u32,
}
