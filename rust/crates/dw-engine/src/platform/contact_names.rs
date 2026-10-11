//! Contact usernames for the contacts read model (DP2-01). platform-wallet
//! keeps contacts' profiles but not their DPNS names, so the session keeps
//! them: a read that shows a contact whose name is not known (or is stale)
//! queues a background fetch, and the fetch signals `Platform{Contacts}` for
//! the identities that asked once a name lands or changes. Reads never wait.
//! In memory only: a cold start shows display names and aliases until the
//! first fetch answers. Each name as it lands is reported to provenance
//! (`Provenance::show_label`), whose re-fetch verifies the names shown.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dpp::prelude::Identifier;

use super::errors::PlatformError;
use super::runtime::guard;
use crate::{NetworkSession, PlatformChange, WalletId};

/// How long a fetched name is shown before it is asked for again.
const FRESH: Duration = Duration::from_secs(3600);
/// How long after a failed fetch the next read retries.
const RETRY: Duration = Duration::from_secs(60);
/// The names of one identity a fetch reads (DPNS `records.identity`).
const NAMES_PER_IDENTITY: u32 = 10;
/// How long one fetch may take before it counts as failed.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) type BoxedFuture<'a, T> = std::pin::Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Where usernames come from: the SDK, or a test's script.
pub(super) trait NameSource: Send + Sync {
    /// The username `identity` shows, `None` if it holds none.
    fn username(
        &self,
        identity: Identifier,
    ) -> BoxedFuture<'_, Result<Option<String>, PlatformError>>;
}

/// DPNS through the SDK: of the first `NAMES_PER_IDENTITY` domains Platform
/// returns for the identity, those whose `records.identity` and owner are
/// the identity, the first by normalized label (Platform has no main name;
/// the contact's own pick is local to their wallet).
struct SdkNames(Arc<dash_sdk::Sdk>);

impl NameSource for SdkNames {
    fn username(
        &self,
        identity: Identifier,
    ) -> BoxedFuture<'_, Result<Option<String>, PlatformError>> {
        Box::pin(async move {
            let names = (self.0)
                .get_dpns_usernames_by_identity(identity, Some(NAMES_PER_IDENTITY))
                .await?;
            Ok(names
                .into_iter()
                .filter(|n| n.owner_id == identity && n.records_identity_id == Some(identity))
                .min_by(|a, b| a.normalized_label.cmp(&b.normalized_label))
                .map(|n| n.label))
        })
    }
}

#[derive(Debug, Default)]
struct NameState {
    label: Option<String>,
    /// When the last fetch answered, and whether it failed.
    checked: Option<(Instant, bool)>,
    in_flight: bool,
    /// The identities whose reads showed this contact since the fetch was
    /// queued: each gets a `Contacts` signal when it lands.
    waiters: BTreeSet<(WalletId, String)>,
}

impl NameState {
    fn due(&self, now: Instant) -> bool {
        !self.in_flight
            && self.checked.is_none_or(|(at, failed)| {
                now.duration_since(at) >= if failed { RETRY } else { FRESH }
            })
    }
}

/// The session's contact usernames. Clones share them.
#[derive(Default, Clone)]
pub(crate) struct ContactNames {
    states: Arc<Mutex<HashMap<Identifier, NameState>>>,
    /// Tests: replaces the SDK.
    #[cfg(test)]
    pub(super) source: Arc<Mutex<Option<Arc<dyn NameSource>>>>,
}

impl ContactNames {
    /// The last name fetched for `identity`.
    pub(super) fn label(&self, identity: &Identifier) -> Option<String> {
        guard(&self.states).get(identity)?.label.clone()
    }

    /// Marks the due ones of `ids` in flight and records `waiter` on each
    /// one still to land; the ones to fetch now.
    fn claim(
        &self,
        ids: impl IntoIterator<Item = Identifier>,
        waiter: (WalletId, String),
    ) -> Vec<Identifier> {
        let now = Instant::now();
        let mut states = guard(&self.states);
        let mut claimed = Vec::new();
        for id in ids {
            let state = states.entry(id).or_default();
            if state.due(now) {
                state.in_flight = true;
                claimed.push(id);
            }
            if state.in_flight {
                state.waiters.insert(waiter.clone());
            }
        }
        claimed
    }

    /// Tests: whether a fetch for `identity` is still owed.
    #[cfg(test)]
    pub(super) fn in_flight(&self, identity: &Identifier) -> bool {
        guard(&self.states)
            .get(identity)
            .is_some_and(|s| s.in_flight)
    }

    /// Tests: makes `identity`'s name due again.
    #[cfg(test)]
    pub(super) fn expire(&self, identity: &Identifier) {
        if let Some(state) = guard(&self.states).get_mut(identity) {
            state.checked = None;
        }
    }

    /// Drops the waiters of a wallet the session forgets.
    pub(super) fn forget(&self, wallet: &WalletId) {
        for state in guard(&self.states).values_mut() {
            state.waiters.retain(|(w, _)| w != wallet);
        }
    }

    /// Stores a fetch's answer; the identities to signal (none when nothing
    /// shown changed). A changed name is published while the names are
    /// locked, so publications follow the landings and the last one is the
    /// name shown (Sol r3 R3-01). A failure keeps the name and publishes
    /// nothing.
    fn land(
        &self,
        id: Identifier,
        answer: Result<Option<String>, PlatformError>,
        publish: Publish<'_>,
    ) -> BTreeSet<(WalletId, String)> {
        let mut states = guard(&self.states);
        let state = states.entry(id).or_default();
        state.in_flight = false;
        let waiters = std::mem::take(&mut state.waiters);
        match answer {
            Ok(label) => {
                state.checked = Some((Instant::now(), false));
                if state.label != label {
                    state.label = label;
                    publish(id, state.label.as_deref());
                    return waiters;
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "could not fetch a contact's username");
                state.checked = Some((Instant::now(), true));
            }
        }
        BTreeSet::new()
    }

    /// Lands a fetch that never answered: the name stays, nothing to
    /// publish.
    fn fail(&self, id: Identifier) {
        self.land(id, Err(PlatformError::Unavailable), &|_, _| {});
    }
}

/// Where a landing publishes a changed name: `Provenance::show_label`.
type Publish<'a> = &'a dyn Fn(Identifier, Option<&str>);

/// The names a fetch task still owes an answer. Whatever it has not landed
/// when it ends (session close, a panic, the runtime shutting down) lands
/// as failed, so no name stays in flight.
struct Owed {
    names: ContactNames,
    ids: VecDeque<Identifier>,
}

impl Drop for Owed {
    fn drop(&mut self) {
        for id in self.ids.drain(..) {
            self.names.fail(id);
        }
    }
}

impl NetworkSession {
    fn name_source(&self) -> Option<Arc<dyn NameSource>> {
        #[cfg(test)]
        if let Some(source) = guard(&self.platform.contacts.names.source).clone() {
            return Some(source);
        }
        let manager = self.manager().ok()?;
        Some(Arc::new(SdkNames(manager.sdk_arc())))
    }

    /// Queues a background fetch of the due names among `ids`, shown to
    /// `identity` of wallet `id`. Session close cancels it
    /// (`ContactsRuntime::close`).
    pub(super) fn fetch_contact_names(
        &self,
        id: WalletId,
        identity: &str,
        ids: impl IntoIterator<Item = Identifier>,
    ) {
        if !self.platform.enabled {
            return;
        }
        let names = self.platform.contacts.names.clone();
        let claimed = names.claim(ids, (id, identity.to_owned()));
        if claimed.is_empty() {
            return;
        }
        let (Some(source), Ok(live)) = (self.name_source(), self.live()) else {
            for contact in claimed {
                names.fail(contact);
            }
            return;
        };
        let (hub, provenance) = (Arc::clone(&self.hub), live.provenance);
        let mut owed = Owed {
            names,
            ids: claimed.into(),
        };
        (self.platform.contacts).spawn(&self.rt, async move {
            while let Some(&contact) = owed.ids.front() {
                let answer = tokio::time::timeout(FETCH_TIMEOUT, source.username(contact))
                    .await
                    .unwrap_or(Err(PlatformError::Timeout));
                owed.ids.pop_front();
                let publish = |c, label: Option<&str>| provenance.show_label(c, label);
                for (wallet, identity) in owed.names.land(contact, answer, &publish) {
                    (hub.pump).mark_platform(wallet, [PlatformChange::Contacts { identity }]);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::provenance::Provenance;
    use super::*;

    const W: WalletId = WalletId([0xAA; 32]);

    fn id(n: u8) -> Identifier {
        Identifier::from([n; 32])
    }

    fn waiter(owner: &str) -> (WalletId, String) {
        (W, owner.to_owned())
    }

    /// One fetch at a time per name, every reader meanwhile waits for it, and
    /// only a change signals; a failure signals no one and is not retried at
    /// once.
    #[test]
    fn a_name_is_fetched_once_and_signals_only_a_change() {
        let names = ContactNames::default();
        assert_eq!(names.claim([id(2), id(3)], waiter("a")), [id(2), id(3)]);
        assert!(names.claim([id(2)], waiter("b")).is_empty());
        assert_eq!(
            names.land(id(2), Ok(Some("bob".into())), &|_, _| {}),
            BTreeSet::from([waiter("a"), waiter("b")])
        );
        assert_eq!(names.label(&id(2)).as_deref(), Some("bob"));
        assert!(names.claim([id(2)], waiter("a")).is_empty());

        assert!(
            names
                .land(id(3), Err(PlatformError::Unavailable), &|_, _| {})
                .is_empty()
        );
        assert_eq!(names.label(&id(3)), None);
        assert!(names.claim([id(3)], waiter("a")).is_empty());

        // Stale again, the same answer: nothing shown changes.
        guard(&names.states).get_mut(&id(2)).unwrap().checked = None;
        assert_eq!(names.claim([id(2)], waiter("a")), [id(2)]);
        assert!(
            names
                .land(id(2), Ok(Some("bob".into())), &|_, _| {})
                .is_empty()
        );
        // A name that goes away is a change.
        guard(&names.states).get_mut(&id(2)).unwrap().checked = None;
        assert_eq!(names.claim([id(2)], waiter("a")), [id(2)]);
        assert_eq!(
            names.land(id(2), Ok(None), &|_, _| {}),
            BTreeSet::from([waiter("a")])
        );
        assert_eq!(names.label(&id(2)), None);
    }

    /// A forgotten wallet's readers are not signalled; a fetch task that
    /// ends early lands what it owed as failed.
    #[test]
    fn forgotten_waiters_and_owed_names() {
        let names = ContactNames::default();
        let other = (WalletId([0xBB; 32]), "b".to_owned());
        names.claim([id(2)], waiter("a"));
        names.claim([id(2)], other.clone());
        names.forget(&W);
        assert_eq!(
            names.land(id(2), Ok(Some("bob".into())), &|_, _| {}),
            BTreeSet::from([other])
        );

        names.claim([id(3), id(4)], waiter("a"));
        drop(Owed {
            names: names.clone(),
            ids: VecDeque::from([id(3), id(4)]),
        });
        let states = guard(&names.states);
        for n in [3, 4] {
            let state = &states[&id(n)];
            assert!(!state.in_flight);
            assert!(state.checked.is_some_and(|(_, failed)| failed));
        }
    }

    /// Sol r3 R3-01: a landing publishes its name while the names are
    /// locked, so an earlier landing's publication can never overwrite a
    /// later one, however long it stalls. Worker A lands `bob` and stalls in
    /// its publication; worker B, due again meanwhile, re-spells the name or
    /// withdraws it, and lands only after A has published. Provenance ends
    /// with the name shown. A failure publishes nothing.
    #[test]
    fn a_landing_publishes_before_the_next_one_lands() {
        let bob = id(2);
        for later in [Some("B0b"), None] {
            let names = ContactNames::default();
            let appdb = Arc::new(dw_appdb::AppDb::open_in_memory().unwrap());
            let provenance = Arc::new(Provenance::open(appdb));
            let b = Mutex::new(None);
            names.claim([bob], waiter("a"));
            names.land(bob, Ok(Some("bob".into())), &|contact, label| {
                assert!(names.states.try_lock().is_err(), "published unlocked");
                // Worker B.
                let (b_names, b_provenance) = (names.clone(), Arc::clone(&provenance));
                *guard(&b) = Some(std::thread::spawn(move || {
                    b_names.expire(&bob);
                    assert_eq!(b_names.claim([bob], waiter("a")), [bob]);
                    let publish = |c, label: Option<&str>| b_provenance.show_label(c, label);
                    b_names.land(bob, Ok(later.map(str::to_owned)), &publish);
                }));
                std::thread::sleep(Duration::from_millis(200));
                let landed = guard(&b).as_ref().is_some_and(|b| b.is_finished());
                assert!(!landed, "B landed during A's publication");
                provenance.show_label(contact, label);
            });
            guard(&b).take().unwrap().join().unwrap();
            assert_eq!(names.label(&bob).as_deref(), later);
            let shown: Vec<_> = (later.iter()).map(|l| (bob, l.to_string())).collect();
            assert_eq!(provenance.shown_labels(), shown);

            names.expire(&bob);
            assert_eq!(names.claim([bob], waiter("a")), [bob]);
            let published = |_, _: Option<&str>| panic!("a failure published");
            names.land(bob, Err(PlatformError::Unavailable), &published);
            assert_eq!(names.label(&bob).as_deref(), later);
            assert_eq!(provenance.shown_labels(), shown);
        }
    }

    /// A name is due when never asked, an hour after an answer, a minute
    /// after a failure, and never while in flight.
    #[test]
    fn when_a_name_is_due() {
        let now = Instant::now();
        let second = Duration::from_secs(1);
        let checked = |failed| NameState {
            checked: Some((now, failed)),
            ..Default::default()
        };
        assert!(NameState::default().due(now));
        assert!(!checked(false).due(now + FRESH - second));
        assert!(checked(false).due(now + FRESH));
        assert!(!checked(true).due(now + RETRY - second));
        assert!(checked(true).due(now + RETRY));
        let flying = NameState {
            in_flight: true,
            ..Default::default()
        };
        assert!(!flying.due(now + FRESH));
    }
}
