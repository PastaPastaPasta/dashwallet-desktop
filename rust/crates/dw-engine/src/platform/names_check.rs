//! Whether an identity that shows no main name owns one (DP1-08 D23, m4
//! item 51): the engine asks Platform per identity, and only an empty
//! answer makes it nameless.
//!
//! The checks run in the background, never in the Platform supervisor (D27):
//! one at a time per wallet, a trigger while one runs makes one more follow,
//! the identities are asked [`CONCURRENCY`] at a time within one
//! [`CHECK_BUDGET`], and the minute's retry has its own task. Each check
//! asks first the identities whose last question ended longest ago, or never
//! did, so a slow few cannot starve the rest (D31). A supervisor's stop ends
//! its checks and its retry, and none starts again until the next start
//! (D32). An answer applies only to the wallet generation it was asked for
//! (D28): a removal drops the answers in flight.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dpp::prelude::Identifier;
use futures_util::{StreamExt, stream};
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
/// How long the questions of one check may take: an identity still
/// unanswered then is unknown (`Failed`), never nameless.
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
    /// Identity (Base58) → when its last question ended on its own (it was
    /// answered, failed or ran out of [`QUESTION_BUDGET`]), on [`Self::clock`]
    /// (D31). Not one cut off by [`CHECK_BUDGET`].
    ended: HashMap<WalletId, HashMap<String, u64>>,
    clock: u64,
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

    /// A check of `id` begins: what its answers apply to, or `None` while
    /// the wallet waits for a names pass, which starts one when it ends.
    pub(super) fn begin(&self, id: &WalletId) -> Option<Asked> {
        let checks = guard(&self.0);
        let owed = checks.owed.get(id).copied();
        (owed != Some(false)).then(|| Asked {
            generation: checks.generation(id),
            settles: owed == Some(true),
        })
    }

    pub(super) fn generation(&self, id: &WalletId) -> u64 {
        guard(&self.0).generation(id)
    }

    /// Orders `asked`, `id`'s identities to ask, as a check asks them: those
    /// whose last question never ended first, then the longest ago (D31).
    /// Ties keep the library's order.
    fn order<T>(&self, id: &WalletId, asked: &mut [(String, T)]) {
        let checks = guard(&self.0);
        let ended = checks.ended.get(id);
        asked.sort_by_key(|(identity, _)| ended.and_then(|e| e.get(identity)).copied());
    }

    /// Whether Platform has said `identity` owns no name.
    fn is_nameless(&self, id: &WalletId, identity: &str) -> bool {
        guard(&self.0).answers.get(id).and_then(|a| a.get(identity)) == Some(&true)
    }

    /// Records Platform's `answers` for `id`'s identities, asked as
    /// `asked` says, and ends the pass owed if the check began after it
    /// ended. Each answer is `Some(nameless)` when its question ended on its
    /// own (`Some(false)` too when it failed or ran out of time), `None` when
    /// the check's budget cut it off or never reached it: unknown either way
    /// unless nameless. Nothing for an earlier generation (D28). Whether a
    /// check changed.
    pub(super) fn answered(
        &self,
        id: WalletId,
        asked: Asked,
        answers: Vec<(String, Option<bool>)>,
    ) -> bool {
        let mut checks = guard(&self.0);
        if checks.generation(&id) != asked.generation {
            return false;
        }
        let mut changed = false;
        // Not by a check asked before the pass ended: it starts another.
        if asked.settles && checks.owed.get(&id) == Some(&true) {
            checks.owed.remove(&id);
            changed = true;
        }
        for (identity, answer) in answers {
            if answer.is_some() {
                checks.clock += 1;
                let at = checks.clock;
                checks
                    .ended
                    .entry(id)
                    .or_default()
                    .insert(identity.clone(), at);
            }
            let nameless = answer == Some(true);
            let known = checks.answers.entry(id).or_default();
            changed |= known.insert(identity, nameless) != Some(nameless);
        }
        changed
    }

    /// Forgets a removed or unloaded wallet, and every answer in flight.
    pub(super) fn forget(&self, id: &WalletId) {
        let mut checks = guard(&self.0);
        checks.owed.remove(id);
        checks.answers.remove(id);
        checks.ended.remove(id);
        *checks.generations.entry(*id).or_default() += 1;
    }
}

/// What a check's answers apply to: the wallet generation it asked for, and
/// whether it began after the names pass owed ended.
#[derive(Debug, Clone, Copy)]
pub(super) struct Asked {
    generation: u64,
    settles: bool,
}

impl Checks {
    fn generation(&self, id: &WalletId) -> u64 {
        self.generations.get(id).copied().unwrap_or_default()
    }
}

/// The checks running in the background, and the minute's retry. They
/// start only while a supervisor runs, and only from it (D32): its start
/// arms them with an epoch its kicks carry, and its stop disarms them and
/// moves the epoch on, so a kick of a stopped supervisor (its retry past
/// the abort, before its next yield) starts nothing. None starts once
/// closed.
#[derive(Default)]
pub(super) struct NamesRuns(Mutex<Runs>);

#[derive(Default)]
struct Runs {
    closed: bool,
    active: bool,
    epoch: u64,
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
    /// A supervisor starts: checks may start, kicked with the epoch this
    /// returns.
    pub(super) fn arm(&self) -> u64 {
        let mut runs = guard(&self.0);
        runs.active = !runs.closed;
        runs.epoch
    }

    /// The epoch of the supervisor running, for a test's kicks.
    #[cfg(test)]
    pub(super) fn epoch(&self) -> u64 {
        guard(&self.0).epoch
    }

    /// Starts `run(token)` for `id` on `rt`, unless one of `id`'s runs:
    /// then that one runs once more when it ends, however many triggers
    /// came. Nothing unless the supervisor of `epoch` runs.
    fn start<F>(&self, rt: &Handle, id: WalletId, epoch: u64, run: impl FnOnce(u64) -> F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut runs = guard(&self.0);
        if !runs.active || runs.epoch != epoch {
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

    fn set_retry(&self, retry: AbortHandle, epoch: u64) {
        let mut runs = guard(&self.0);
        if !runs.active || runs.epoch != epoch {
            retry.abort();
        } else if let Some(old) = runs.retry.replace(retry) {
            old.abort();
        }
    }

    /// The checks alive, for a test.
    #[cfg(test)]
    pub(super) fn alive(&self) -> usize {
        let mut runs = guard(&self.0);
        while runs.set.try_join_next().is_some() {}
        runs.set.len()
    }

    /// Stops the retry and every check: the supervisor stopped, and its
    /// start kicks them again.
    pub(super) fn stop(&self) {
        let mut runs = guard(&self.0);
        runs.active = false;
        runs.epoch += 1;
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
            runs.active = false;
            runs.epoch += 1;
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
    /// the check that runs. `epoch`: the kicking supervisor's (D32).
    pub(super) fn kick_names_check(self: &Arc<Self>, id: WalletId, epoch: u64) {
        let session = Arc::downgrade(self);
        let runs = &self.platform.recovery.names_runs;
        runs.start(&self.rt, id, epoch, move |token| async move {
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
    /// supervisor pass delays it (D27), until the supervisor of `epoch`
    /// stops.
    pub(super) fn start_names_retry(self: &Arc<Self>, epoch: u64) {
        let session = Arc::downgrade(self);
        #[cfg(not(test))]
        let period = RETRY_EVERY;
        #[cfg(test)]
        let period = (*guard(&self.platform.recovery.retry_every)).unwrap_or(RETRY_EVERY);
        let retry = self.rt.spawn(async move {
            let first = tokio::time::Instant::now() + period;
            let mut every = tokio::time::interval_at(first, period);
            loop {
                every.tick().await;
                let Some(this) = session.upgrade() else {
                    return;
                };
                let Ok(manager) = this.manager() else {
                    return;
                };
                for (n, id) in manager.wallet_ids().await.into_iter().enumerate() {
                    this.between_kicks(n);
                    this.kick_names_check(WalletId(id), epoch);
                }
            }
        });
        (self.platform.recovery.names_runs).set_retry(retry.abort_handle(), epoch);
    }

    #[cfg(not(test))]
    fn between_kicks(&self, _: usize) {}

    /// Tests: holds the retry before its second kick, blocking, so the abort
    /// of a stop cannot land in between (no yield).
    #[cfg(test)]
    fn between_kicks(&self, n: usize) {
        if n == 1
            && let Some((reached, release)) =
                guard(&self.platform.recovery.pause_between_kicks).take()
        {
            let _ = reached.send(());
            let _ = release.recv();
        }
    }

    /// Asks Platform whether `id`'s identities that show no main name own
    /// one, unless the wallet waits for a names pass: that pass asks when it
    /// ends.
    pub(super) async fn check_names(self: &Arc<Self>, manager: &Arc<Manager>, id: WalletId) {
        let checks = &self.platform.recovery.names_checks;
        let Some(asked) = checks.begin(&id) else {
            return;
        };
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
        if checks.answered(id, asked, answers) {
            self.hub.pump.mark_platform(id, [PlatformChange::Names]);
        }
    }

    /// Platform's answer for each identity of `id` that shows no main name
    /// (as `identities()` would: a refused or contested label is none) and
    /// that it has not said is nameless, in [`NamesChecks::order`]'s order:
    /// `Some(true)` when it owns no DPNS name, `Some(false)` when it owns
    /// one or its question failed, `None` when [`CHECK_BUDGET`] ran out
    /// before its question ended. Nothing when the main-name rows cannot be
    /// read: the next check asks.
    async fn ask_names(&self, manager: &Manager, id: WalletId) -> Vec<(String, Option<bool>)> {
        let checks = &self.platform.recovery.names_checks;
        let prefs = match self.appdb_op(move |db| read_name_prefs(db, id)).await {
            Ok(prefs) => prefs,
            Err(e) => {
                tracing::warn!(wallet_id = %id, error = %e, "could not read the main-name rows");
                return Vec::new();
            }
        };
        let mut asked: Vec<(String, Identifier)> = {
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
        checks.order(&id, &mut asked);
        #[cfg(not(test))]
        let budget = CHECK_BUDGET;
        #[cfg(test)]
        let budget = (*guard(&self.platform.recovery.check_budget)).unwrap_or(CHECK_BUDGET);
        // In this task, so an abort of the check drops every question.
        let mut answers = Vec::with_capacity(asked.len());
        let questions = stream::iter(asked.iter().cloned())
            .map(|(identity, owner)| async move {
                let owns = self.owns_a_name(manager, id, &identity, &owner).await;
                (identity, Some(owns == Some(false)))
            })
            .buffer_unordered(CONCURRENCY)
            .for_each(|answer| {
                answers.push(answer);
                std::future::ready(())
            });
        if tokio::time::timeout(budget, questions).await.is_err() {
            tracing::warn!(wallet_id = %id, "the names check ran out of time");
        }
        let answered: HashSet<String> = answers.iter().map(|(i, _)| i.clone()).collect();
        for (identity, _) in asked {
            if !answered.contains(&identity) {
                answers.push((identity, None));
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
