//! Whether an identity that shows no main name owns one (DP1-08 D23, m4
//! item 51): the engine asks Platform per identity, and only an empty
//! answer makes it nameless.
//!
//! The checks run in the background, never in the Platform supervisor (D27):
//! one at a time per wallet, a trigger while one runs makes one more follow,
//! the identities are asked [`CONCURRENCY`] at a time within one
//! [`CHECK_BUDGET`], and the minute's retry has its own task. An answer
//! applies only to the wallet generation it was asked for (D28): a removal
//! drops the answers in flight.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dpp::prelude::Identifier;
use tokio::runtime::Handle;
use tokio::task::{AbortHandle, JoinSet};

use super::identity::NamesCheck;
use super::names::resolve_main_name;
use super::recovery::{evident, owned_identities, read_name_prefs};
use super::runtime::guard;
use crate::session::Manager;
use crate::{NetworkSession, PlatformChange, WalletId};

/// How long Platform has to say whether one identity owns a name.
const QUESTION_BUDGET: Duration = Duration::from_secs(10);
/// How long one check of a wallet may take: an identity still unanswered
/// then is unknown (`Failed`), never nameless.
const CHECK_BUDGET: Duration = Duration::from_secs(20);
/// The questions of one check in flight at once.
const CONCURRENCY: usize = 4;
/// How often every wallet is checked again (what is answered nameless is
/// not asked again).
const RETRY_EVERY: Duration = Duration::from_secs(60);

/// Per wallet, what Platform answered for each of its identities that show
/// no main name: only an identity Platform said owns no DPNS name is
/// nameless. Until it has answered, a name may be on its way: a restore or
/// a discovery found the identity and the names pass that fetches its names
/// has not ended, or a pass was cut off. Answers live for the session; an
/// identity with none is pending. A name registered elsewhere after a
/// "nameless" answer shows once the DPNS loop loads it.
#[derive(Default)]
pub(super) struct NamesChecks(Mutex<Checks>);

#[derive(Default)]
struct Checks {
    /// Wallets a restore or a discovery found identities of: `false` until
    /// their names pass ends, then `true` until the check after it settles
    /// them. Meanwhile no answer says "unknown".
    owed: HashMap<WalletId, bool>,
    /// Identity (Base58) → `true` when Platform said it owns no name,
    /// `false` when it did not say so (it owns one we have not loaded, or
    /// the question failed or ran out of time).
    answers: HashMap<WalletId, HashMap<String, bool>>,
    /// Raised by each removal or unload (D28): an answer asked for an
    /// earlier generation is dropped.
    generations: HashMap<WalletId, u64>,
}

impl NamesChecks {
    /// The check of `identity`, which shows no main name.
    pub(super) fn of(&self, id: &WalletId, identity: &str) -> Option<NamesCheck> {
        let checks = guard(&self.0);
        match checks.answers.get(id).and_then(|a| a.get(identity)) {
            Some(true) => None,
            Some(false) if !checks.owed.contains_key(id) => Some(NamesCheck::Failed),
            _ => Some(NamesCheck::Pending),
        }
    }

    /// A restore or a discovery found identities of `id`: a names pass
    /// follows.
    pub(super) fn owe(&self, id: WalletId) {
        guard(&self.0).owed.insert(id, false);
    }

    /// The names pass `id` owed ended, if `generation` is still its own: the
    /// next check settles it.
    pub(super) fn pass_ended(&self, id: WalletId, generation: u64) {
        let mut checks = guard(&self.0);
        if checks.generation(&id) == generation
            && let Some(ended) = checks.owed.get_mut(&id)
        {
            *ended = true;
        }
    }

    /// Whether `id` waits for a names pass, which asks when it ends.
    fn awaits_pass(&self, id: &WalletId) -> bool {
        guard(&self.0).owed.get(id) == Some(&false)
    }

    pub(super) fn generation(&self, id: &WalletId) -> u64 {
        guard(&self.0).generation(id)
    }

    /// Whether Platform has said `identity` owns no name.
    fn is_nameless(&self, id: &WalletId, identity: &str) -> bool {
        guard(&self.0).answers.get(id).and_then(|a| a.get(identity)) == Some(&true)
    }

    /// Records Platform's `answers` for `id`'s identities, asked for
    /// `generation`, and ends the pass owed if it has ended. Nothing for an
    /// earlier generation (D28). Whether a check changed.
    pub(super) fn answered(
        &self,
        id: WalletId,
        generation: u64,
        answers: Vec<(String, bool)>,
    ) -> bool {
        let mut checks = guard(&self.0);
        if checks.generation(&id) != generation {
            return false;
        }
        let mut changed = false;
        if checks.owed.get(&id) == Some(&true) {
            checks.owed.remove(&id);
            changed = true;
        }
        let known = checks.answers.entry(id).or_default();
        for (identity, nameless) in answers {
            changed |= known.insert(identity, nameless) != Some(nameless);
        }
        changed
    }

    /// Forgets a removed or unloaded wallet, and every answer in flight.
    pub(super) fn forget(&self, id: &WalletId) {
        let mut checks = guard(&self.0);
        checks.owed.remove(id);
        checks.answers.remove(id);
        *checks.generations.entry(*id).or_default() += 1;
    }
}

impl Checks {
    fn generation(&self, id: &WalletId) -> u64 {
        self.generations.get(id).copied().unwrap_or_default()
    }
}

/// The checks running in the background, and the minute's retry. None
/// starts once closed.
#[derive(Default)]
pub(super) struct NamesRuns(Mutex<Runs>);

#[derive(Default)]
struct Runs {
    closed: bool,
    set: JoinSet<()>,
    running: HashMap<WalletId, Run>,
    tokens: u64,
    retry: Option<AbortHandle>,
}

/// A wallet's check in flight: its token, and whether a trigger came while
/// it ran.
struct Run {
    token: u64,
    again: bool,
    abort: AbortHandle,
}

impl NamesRuns {
    /// Starts `run(token)` for `id` on `rt`, unless `id`'s runs: then that
    /// one runs once more when it ends, however many triggers came.
    fn start<F>(&self, rt: &Handle, id: WalletId, run: impl FnOnce(u64) -> F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut runs = guard(&self.0);
        if runs.closed {
            return;
        }
        while runs.set.try_join_next().is_some() {}
        // One that ended without saying so (it panicked) runs no more.
        if let Some(running) = runs.running.get_mut(&id)
            && !running.abort.is_finished()
        {
            running.again = true;
            return;
        }
        runs.tokens += 1;
        let token = runs.tokens;
        let abort = runs.set.spawn_on(run(token), rt);
        (runs.running).insert(
            id,
            Run {
                token,
                again: false,
                abort,
            },
        );
    }

    /// Run `token` of `id` ended: whether it runs again.
    fn again(&self, id: WalletId, token: u64) -> bool {
        let mut runs = guard(&self.0);
        let Some(run) = runs.running.get_mut(&id).filter(|r| r.token == token) else {
            return false;
        };
        if run.again {
            run.again = false;
            return true;
        }
        runs.running.remove(&id);
        false
    }

    fn set_retry(&self, retry: AbortHandle) {
        if let Some(old) = guard(&self.0).retry.replace(retry) {
            old.abort();
        }
    }

    /// Stops the retry and every check: the supervisor stopped, and its
    /// start kicks them again.
    pub(super) fn stop(&self) {
        let mut runs = guard(&self.0);
        if let Some(retry) = runs.retry.take() {
            retry.abort();
        }
        runs.running.clear();
        runs.set.abort_all();
    }

    /// Aborts `id`'s check. One past its last yield cannot apply its answers
    /// ([`NamesChecks::forget`]).
    pub(super) fn forget(&self, id: &WalletId) {
        if let Some(run) = guard(&self.0).running.remove(id) {
            run.abort.abort();
        }
    }

    /// Aborts every check and waits for them, and starts none after: the
    /// session's close, before the wallets unload.
    pub(super) async fn close(&self) {
        let mut set = {
            let mut runs = guard(&self.0);
            runs.closed = true;
            if let Some(retry) = runs.retry.take() {
                retry.abort();
            }
            runs.running.clear();
            std::mem::take(&mut runs.set)
        };
        set.abort_all();
        while set.join_next().await.is_some() {}
    }
}

impl NetworkSession {
    /// Checks `id`'s names in the background (D27): now, or once more after
    /// the check that runs.
    pub(super) fn kick_names_check(self: &Arc<Self>, id: WalletId) {
        let session = Arc::downgrade(self);
        let runs = &self.platform.recovery.names_runs;
        runs.start(&self.rt, id, move |token| async move {
            loop {
                let Some(this) = session.upgrade() else {
                    return;
                };
                if let Ok(manager) = this.manager() {
                    this.check_names(&manager, id).await;
                }
                if !this.platform.recovery.names_runs.again(id, token) {
                    return;
                }
            }
        });
    }

    /// Checks every wallet's names every minute, on a task of its own so no
    /// supervisor pass delays it (D27), until the supervisor stops.
    pub(super) fn start_names_retry(self: &Arc<Self>) {
        let session = Arc::downgrade(self);
        let retry = self.rt.spawn(async move {
            let first = tokio::time::Instant::now() + RETRY_EVERY;
            let mut every = tokio::time::interval_at(first, RETRY_EVERY);
            loop {
                every.tick().await;
                let Some(this) = session.upgrade() else {
                    return;
                };
                let Ok(manager) = this.manager() else {
                    return;
                };
                for id in manager.wallet_ids().await {
                    this.kick_names_check(WalletId(id));
                }
            }
        });
        (self.platform.recovery.names_runs).set_retry(retry.abort_handle());
    }

    /// Asks Platform whether `id`'s identities that show no main name own
    /// one, unless the wallet waits for a names pass: that pass asks when it
    /// ends.
    pub(super) async fn check_names(self: &Arc<Self>, manager: &Arc<Manager>, id: WalletId) {
        let checks = &self.platform.recovery.names_checks;
        if checks.awaits_pass(&id) {
            return;
        }
        let generation = checks.generation(&id);
        let answers = self.ask_names(manager, id).await;
        #[cfg(test)]
        {
            let pause = guard(&self.platform.recovery.pause_before_answer).take();
            if let Some((reached, release)) = pause {
                // Blocking: an abort cannot land here, as past a check's
                // last yield.
                let _ = reached.send(());
                let _ = release.recv();
            }
        }
        if checks.answered(id, generation, answers) {
            self.hub.pump.mark_platform(id, [PlatformChange::Names]);
        }
    }

    /// Platform's answer for each identity of `id` that shows no main name
    /// (as `identities()` would: a refused or contested label is none) and
    /// that it has not said is nameless: `true` when it owns no DPNS name.
    /// Within [`CHECK_BUDGET`]; one unanswered by then is `false`. Nothing
    /// when the main-name rows cannot be read: the next check asks.
    async fn ask_names(
        self: &Arc<Self>,
        manager: &Arc<Manager>,
        id: WalletId,
    ) -> Vec<(String, bool)> {
        let checks = &self.platform.recovery.names_checks;
        let prefs = match self.appdb_op(move |db| read_name_prefs(db, id)).await {
            Ok(prefs) => prefs,
            Err(e) => {
                tracing::warn!(wallet_id = %id, error = %e, "could not read the main-name rows");
                return Vec::new();
            }
        };
        let asked: Vec<(String, Identifier)> = {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let Some(info) = wm.get_wallet_info(&id.0) else {
                return Vec::new();
            };
            (owned_identities(info, &id).into_iter())
                .filter(|o| {
                    let prefs = prefs.get(&o.identity).cloned().unwrap_or_default();
                    resolve_main_name(&evident(o, &prefs), &o.open_contests, &prefs).is_none()
                        && !checks.is_nameless(&id, &o.identity)
                })
                .map(|o| (o.identity, o.owner))
                .collect()
        };
        let mut answers = Vec::with_capacity(asked.len());
        let mut unasked = asked.clone().into_iter();
        let mut questions = JoinSet::new();
        let asking = async {
            loop {
                while questions.len() < CONCURRENCY
                    && let Some((identity, owner)) = unasked.next()
                {
                    let (this, manager) = (Arc::clone(self), Arc::clone(manager));
                    questions.spawn(async move {
                        let owns = this.owns_a_name(&manager, id, &identity, &owner).await;
                        (identity, owns == Some(false))
                    });
                }
                match questions.join_next().await {
                    Some(Ok(answer)) => answers.push(answer),
                    Some(Err(e)) => {
                        tracing::warn!(wallet_id = %id, error = %e, "a names question failed")
                    }
                    None => break,
                }
            }
        };
        if tokio::time::timeout(CHECK_BUDGET, asking).await.is_err() {
            tracing::warn!(wallet_id = %id, "the names check ran out of time");
        }
        questions.abort_all();
        let answered: HashSet<String> = answers.iter().map(|(i, _)| i.clone()).collect();
        for (identity, _) in asked {
            if !answered.contains(&identity) {
                answers.push((identity, false));
            }
        }
        answers
    }

    /// Whether `owner` owns a DPNS name, from one read-only query within
    /// [`QUESTION_BUDGET`]; `None` when Platform did not answer.
    async fn owns_a_name(
        &self,
        manager: &Manager,
        id: WalletId,
        identity: &str,
        owner: &Identifier,
    ) -> Option<bool> {
        #[cfg(test)]
        if let Some(mock) = self.platform.recovery.mock() {
            return mock.owns_a_name(id, identity.to_owned()).await;
        }
        let wallet = manager.get_wallet(&id.0).await?;
        let query = wallet
            .identity()
            .dpns_domain_states_for_identity(owner, Some(1));
        match tokio::time::timeout(QUESTION_BUDGET, query).await {
            Ok(Ok(states)) => Some(!states.is_empty()),
            Ok(Err(e)) => {
                tracing::warn!(wallet_id = %id, identity, error = %e, "could not ask for the names");
                None
            }
            Err(_) => {
                tracing::warn!(wallet_id = %id, identity, "asking for the names ran out of time");
                None
            }
        }
    }
}
