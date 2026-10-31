//! The session's side of a masternode withdrawal: its [`Env`], the wallet's
//! keys (P2), the grant's signer and lease (P3, P12), the Platform it reads,
//! the crate-level calls the facade binds (`masternodes/withdrawals.rs`),
//! the quote's `withdrawal_ready` wait (P4) and the background recheck (P8).

use std::sync::Arc;

use dashcore::Network;
use dpp::identity::IdentityPublicKey;
use dpp::identity::core_script::CoreScript;
use dpp::state_transition::StateTransition;
use dw_appdb::{AppDb, PayoutBarrierRow, WithdrawalMark, WithdrawalRow};
use dw_vault::{SignerError, WithdrawalKey};

use dpp::document::Document;
use dpp::identity::Identity;
use dpp::prelude::Identifier;
use dpp::version::PlatformVersion;

use super::check::{Quote, look_until, quote};
use super::net::{Answer, MnWithdrawNet, Proved, SdkMnWithdrawals};
use super::recheck::{mark, recheck_all, recheck_record};
use super::sign::{Request, WithdrawalKeyChoice, retry, withdraw};
use super::{
    Alarm, Authorize, Chain, Env, HeldKey, Waits, WalletKeys, WithdrawalError, WithdrawalRuntime,
};
use crate::masternodes::protx_records::ProTxOperationState;
use crate::masternodes::{MasternodesError, display_hex, parse_pro_tx_hash};
use crate::platform::trust::Rule5;
use crate::platform::withdrawal::destination;
use crate::platform::{BudgetPurpose, FlowKind};
use crate::platform::{PlatformError, WithdrawAmount};
use crate::{EngineError, NetworkSession, WalletId};

/// The session as the withdrawal's core reads it.
pub(crate) struct SessionEnv {
    session: Arc<NetworkSession>,
    db: Arc<AppDb>,
}

impl SessionEnv {
    pub(crate) fn of(session: &Arc<NetworkSession>) -> Result<Self, WithdrawalError> {
        Ok(Self {
            session: Arc::clone(session),
            db: session.live()?.appdb,
        })
    }
}

impl Env for SessionEnv {
    fn db(&self) -> &Arc<AppDb> {
        &self.db
    }

    fn rule_5(&self) -> Rule5 {
        self.session.trust.rule_5()
    }

    /// The synced header tip (`sync.rs` `tip_height()`, not the chain-locked
    /// height, P4 (a)), only once dash-spv is caught up.
    fn chain(&self) -> Chain {
        #[cfg(test)]
        if let Some(seam) = &*(self.session.masternodes.withdrawals.seam.lock())
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            return Chain {
                header_tip: seam.header_tip,
            };
        }
        let tracker = self.session.hub.tracker();
        Chain {
            header_tip: tracker.caught_up().then(|| tracker.tip_height()).flatten(),
        }
    }

    fn barriers_ready(&self) -> bool {
        self.session.masternodes.barriers_ready()
    }

    async fn barrier_state(
        &self,
        barrier: &PayoutBarrierRow,
    ) -> Result<Option<ProTxOperationState>, WithdrawalError> {
        Ok(self.session.barrier_operation_state(barrier).await?)
    }

    fn waits(&self) -> Waits {
        self.session.masternodes.withdrawals.waits()
    }

    fn now(&self) -> u64 {
        crate::events::unix_now()
    }

    fn alarm(&self, alarm: Alarm) {
        self.session.masternodes.withdrawals.alarm(alarm);
    }
}

/// The wallet's withdrawal keys of one masternode (P2): the keychain owner
/// key by platform-wallet's derive-and-compare and its funding accounts'
/// payout addresses (`masternode_withdrawal_keys`,
/// `rs-platform-wallet/src/wallet/masternode_withdrawal.rs:158-201`, each
/// TRANSFER key's hash160 handed over as its P2PKH script), and the keys
/// attached to the tracked masternode, by the vault's key ids.
pub(crate) struct SessionKeys {
    session: Arc<NetworkSession>,
    wallet: WalletId,
    /// Wire order.
    pro_tx_hash: [u8; 32],
}

impl WalletKeys for SessionKeys {
    async fn held(
        &self,
        owner: Vec<[u8; 20]>,
        payout: Vec<[u8; 20]>,
    ) -> Result<Vec<HeldKey>, WithdrawalError> {
        let manager = self.session.manager()?;
        let wallet = (manager.get_wallet(&self.wallet.0).await)
            .ok_or(EngineError::WalletNotFound(self.wallet.to_string()))?;
        let (vault, hash) = (self.session.vault.clone(), self.pro_tx_hash);
        Ok(tokio::task::spawn_blocking(move || {
            let mut held = Vec::new();
            for h in &owner {
                match wallet.masternode_withdrawal_keys(Some(h), None, None) {
                    Ok(k) => {
                        if let Some((index, _)) = k.owner_key {
                            held.push(HeldKey {
                                key: WithdrawalKey::KeychainOwner { index },
                                hash: *h,
                            });
                        }
                    }
                    Err(e) => tracing::debug!(error = %e, "the keychain owner key"),
                }
            }
            for h in &payout {
                let script = CoreScript::new_p2pkh(*h);
                match wallet.masternode_withdrawal_keys(None, Some(script.as_bytes()), None) {
                    Ok(k) => {
                        if let Some(path) = k.transfer_key {
                            held.push(HeldKey {
                                key: WithdrawalKey::WalletPayout { path },
                                hash: *h,
                            });
                        }
                    }
                    Err(e) => tracing::debug!(error = %e, "the wallet's payout key"),
                }
            }
            // Attached keys need the vault's full-scope key; a locked vault
            // shows none (it could not sign either).
            for (role, hashes) in [("owner", &owner), ("payout", &payout)] {
                match vault.masternode_key_id(&hash, role) {
                    Ok(Some(id)) if hashes.contains(&id) => held.push(HeldKey {
                        key: if role == "owner" {
                            WithdrawalKey::AttachedOwner { pro_tx_hash: hash }
                        } else {
                            WithdrawalKey::AttachedPayout { pro_tx_hash: hash }
                        },
                        hash: id,
                    }),
                    Ok(_) => {}
                    Err(e) => tracing::debug!(error = %e, role, "an attached withdrawal key"),
                }
            }
            held
        })
        .await?)
    }
}

/// The grant, redeemed into a `masternode_withdraw` lease whose Credits
/// budget is charged the amount and the reserve, and its single-use
/// `MasternodeWithdrawal` signer (P3, P12). The charge goes back only when
/// nothing was signed.
pub(crate) struct LeaseGrant {
    pub(super) session: Arc<NetworkSession>,
    pub(super) wallet: WalletId,
    pub(super) grant: String,
}

impl Authorize for LeaseGrant {
    async fn sign(
        self,
        credits: u64,
        key: &WithdrawalKey,
        identity_key: &IdentityPublicKey,
        transition: &StateTransition,
        pro_tx_hash: [u8; 32],
    ) -> Result<[u8; 65], WithdrawalError> {
        let lease = (self.session)
            .begin_lease(self.wallet, FlowKind::MasternodeWithdraw, &[self.grant])
            .await
            .map_err(PlatformError::from)?;
        let signed = async {
            let (charge, signer) =
                (lease.charge_withdrawal_signer(credits)).map_err(PlatformError::from)?;
            let (key, identity_key, transition) =
                (key.clone(), identity_key.clone(), transition.clone());
            let signed = tokio::task::spawn_blocking(move || {
                signer.sign_masternode_withdrawal(&key, &identity_key, &transition, &pro_tx_hash)
            })
            .await?;
            signed.map_err(|e| {
                lease.refund(BudgetPurpose::Credits, charge);
                match e {
                    SignerError::KeyMismatch(_) => WithdrawalError::KeyMissing,
                    e => EngineError::from(e).into(),
                }
            })
        }
        .await;
        lease.end();
        signed
    }
}

/// `withdraw_credits`' arguments as the facade passes them (P13).
#[derive(Debug, Clone)]
pub(crate) struct WithdrawCall {
    pub(crate) pro_tx_hash: String,
    pub(crate) key: WithdrawalKeyChoice,
    pub(crate) amount: WithdrawAmount,
    /// A payout-key withdrawal's address; `None`: the signing key's.
    pub(crate) destination: Option<String>,
}

/// The Platform the session's withdrawal calls read and send to: the SDK's
/// proved reads in their own trust scopes ([`SdkMnWithdrawals`]); in tests,
/// the scripted one the runtime's seam holds.
enum SessionNet {
    Sdk(SdkMnWithdrawals),
    #[cfg(test)]
    Scripted(Arc<super::tests::Platform>),
}

impl MnWithdrawNet for SessionNet {
    fn version(&self) -> &'static PlatformVersion {
        match self {
            Self::Sdk(n) => n.version(),
            #[cfg(test)]
            Self::Scripted(n) => n.version(),
        }
    }

    async fn identity(&self, id: Identifier) -> Result<Proved<Option<Identity>>, PlatformError> {
        match self {
            Self::Sdk(n) => n.identity(id).await,
            #[cfg(test)]
            Self::Scripted(n) => n.identity(id).await,
        }
    }

    async fn nonce_state(&self, id: Identifier) -> Result<Proved<u64>, PlatformError> {
        match self {
            Self::Sdk(n) => n.nonce_state(id).await,
            #[cfg(test)]
            Self::Scripted(n) => n.nonce_state(id).await,
        }
    }

    async fn balance(&self, id: Identifier) -> Result<Proved<Option<u64>>, PlatformError> {
        match self {
            Self::Sdk(n) => n.balance(id).await,
            #[cfg(test)]
            Self::Scripted(n) => n.balance(id).await,
        }
    }

    async fn document(&self, id: Identifier) -> Result<Proved<Option<Document>>, PlatformError> {
        match self {
            Self::Sdk(n) => n.document(id).await,
            #[cfg(test)]
            Self::Scripted(n) => n.document(id).await,
        }
    }

    async fn broadcast(&self, bytes: &[u8]) -> Answer {
        match self {
            Self::Sdk(n) => n.broadcast(bytes).await,
            #[cfg(test)]
            Self::Scripted(n) => n.broadcast(bytes).await,
        }
    }
}

impl NetworkSession {
    /// The Platform the withdrawal calls use;
    /// `masternodes.platform_unavailable` in a session without Platform.
    fn mn_withdraw_net(&self) -> Result<SessionNet, WithdrawalError> {
        #[cfg(test)]
        if let Some(seam) = &*(self.masternodes.withdrawals.seam.lock())
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            return Ok(SessionNet::Scripted(Arc::clone(&seam.platform)));
        }
        if !self.platform.enabled() {
            return Err(MasternodesError::PlatformUnavailable {
                detail: "this session runs without Platform".into(),
            }
            .into());
        }
        Ok(SessionNet::Sdk(SdkMnWithdrawals(self.manager()?.sdk_arc())))
    }

    /// The masternode's keys, refusing a shared masternode (F8, P2).
    async fn withdrawal_keys(
        self: &Arc<Self>,
        wallet: WalletId,
        pro_tx_hash: &str,
    ) -> Result<([u8; 32], SessionKeys), WithdrawalError> {
        let hash = parse_pro_tx_hash(pro_tx_hash)?;
        if crate::masternodes::reads::owner_facts(self, wallet, hash)
            .await?
            .shared
        {
            return Err(WithdrawalError::Shared);
        }
        Ok((
            hash,
            SessionKeys {
                session: Arc::clone(self),
                wallet,
                pro_tx_hash: hash,
            },
        ))
    }

    /// `withdrawal_quote` (P13), waiting up to `wait` while only Platform is
    /// behind (P4); the facade reads once and leaves the wait to
    /// [`Self::mn_withdrawal_ready_wait`].
    pub(crate) async fn mn_withdrawal_quote(
        self: &Arc<Self>,
        wallet: WalletId,
        pro_tx_hash: &str,
        wait: std::time::Duration,
    ) -> Result<Quote, WithdrawalError> {
        let net = &self.mn_withdraw_net()?;
        let (hash, keys) = self.withdrawal_keys(wallet, pro_tx_hash).await?;
        let env = SessionEnv::of(self)?;
        quote(&env, net, &keys, self.network.core_network(), &hash, wait).await
    }

    fn request(
        &self,
        wallet: WalletId,
        hash: [u8; 32],
        call: &WithdrawCall,
    ) -> Result<Request, WithdrawalError> {
        let network: Network = self.network.core_network();
        let destination = match &call.destination {
            Some(to) => Some(destination(network, to)?),
            None => None,
        };
        Ok(Request {
            pro_tx_hash: hash,
            key: call.key,
            amount: call.amount,
            destination,
            created_by: wallet.to_string(),
        })
    }

    /// `withdraw_credits` (P13) under `grant` (a `MasternodeWithdraw` grant).
    pub(crate) async fn mn_withdraw(
        self: &Arc<Self>,
        wallet: WalletId,
        call: &WithdrawCall,
        grant: &str,
    ) -> Result<WithdrawalRow, WithdrawalError> {
        let net = &self.mn_withdraw_net()?;
        let (hash, keys) = self.withdrawal_keys(wallet, &call.pro_tx_hash).await?;
        let req = self.request(wallet, hash, call)?;
        let env = SessionEnv::of(self)?;
        let auth = LeaseGrant {
            session: Arc::clone(self),
            wallet,
            grant: grant.to_owned(),
        };
        withdraw(&env, net, &keys, auth, self.network.core_network(), &req).await
    }

    /// `retry_withdrawal` (P13, P15).
    pub(crate) async fn mn_retry_withdrawal(
        self: &Arc<Self>,
        wallet: WalletId,
        refused_id: &str,
        call: &WithdrawCall,
        grant: &str,
    ) -> Result<WithdrawalRow, WithdrawalError> {
        let net = &self.mn_withdraw_net()?;
        let (hash, keys) = self.withdrawal_keys(wallet, &call.pro_tx_hash).await?;
        let req = self.request(wallet, hash, call)?;
        let env = SessionEnv::of(self)?;
        let auth = LeaseGrant {
            session: Arc::clone(self),
            wallet,
            grant: grant.to_owned(),
        };
        retry(
            &env,
            net,
            &keys,
            auth,
            self.network.core_network(),
            refused_id,
            &req,
        )
        .await
    }

    /// `recheck_withdrawal` (P13).
    pub(crate) async fn mn_recheck_withdrawal(
        self: &Arc<Self>,
        id: &str,
    ) -> Result<WithdrawalRow, WithdrawalError> {
        let net = self.mn_withdraw_net()?;
        recheck_record(&SessionEnv::of(self)?, &net, id).await
    }

    /// `mark_withdrawal` (P9, P13).
    pub(crate) async fn mn_mark_withdrawal(
        self: &Arc<Self>,
        id: &str,
        to: WithdrawalMark,
    ) -> Result<WithdrawalRow, WithdrawalError> {
        mark(&SessionEnv::of(self)?, id, to).await
    }

    /// The withdrawal record `id`.
    pub(crate) async fn mn_withdrawal(
        self: &Arc<Self>,
        id: &str,
    ) -> Result<WithdrawalRow, WithdrawalError> {
        let (db, id) = (SessionEnv::of(self)?.db, id.to_owned());
        (tokio::task::spawn_blocking(move || db.withdrawal(&id)).await??)
            .ok_or(WithdrawalError::NotFound)
    }

    /// P4's `withdrawal_ready`: after a quote whose destination read
    /// `checking{platform_behind}`, the look again every `Waits::poll` for at
    /// most `Waits::quote` (240 s) while only (a) fails, and §6's event once
    /// (a)–(c) hold, so the host reads the quote again. Before consent: no
    /// grant waits on it. One per masternode; the close stops it.
    pub(crate) fn mn_withdrawal_ready_wait(self: &Arc<Self>, pro_tx_hash: [u8; 32]) {
        let (weak, rt) = (Arc::downgrade(self), self.rt.clone());
        self.masternodes
            .withdrawals
            .wait_ready(pro_tx_hash, move || {
                rt.spawn(async move {
                    let Some(s) = weak.upgrade() else { return };
                    let (Ok(env), Ok(net)) = (SessionEnv::of(&s), s.mn_withdraw_net()) else {
                        return;
                    };
                    let wait = env.waits().quote;
                    match look_until(&env, &net, &pro_tx_hash, wait).await {
                        Ok(seen) if seen.reason.is_none() => s.masternodes.local_change(),
                        Ok(seen) => tracing::debug!(reason = ?seen.reason,
                            "a masternode withdrawal's destination still checking"),
                        Err(e) => tracing::debug!(error = %e,
                            "waiting for Platform to catch up (MG-13 P4)"),
                    }
                })
            });
    }

    /// `withdrawals(pro_tx_hash)` (P13), oldest first.
    pub(crate) async fn mn_withdrawals(
        self: &Arc<Self>,
        pro_tx_hash: &str,
    ) -> Result<Vec<WithdrawalRow>, WithdrawalError> {
        let shown = display_hex(&parse_pro_tx_hash(pro_tx_hash)?);
        let db = SessionEnv::of(self)?.db;
        Ok(tokio::task::spawn_blocking(move || db.withdrawals_of_masternode(&shown)).await??)
    }
}

impl WithdrawalRuntime {
    /// Starts the background recheck (P8): every undecided group at once,
    /// then every `Waits::recheck` while the session lives; nothing to do
    /// when nothing is undecided. Not with Platform off. [`Self::stop`] at
    /// close aborts it and waits for it.
    pub(crate) fn start(&self, session: &Arc<NetworkSession>) {
        if !session.platform.enabled() {
            return;
        }
        let weak = Arc::downgrade(session);
        let task = session.rt.spawn(async move {
            loop {
                let Some(s) = weak.upgrade() else { return };
                let every = s.masternodes.withdrawals.waits().recheck;
                if let (Ok(env), Ok(net)) = (SessionEnv::of(&s), s.mn_withdraw_net())
                    && let Err(e) = recheck_all(&env, &net).await
                {
                    tracing::info!(error = %e, "the masternode withdrawals' recheck");
                }
                drop(s);
                tokio::time::sleep(every).await;
            }
        });
        self.set_task(task);
    }
}
