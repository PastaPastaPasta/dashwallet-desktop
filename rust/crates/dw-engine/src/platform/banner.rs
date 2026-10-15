//! `DashPay::status`, the banner state (BL-153; DASHPAY §3.6, F1): what
//! the identity chip, the Home card and the Contacts empty state show.
//!
//! The call is sync and offline-safe: it reads memory (the bring-up's
//! outcome, the identity list, the sync tracker, the vault's seed index)
//! and app.sqlite (registration rows, contest watches, main-name rows, the
//! bring-up markers), never Platform. [`derive_status`] holds the rules
//! (`docs/design/BL-153-dashpay-status.md`); [`NetworkSession::dashpay_status_of`]
//! gathers what they read.

use dw_appdb::{ContestWatch, RegistrationRow};

use super::bringup::read_bring_up_markers;
use super::contest_watch::{Seen, watch_of};
use super::names::same_name;
use super::registration_flow::failure_of;
use super::registration_rows::{Decoded, Funding, Phase};
use super::startup::{DashPayStatus, NoIdentityReason, StartupStatus};
use crate::{EngineError, NetworkSession, WalletId};

/// The main identity as the identity list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MainIdentity {
    pub(super) identity: String,
    /// The names it shows (owned by Platform's evidence).
    pub(super) names: Vec<String>,
    /// Its temporary name, a contested registration's second name.
    pub(super) temporary: Option<String>,
}

/// Everything [`derive_status`] reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StatusInputs {
    /// The wallet has no keys.
    pub(super) read_only: bool,
    pub(super) startup: StartupStatus,
    /// The identity the bring-up named.
    pub(super) startup_identity: Option<String>,
    pub(super) main: Option<MainIdentity>,
    /// The identity list was read from the library, not the snapshot a
    /// sync pass left (DEC-138): only then does an empty list, or a list of
    /// names, say anything.
    pub(super) live: bool,
    pub(super) registrations: Vec<RegistrationRow>,
    pub(super) contests: Vec<ContestWatch>,
    /// Created here, or Platform proved within its TTL that the seed owns no
    /// identity (the bring-up's markers).
    pub(super) absence_on_file: bool,
    /// SPV is caught up: Core and masternode sync are done.
    pub(super) caught_up: bool,
}

/// The banner state (D2–D7 of the BL-153 design).
pub(super) fn derive_status(i: &StatusInputs) -> DashPayStatus {
    let Some(main) = i
        .main
        .as_ref()
        .map(|m| m.identity.as_str())
        .or_else(|| on_file(i))
    else {
        if i.read_only {
            return DashPayStatus::NoIdentity {
                reason: Some(NoIdentityReason::WatchOnly),
            };
        }
        let registering_one =
            |r: &&RegistrationRow| creates_identity(r) && in_progress(r).is_some();
        if let Some(row) = newest(i.registrations.iter().filter(registering_one)) {
            return registering(row);
        }
        // An empty snapshot proves nothing: the identity may be on file.
        if i.live && (i.startup == StartupStatus::NoIdentity || i.absence_on_file) {
            return DashPayStatus::NoIdentity {
                reason: (!i.caught_up).then_some(NoIdentityReason::WaitingForSync),
            };
        }
        return DashPayStatus::StartupIncomplete { startup: i.startup };
    };
    // The flow that creates `main`, before its name step ended (D5).
    let creating_main = i.registrations.iter().filter(|r| {
        r.identity.as_deref() == Some(main)
            && creates_identity(r)
            && in_progress(r).is_some_and(before_name)
    });
    if let Some(row) = newest(creating_main) {
        return registering(row);
    }
    if let Some(m) = i.main.as_ref()
        && let Some((label, ends_at)) = open_contest(i, m)
    {
        return DashPayStatus::ContestPending {
            identity: main.to_owned(),
            label,
            ends_at,
        };
    }
    DashPayStatus::Ready {
        main: main.to_owned(),
    }
}

/// A flow that creates its identity and has not ended without a name: not
/// one adding a name to an identity that existed before it (D15).
fn creates_identity(r: &RegistrationRow) -> bool {
    r.name_error.is_none()
        && !matches!(
            Funding::decode(&r.funding),
            Decoded::Known(Funding::ExistingIdentity)
        )
}

/// While the list is a snapshot with no identity (D4): the identity the
/// bring-up named, else the newest finished flow's.
fn on_file(i: &StatusInputs) -> Option<&str> {
    if i.live {
        return None;
    }
    let done = |r: &&RegistrationRow| {
        r.identity.is_some()
            && failure_of(r).is_none()
            && Phase::parse(&r.phase) == Some(Phase::Done)
    };
    (i.startup_identity.as_deref()).or_else(|| {
        newest(i.registrations.iter().filter(done))?
            .identity
            .as_deref()
    })
}

/// A phase before the name step's end.
fn before_name(p: Phase) -> bool {
    matches!(
        p,
        Phase::Draft
            | Phase::KeysPrepared
            | Phase::FundingSent
            | Phase::ProofWaiting
            | Phase::IdentityRegistered
            | Phase::NameRequested
    )
}

/// The main identity's open contest, while it shows no name but its
/// temporary one on a live read (D6): its label and end, `None` while
/// the watch has not read it.
fn open_contest(i: &StatusInputs, m: &MainIdentity) -> Option<(String, Option<u64>)> {
    let temporary = |n: &String| m.temporary.as_deref().is_some_and(|t| same_name(n, t));
    if !i.live || !m.names.iter().all(temporary) {
        return None;
    }
    let open = |w: &&ContestWatch| {
        w.identity == m.identity
            && !matches!(Seen::parse(w.last_state.as_deref()), Seen::Won { .. })
    };
    let registered = newest(i.registrations.iter().filter(|r| {
        r.identity.as_deref() == Some(m.identity.as_str())
            && in_progress(r) == Some(Phase::Contested)
    }));
    let label = match registered {
        Some(row) => row.label.clone(),
        None => (i.contests.iter().filter(open))
            .min_by_key(|w| (w.ends_at == 0, w.ends_at, w.label.as_str()))?
            .label
            .clone(),
    };
    let ends_at = watch_of(&i.contests, &m.identity, &label)
        .filter(open)
        .map(|w| w.ends_at)
        .filter(|&end| end > 0);
    Some((label, ends_at))
}

/// The row's phase while its flow is in progress: short of `Done`, and not
/// failed, or failed retryably (D5). A row this build cannot read is a
/// failure that cannot be retried.
fn in_progress(row: &RegistrationRow) -> Option<Phase> {
    if failure_of(row).is_some_and(|(_, retryable)| !retryable) {
        return None;
    }
    Phase::parse(&row.phase).filter(|&p| p != Phase::Done)
}

fn newest<'a>(rows: impl Iterator<Item = &'a RegistrationRow>) -> Option<&'a RegistrationRow> {
    rows.max_by_key(|r| r.id)
}

fn registering(row: &RegistrationRow) -> DashPayStatus {
    DashPayStatus::Registering {
        draft: row.id.to_string(),
    }
}

impl NetworkSession {
    /// `DashPay::status`: the inputs of [`derive_status`], from memory and
    /// app.sqlite. A read that fails fails the call.
    pub(super) fn dashpay_status_of(&self, id: WalletId) -> Result<DashPayStatus, EngineError> {
        let startup = self.dashpay_startup(&id)?;
        let mut read = self.identity_read(id)?;
        let main = read.summaries.into_iter().find(|s| s.is_main).map(|s| {
            let prefs = read.names.remove(&s.identity);
            MainIdentity {
                temporary: prefs.and_then(|p| p.temporary),
                identity: s.identity,
                names: s.names,
            }
        });
        let db = self.live()?.appdb;
        let wallet = id.to_string();
        let absence_on_file = if main.is_none() && read.live {
            let (created_here, proven) = read_bring_up_markers(&db, id)?;
            created_here || proven
        } else {
            false
        };
        Ok(derive_status(&StatusInputs {
            read_only: startup.read_only,
            startup: startup.startup,
            startup_identity: startup.identity,
            main,
            live: read.live,
            registrations: db.registration_rows(&wallet)?,
            contests: db.wallet_contest_watches(&wallet)?,
            absence_on_file,
            caught_up: self.hub.tracker().caught_up(),
        }))
    }
}
