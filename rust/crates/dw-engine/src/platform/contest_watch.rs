//! The contest watch (DASHPAY §3.2, §3.5; ROADMAP DP1-04): the wallet's own
//! contested usernames, watched until their vote resolves, and each outcome
//! journaled as a feed event.
//!
//! - **Rows.** One `dp_contest_watch` row per contest: written when
//!   `register_name` finds the identity contending ([`NetworkSession::start_watching`]),
//!   and adopted at each pass from the library's open contests. A row goes
//!   when its contest resolves; the row's `last_state` is what the watch
//!   last saw ([`Seen`]).
//! - **Cadence** ([`due_at`]). Every 10 minutes while the end is unknown or
//!   more than an hour away, every minute from an hour before the end to an
//!   hour after it (Platform settles a poll in the first block after its
//!   end), every 10 minutes again after that. The supervisor runs one pass
//!   right after the loops start, so a contest that ended while the app was
//!   closed shows at once, and re-arms when a row is written. No rows, no
//!   timer and no request. A pass asks one vote-state page per due row, at
//!   most [`PASS_CAP`] rows, and one end-date page ([`ends_page`]) while a
//!   due row's end is unknown.
//! - **Outcomes** ([`verdict`]). The vote state's winner decides: this
//!   identity won, another identity won (lost), or masternodes locked the
//!   name (nobody gets it, now or later at this pin, and no outcome returns
//!   the contest fund). No winner while the identity contends is still
//!   voting, and so is an open poll's full page that does not list the
//!   identity (it may be on a later one). A resolved poll lists every
//!   contender. A row whose identity a complete list lacks, or whose label
//!   has no poll, is `Unplaced`: no outcome, and the row goes once that has
//!   lasted [`UNPLACED_FOR`].
//! - **Journal.** Outcomes go through the changeset tap's one news writer
//!   (`ChangesetTap::journal`): a feed row, the unread count and
//!   `DashPayNews`, with catch-up silence by the time of the block that
//!   resolved the poll. The watch alone journals `ContestWon`: the tap
//!   stores a watched label's `UsernameRegistered` read and journals no win
//!   of its own, whichever comes first. A win's row stays until the library
//!   lists the name. A loss or lock leaves the open contests with its row.
//! - **Fencing** (DP2-05's rule for a removal and a reimport). The network
//!   reads run unfenced; every write of a pass holds the wallet's
//!   [`super::private_details::WriteFences`] fence for the `PlatformWallet`
//!   it read, so once that wallet left the manager (removed, or replaced by
//!   a reimport) the pass writes nothing.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dash_sdk::drive::query::VotePollsByEndDateDriveQuery;
use dash_sdk::drive::query::vote_poll_vote_state_query::{
    ContestedDocumentVotePollDriveQuery, ContestedDocumentVotePollDriveQueryResultType,
};
use dash_sdk::platform::FetchMany;
use dash_sdk::query_types::{Contenders, VotePollsGroupedByTimestamp};
use dpp::platform_value::Value;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::system_data_contracts::SystemDataContract;
use dpp::util::strings::convert_to_homograph_safe_chars;
use dpp::voting::contender_structs::ContenderWithSerializedDocument;
use dpp::voting::vote_info_storage::contested_document_vote_poll_winner_info::ContestedDocumentVotePollWinnerInfo;
use dpp::voting::vote_polls::VotePoll;
use dpp::voting::vote_polls::contested_document_resource_vote_poll::ContestedDocumentResourceVotePoll;
use dw_appdb::ContestWatch;
use platform_wallet::PlatformWallet;

use super::dashpay::DashPay;
use super::errors::{IdentityError, PlatformError};
use super::identity_writes::identity_locks;
use super::journal::{Age, Event, name_ref};
use super::names::{ContestContender, ContestState, ContestStatus};
use super::notifications::EventKind;
use super::private_details::WriteFence;
use super::registration_flow::b58;
use super::runtime::{PlatformSignal, guard};
use crate::events::unix_now;
use crate::session::Manager;
use crate::{NetworkSession, WalletId};

/// The watch's pace while a contest's end is unknown or far, seconds.
const WATCH_IDLE: u64 = 600;
/// Its pace around the end, seconds.
const WATCH_ENDING: u64 = 60;
/// How long before and after the end the watch keeps [`WATCH_ENDING`].
const ENDING_WINDOW: u64 = 3600;
/// Rows one pass asks Platform about, at most, per wallet; the rest wait
/// for the next pass.
pub(super) const PASS_CAP: usize = 16;
/// How long one wallet's pass may take; it runs inside the supervisor.
pub(super) const PASS_BUDGET: Duration = Duration::from_secs(20);
/// How long a row may stay [`Verdict::Unplaced`] before the watch stops,
/// seconds: DAPI nodes lag a new contender by seconds, never a day.
const UNPLACED_FOR: u64 = 24 * 3600;
/// How often a won row asks the marketplace loop for a pass, seconds.
const MARKETPLACE_EVERY: u64 = 600;
/// How long a won row waits for the library to list the name, seconds; a
/// later listing is then a plain `UsernameRegistered`.
const WON_FOR: u64 = 24 * 3600;
/// Contenders on the one vote-state page a row's check reads while the
/// poll is open (Drive's default; a contest may have 1,000). A resolved
/// poll's answer is its stored result, every contender whatever the limit.
pub(super) const POLL_PAGE: u16 = 100;
/// Polls on the one end-date page a pass reads: the soonest-ending from an
/// hour ago on, which are the ones the 1-minute cadence needs.
pub(super) const ENDS_PAGE: u16 = 100;

/// A vote poll as the watch reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Poll {
    /// Set once the poll resolved, with the time (ms) of the block that
    /// resolved it.
    pub(crate) winner: Option<(PollWinner, u64)>,
    /// With their tallies: every contender once the poll resolved (Drive
    /// stores them all), else the first [`POLL_PAGE`] in identity order.
    pub(crate) contenders: Vec<(Identifier, Option<u32>)>,
    pub(crate) lock_votes: Option<u32>,
    pub(crate) abstain_votes: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PollWinner {
    Identity(Identifier),
    Locked,
    /// Drive stores it only for a poll with no contender.
    NoWinner,
}

impl Poll {
    pub(crate) fn of(state: Contenders) -> Self {
        let winner = state.winner.map(|(winner, block)| {
            let winner = match winner {
                ContestedDocumentVotePollWinnerInfo::WonByIdentity(id) => PollWinner::Identity(id),
                ContestedDocumentVotePollWinnerInfo::Locked => PollWinner::Locked,
                ContestedDocumentVotePollWinnerInfo::NoWinner => PollWinner::NoWinner,
            };
            (winner, block.time_ms)
        });
        Self {
            winner,
            contenders: (state.contenders.iter())
                .map(|(id, c)| (*id, c.vote_tally()))
                .collect(),
            lock_votes: state.lock_vote_tally,
            abstain_votes: state.abstain_vote_tally,
        }
    }

    fn lists(&self, me: &Identifier) -> bool {
        self.contenders.iter().any(|(id, _)| id == me)
    }

    /// Whether a contender `contenders` lacks may be on a later page: an
    /// open poll's full page.
    fn truncated(&self) -> bool {
        self.winner.is_none() && self.contenders.len() >= usize::from(POLL_PAGE)
    }
}

/// What a poll means for one of its contenders (DP1-04 D2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Voting, or ended and waiting for the block that resolves it (its
    /// full page need not list the identity).
    Open,
    Won {
        at: u64,
    },
    /// `winner` `None`: the poll resolved with no winner and no lock.
    Lost {
        winner: Option<Identifier>,
        at: u64,
    },
    Locked {
        at: u64,
    },
    /// No poll for the label, or the identity is not among its contenders
    /// (a resolved poll keeps them listed).
    Unplaced,
}

/// The verdict of `poll` for `me`; `at` is the resolving block's time (ms).
pub(crate) fn verdict(poll: &Poll, me: &Identifier) -> Verdict {
    match poll.winner {
        // The winner contended.
        Some((PollWinner::Identity(id), at)) if id == *me => Verdict::Won { at },
        // An open poll's full page proves nothing about a contender it
        // lacks; a resolved poll lists them all.
        _ if !poll.lists(me) && poll.truncated() => Verdict::Open,
        _ if !poll.lists(me) => Verdict::Unplaced,
        Some((PollWinner::Identity(id), at)) => Verdict::Lost {
            winner: Some(id),
            at,
        },
        Some((PollWinner::Locked, at)) => Verdict::Locked { at },
        Some((PollWinner::NoWinner, at)) => Verdict::Lost { winner: None, at },
        None => Verdict::Open,
    }
}

/// What the watch last saw of a contest, as `last_state` stores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Seen {
    /// Not looked at yet.
    New,
    Open,
    /// Won at `at` (ms), journaled; waiting since `since` (UNIX s) for
    /// the library to list the name.
    Won {
        at: u64,
        since: u64,
    },
    /// Unplaced since `since` (UNIX s).
    Unplaced {
        since: u64,
    },
}

impl Seen {
    pub(crate) fn parse(text: Option<&str>) -> Self {
        let stamp = |rest: &str| rest.parse::<u64>().ok();
        match text {
            Some("open") => Self::Open,
            Some(t) => match t.split_once(':') {
                Some(("won", rest)) => match rest.split_once(':') {
                    Some((at, since)) => match (stamp(at), stamp(since)) {
                        (Some(at), Some(since)) => Self::Won { at, since },
                        _ => Self::New,
                    },
                    None => Self::New,
                },
                Some(("unplaced", since)) => {
                    stamp(since).map_or(Self::New, |since| Self::Unplaced { since })
                }
                _ => Self::New,
            },
            None => Self::New,
        }
    }

    pub(crate) fn text(self) -> Option<String> {
        match self {
            Self::New => None,
            Self::Open => Some("open".into()),
            Self::Won { at, since } => Some(format!("won:{at}:{since}")),
            Self::Unplaced { since } => Some(format!("unplaced:{since}")),
        }
    }
}

/// When a row is next due, UNIX seconds, if it was last checked at
/// `checked` (`None`: never in this process, so now).
pub(crate) fn due_at(row: &ContestWatch, checked: Option<u64>) -> u64 {
    let Some(checked) = checked else {
        return 0;
    };
    let ends = row.ends_at;
    let pace = if matches!(Seen::parse(row.last_state.as_deref()), Seen::Won { .. }) {
        // No query: the library's list only.
        WATCH_ENDING
    } else if ends == 0 || checked >= ends.saturating_add(ENDING_WINDOW) {
        WATCH_IDLE
    } else if checked.saturating_add(ENDING_WINDOW) < ends {
        // Far from the end: idle, but not past the start of its last hour.
        return (checked + WATCH_IDLE).min(ends - ENDING_WINDOW);
    } else {
        WATCH_ENDING
    };
    checked + pace
}

/// What a pass asks of Platform, behind a seam: [`SdkContests`] asks the
/// SDK; the tests script it.
pub(crate) trait ContestNet: Send + Sync {
    /// One vote-state request for the poll for `normalized`: the winner,
    /// the tallies, and the first `limit` contenders in identity order.
    fn page(
        &self,
        normalized: &str,
        limit: u16,
    ) -> impl Future<Output = Result<Poll, PlatformError>> + Send;
    /// One end-date request: the end (UNIX s) of at most `limit` polls,
    /// the soonest ending from `from` (UNIX s) on, by normalized label.
    fn end_page(
        &self,
        from: u64,
        limit: u16,
    ) -> impl Future<Output = Result<BTreeMap<String, u64>, PlatformError>> + Send;
}

/// The one end-date page a pass reads ([`ENDS_PAGE`]): a poll stays in the
/// index until the block that resolves it, so from an hour ago on.
pub(crate) async fn ends_page(
    net: &impl ContestNet,
) -> Result<BTreeMap<String, u64>, PlatformError> {
    let from = unix_now().saturating_sub(ENDING_WINDOW);
    net.end_page(from, ENDS_PAGE).await
}

pub(crate) struct SdkContests(pub(crate) Arc<dash_sdk::Sdk>);

/// DPNS's `domain` poll for `normalized`, as the SDK's own DPNS queries
/// name it.
fn dpns_poll(normalized: &str) -> ContestedDocumentResourceVotePoll {
    ContestedDocumentResourceVotePoll {
        contract_id: SystemDataContract::DPNS.id(),
        document_type_name: "domain".into(),
        index_name: "parentNameAndLabel".into(),
        index_values: vec![Value::Text("dash".into()), Value::Text(normalized.into())],
    }
}

// One request each: the SDK's `get_current_dpns_contests` pages on its own.
impl ContestNet for SdkContests {
    async fn page(&self, normalized: &str, limit: u16) -> Result<Poll, PlatformError> {
        let query = ContestedDocumentVotePollDriveQuery {
            vote_poll: dpns_poll(normalized),
            result_type: ContestedDocumentVotePollDriveQueryResultType::DocumentsAndVoteTally,
            allow_include_locked_and_abstaining_vote_tally: true,
            start_at: None,
            limit: Some(limit),
            offset: None,
        };
        let state = ContenderWithSerializedDocument::fetch_many(&self.0, query).await?;
        Ok(Poll::of(state))
    }

    async fn end_page(
        &self,
        from: u64,
        limit: u16,
    ) -> Result<BTreeMap<String, u64>, PlatformError> {
        let query = VotePollsByEndDateDriveQuery {
            start_time: Some((from.saturating_mul(1000), true)),
            end_time: None,
            limit: Some(limit),
            offset: None,
            order_ascending: true,
        };
        let groups: VotePollsGroupedByTimestamp = VotePoll::fetch_many(&self.0, query).await?;
        let dpns = SystemDataContract::DPNS.id();
        let mut ends = BTreeMap::new();
        for (ms, polls) in groups.0 {
            for VotePoll::ContestedDocumentResourceVotePoll(poll) in polls {
                if poll.contract_id == dpns
                    && poll.document_type_name == "domain"
                    && let Some(Value::Text(label)) = poll.index_values.get(1)
                {
                    ends.insert(label.clone(), ms / 1000);
                }
            }
        }
        Ok(ends)
    }
}

/// The session's watch state (in memory only).
#[derive(Default)]
pub(crate) struct ContestsRuntime {
    /// When each row was last checked in this process, UNIX s.
    checked: Mutex<HashMap<(WalletId, String, String), u64>>,
    /// When each wallet last asked the marketplace loop for a pass.
    kicked: Mutex<HashMap<WalletId, u64>>,
    /// Tests: stands in for Platform.
    #[cfg(test)]
    pub(crate) net: Mutex<Option<Arc<super::contest_watch_tests::FakePolls>>>,
}

fn row_key(id: WalletId, row: &ContestWatch) -> (WalletId, String, String) {
    (id, row.identity.clone(), row.label.clone())
}

impl ContestsRuntime {
    fn checked(&self, id: WalletId, row: &ContestWatch) -> Option<u64> {
        guard(&self.checked).get(&row_key(id, row)).copied()
    }

    fn mark_checked(&self, id: WalletId, row: &ContestWatch, at: u64) {
        guard(&self.checked).insert(row_key(id, row), at);
    }

    /// Whether the wallet may ask the marketplace loop for a pass now.
    fn may_kick(&self, id: WalletId, now: u64) -> bool {
        let mut kicked = guard(&self.kicked);
        let last = kicked.get(&id).copied();
        let may = last.is_none_or(|at| now >= at + MARKETPLACE_EVERY);
        if may {
            kicked.insert(id, now);
        }
        may
    }

    /// Forgets a removed or closed wallet.
    pub(crate) fn forget(&self, id: &WalletId) {
        guard(&self.checked).retain(|(w, _, _), _| w != id);
        guard(&self.kicked).remove(id);
    }
}

/// One of the wallet's identities as the library holds it.
struct Mine {
    id: Identifier,
    /// The open contests, normalized.
    open: Vec<String>,
    /// The listed names, normalized.
    listed: Vec<String>,
}

async fn identities_of(wallet: &PlatformWallet) -> BTreeMap<String, Mine> {
    let state = wallet.state().await;
    state
        .identity_manager
        .wallet_managed_identities(&wallet.wallet_id())
        .filter(|m| m.identity_index.is_some())
        .map(|m| {
            let id = dpp::identity::accessors::IdentityGettersV0::id(&m.identity);
            let mine = Mine {
                id,
                open: (m.contested_dpns_names.iter())
                    .map(|l| convert_to_homograph_safe_chars(l))
                    .collect(),
                listed: (m.dpns_names.iter())
                    .map(|n| convert_to_homograph_safe_chars(&n.label))
                    .collect(),
            };
            (id.to_string(Encoding::Base58), mine)
        })
        .collect()
}

/// Whether a row's label is `label`'s contest (DPNS's normalized compare).
fn same_contest(label: &str) -> impl Fn(&str) -> bool + use<> {
    let safe = convert_to_homograph_safe_chars(label);
    move |other| convert_to_homograph_safe_chars(other) == safe
}

/// The row of `identity` watching `label` (DPNS's normalized compare).
pub(crate) fn watch_of<'a>(
    rows: &'a [ContestWatch],
    identity: &str,
    label: &str,
) -> Option<&'a ContestWatch> {
    let same = same_contest(label);
    rows.iter()
        .find(|r| r.identity == identity && same(&r.label))
}

impl NetworkSession {
    /// What the watch asks of Platform.
    pub(super) fn contest_net(&self, manager: &Manager) -> SdkOrFake {
        #[cfg(test)]
        if let Some(fake) = guard(&self.platform.contests.net).clone() {
            return SdkOrFake::Fake(fake);
        }
        SdkOrFake::Sdk(SdkContests(manager.sdk_arc()))
    }

    /// Starts watching a contest `register_name` found the identity
    /// contending in (`ends_at` when known), and re-arms the watch. A row
    /// of the same contest keeps its label's spelling. Best effort: the
    /// next pass adopts the contest from the library's open contests if
    /// this write is lost; a wallet no longer the manager's writes nothing.
    pub(super) async fn start_watching(
        &self,
        wallet: &PlatformWallet,
        id: WalletId,
        identity: &Identifier,
        label: &str,
        ends_at: Option<u64>,
    ) {
        let written = async {
            let fence = self.write_fence(id, wallet).await?;
            let (w, i, label) = (id.to_string(), b58(identity), label.to_string());
            (self.fenced_op(&fence, "watch", move |db| {
                let same = same_contest(&label);
                db.watch_contest_once(&w, &i, &label, ends_at.unwrap_or(0), true, same)
            }))
            .await
        };
        match written.await {
            Ok(_) => self.platform.signal(PlatformSignal::Contests),
            Err(e) => tracing::warn!(wallet_id = %id, error = %e, "could not watch a contest"),
        }
    }

    /// One wallet's watch pass with Platform's own answers.
    pub(super) async fn contest_pass(&self, manager: &Manager, id: WalletId) {
        let net = self.contest_net(manager);
        if let Err(e) = self.watch_contests(&net, manager, id).await {
            tracing::info!(wallet_id = %id, error = %e, "contest watch pass stopped");
        }
    }

    /// The pass: adopts the library's open contests, then asks Platform
    /// about each due row and applies what it says.
    pub(crate) async fn watch_contests(
        &self,
        net: &impl ContestNet,
        manager: &Manager,
        id: WalletId,
    ) -> Result<(), PlatformError> {
        let Some(wallet) = manager.get_wallet(&id.0).await else {
            return Ok(());
        };
        // Not before the wallet's first bring-up here: a restore's old
        // outcome would be news.
        if !self.has_catch_up(id).await {
            return Ok(());
        }
        let now = unix_now();
        let mine = identities_of(&wallet).await;
        let w = id.to_string();
        let rows = self
            .appdb_op(move |db| db.wallet_contest_watches(&w))
            .await?;
        let contests = &self.platform.contests;
        let mut due: Vec<(u64, ContestWatch)> = rows
            .iter()
            .filter_map(|row| {
                let checked = contests.checked(id, row);
                (due_at(row, checked) <= now).then(|| (checked.unwrap_or(0), row.clone()))
            })
            .collect();
        // The longest unchecked first.
        due.sort_by_key(|(checked, _)| *checked);
        due.truncate(PASS_CAP);
        // Before anything else: a pass that hangs or fails still moves on.
        for (_, row) in &due {
            contests.mark_checked(id, row, now);
        }
        for row in self.adopt(&wallet, id, &mine, &rows).await? {
            if due.len() < PASS_CAP {
                contests.mark_checked(id, &row, now);
                due.push((0, row));
            }
        }
        let unknown_end = due.iter().any(|(_, row)| {
            row.ends_at == 0 && !matches!(Seen::parse(row.last_state.as_deref()), Seen::Won { .. })
        });
        let ends = if unknown_end {
            ends_page(net).await.unwrap_or_else(|e| {
                tracing::info!(wallet_id = %id, "contest ends unavailable: {e}");
                BTreeMap::new()
            })
        } else {
            BTreeMap::new()
        };

        for (_, row) in due {
            let Some(me) = mine.get(&row.identity) else {
                continue;
            };
            let normalized = convert_to_homograph_safe_chars(&row.label);
            let listed = me.listed.contains(&normalized);
            if let Seen::Won { at, .. } = Seen::parse(row.last_state.as_deref()) {
                self.apply_contest(&wallet, id, &row, Verdict::Won { at }, 0, listed, now)
                    .await?;
                continue;
            }
            let poll = match net.page(&normalized, POLL_PAGE).await {
                Ok(poll) => poll,
                Err(e) => {
                    tracing::info!(wallet_id = %id, label = %normalized, "vote state unavailable: {e}");
                    continue;
                }
            };
            let ends_at = ends.get(&normalized).copied().unwrap_or(0);
            let verdict = verdict(&poll, &me.id);
            self.apply_contest(&wallet, id, &row, verdict, ends_at, listed, now)
                .await?;
        }
        Ok(())
    }

    /// Watches each open contest of the library's that `rows` lacks, end
    /// unknown, in the spelling the identity last contended with when it
    /// folds the same (the library keeps the folded label); the rows
    /// written, once each however `register_name` races it.
    async fn adopt(
        &self,
        wallet: &PlatformWallet,
        id: WalletId,
        mine: &BTreeMap<String, Mine>,
        rows: &[ContestWatch],
    ) -> Result<Vec<ContestWatch>, PlatformError> {
        let mut new = Vec::new();
        for (identity, m) in mine {
            let open: Vec<&String> = (m.open.iter())
                .filter(|label| watch_of(rows, identity, label).is_none())
                .collect();
            if open.is_empty() {
                continue;
            }
            let typed = self.name_prefs(id, identity.clone()).await?.contested;
            for label in open {
                let typed = typed
                    .as_ref()
                    .filter(|t| convert_to_homograph_safe_chars(t) == *label);
                new.push((identity.clone(), typed.unwrap_or(label).clone()));
            }
        }
        if new.is_empty() {
            return Ok(Vec::new());
        }
        let fence = self.write_fence(id, wallet).await?;
        let w = id.to_string();
        let adopted = (self.fenced_op(&fence, "watch", move |db| {
            let mut adopted = Vec::new();
            for (identity, label) in new {
                let same = same_contest(&label);
                let watched = db.watch_contest_once(&w, &identity, &label, 0, false, same)?;
                adopted.push((identity, watched));
            }
            db.wallet_contest_watches(&w).map(|rows| {
                rows.into_iter()
                    .filter(|r| {
                        adopted
                            .iter()
                            .any(|(i, l)| *i == r.identity && *l == r.label)
                    })
                    .collect()
            })
        }))
        .await?;
        Ok(adopted)
    }

    /// Applies `verdict` to `row` under the wallet's write fence (module
    /// doc): `ends_at` is an end learned this pass (0: none), kept while
    /// the poll is open; `listed` whether the library lists the label as
    /// the identity's name. An outcome's ref uses only the row's stored
    /// end, which a retry reads again (a resolved poll has left the
    /// end-date index).
    #[expect(clippy::too_many_arguments, reason = "one row's context")]
    pub(super) async fn apply_contest(
        &self,
        wallet: &PlatformWallet,
        id: WalletId,
        row: &ContestWatch,
        verdict: Verdict,
        ends_at: u64,
        listed: bool,
        now: u64,
    ) -> Result<(), PlatformError> {
        let seen = Seen::parse(row.last_state.as_deref());
        let end = match verdict {
            Verdict::Open | Verdict::Unplaced if row.ends_at == 0 => ends_at,
            _ => row.ends_at,
        };
        let fence = self.write_fence(id, wallet).await?;
        let (w, i, label) = (id.to_string(), row.identity.clone(), row.label.clone());
        let set_state = |state: Seen| {
            let (w, i, label) = (w.clone(), i.clone(), label.clone());
            let text = state.text().unwrap_or_default();
            move |db: &dw_appdb::AppDb| db.set_contest_watch_state(&w, &i, &label, end, &text)
        };
        let (kind, at) = match verdict {
            Verdict::Open => {
                if seen != Seen::Open || end != row.ends_at {
                    self.fenced_op(&fence, "watch", set_state(Seen::Open))
                        .await?;
                }
                return Ok(());
            }
            Verdict::Unplaced => {
                let since = match seen {
                    Seen::Unplaced { since } => since,
                    _ => now,
                };
                if now >= since.saturating_add(UNPLACED_FOR) {
                    // The identity does not contend: the library's open
                    // contest is stale too (as DP1-03 forgets a refusal).
                    tracing::info!(wallet_id = %id, label = %label, "no contest found for a day; no longer watched");
                    self.end_watch(wallet, id, &fence, row).await?;
                } else if seen != (Seen::Unplaced { since }) || end != row.ends_at {
                    self.fenced_op(&fence, "watch", set_state(Seen::Unplaced { since }))
                        .await?;
                }
                return Ok(());
            }
            Verdict::Won { at } => {
                // Before the event: a won row waits for the listing with no
                // query.
                if !matches!(seen, Seen::Won { at: seen_at, .. } if seen_at == at) {
                    let won = Seen::Won { at, since: now };
                    self.fenced_op(&fence, "watch", set_state(won)).await?;
                }
                (EventKind::ContestWon, at)
            }
            Verdict::Lost { at, .. } => (EventKind::ContestLost, at),
            Verdict::Locked { at } => (EventKind::ContestLocked, at),
        };

        let event = Event {
            identity: row.identity.clone(),
            kind,
            contact: String::new(),
            reference: name_ref(&row.label, Some(end)),
            age: Age::At(at),
        };
        let tap = self.live()?.tap;
        let epoch = tap.epoch(id);
        let journaled = (self.fenced_op(&fence, "contest outcome", move |_| {
            Ok(tap.journal(id, epoch, vec![event], &mut BTreeSet::new()))
        }))
        .await?;
        if !journaled {
            // The next pass asks again; the event is written once.
            return Ok(());
        }
        let waited = match seen {
            Seen::Won { since, .. } => now.saturating_sub(since),
            _ => 0,
        };
        if kind == EventKind::ContestWon && !listed && waited < WON_FOR {
            self.kick_marketplace(id, now);
            return Ok(());
        }
        self.end_watch(wallet, id, &fence, row).await
    }

    /// Takes `row`'s label out of the identity's open contests in the
    /// library, then deletes the row, under `fence`.
    async fn end_watch(
        &self,
        wallet: &PlatformWallet,
        id: WalletId,
        fence: &WriteFence,
        row: &ContestWatch,
    ) -> Result<(), PlatformError> {
        let identity = Identifier::from_string(&row.identity, Encoding::Base58).map_err(|_| {
            PlatformError::Internal {
                detail: "a watched identity is not base58".into(),
            }
        })?;
        {
            let locks = identity_locks(id, identity);
            let _state = locks.state.lock().await;
            let mut state = wallet.state_mut().await;
            if let Some(managed) = state.identity_manager.managed_identity_mut(&identity) {
                DashPay::drop_contest(managed, &row.label, wallet.persister());
            }
        }
        let (w, i, label) = (id.to_string(), row.identity.clone(), row.label.clone());
        (self.fenced_op(fence, "watch", move |db| {
            db.end_contest_watch(&w, &i, &label)
        }))
        .await?;
        Ok(())
    }

    /// `contest_status` (names.rs) for `identity` of wallet `id` and
    /// `label`, but its temporary name.
    pub(crate) async fn contest_status_of(
        &self,
        net: &impl ContestNet,
        wallet: &PlatformWallet,
        (id, identity): (WalletId, Identifier),
        label: &str,
        normalized: &str,
    ) -> Result<ContestStatus, PlatformError> {
        let mine = identities_of(wallet).await;
        let me =
            (mine.get(&b58(&identity))).ok_or(PlatformError::Identity(IdentityError::NotFound))?;
        let poll = net.page(normalized, POLL_PAGE).await?;
        let w = id.to_string();
        let rows = self
            .appdb_op(move |db| db.wallet_contest_watches(&w))
            .await?;
        let row = watch_of(&rows, &b58(&identity), label).cloned();
        let verdict = verdict(&poll, &identity);
        let state = match &verdict {
            Verdict::Open => ContestState::Open,
            // A watched contest DAPI does not show yet.
            Verdict::Unplaced if row.is_some() => ContestState::Open,
            Verdict::Unplaced => {
                return Err(PlatformError::InvalidArgument {
                    detail: "the identity does not contend for this label".into(),
                });
            }
            Verdict::Won { .. } => ContestState::Won,
            Verdict::Lost { winner, .. } => ContestState::Lost {
                winner: winner.as_ref().map(b58),
            },
            Verdict::Locked { .. } => ContestState::Locked,
        };
        let mut ends_at = row.as_ref().map(|r| r.ends_at).filter(|&end| end > 0);
        if ends_at.is_none() && state == ContestState::Open {
            ends_at = (ends_page(net).await.ok()).and_then(|e| e.get(normalized).copied());
        }
        if let Some(row) = &row
            && state != ContestState::Open
            && self.has_catch_up(id).await
        {
            let listed = me.listed.iter().any(|l| l == normalized);
            let learned = ends_at.unwrap_or(0);
            (self.apply_contest(wallet, id, row, verdict, learned, listed, unix_now())).await?;
        }
        Ok(ContestStatus {
            label: label.to_string(),
            state,
            ends_at,
            contenders: (poll.contenders.iter())
                .map(|(who, votes)| ContestContender {
                    identity: b58(who),
                    votes: *votes,
                    is_self: *who == identity,
                })
                .collect(),
            lock_votes: poll.lock_votes,
            abstain_votes: poll.abstain_votes,
            temporary_name: None,
        })
    }

    /// Asks the marketplace loop (`dpns_sync`) for a pass, which lists a
    /// won name, at most every [`MARKETPLACE_EVERY`] per wallet, and only
    /// while the loops run.
    fn kick_marketplace(&self, id: WalletId, now: u64) {
        if !self
            .platform
            .loops_on
            .load(std::sync::atomic::Ordering::Acquire)
            || !self.platform.contests.may_kick(id, now)
        {
            return;
        }
        let Ok(manager) = self.manager() else {
            return;
        };
        let dpns = manager.dpns_sync_arc();
        let loops_on = Arc::clone(&self.platform.loops_on);
        self.rt.spawn(async move {
            // Not after a stop that came first.
            if loops_on.load(std::sync::atomic::Ordering::Acquire) {
                dpns.sync_now().await;
            }
        });
    }

    /// When the supervisor runs the next pass: when the next row of any
    /// wallet is due, `None` with none watched.
    pub(super) async fn next_contest_pass(&self, manager: &Manager) -> Option<Instant> {
        let mut next: Option<u64> = None;
        for raw in manager.wallet_ids().await {
            let id = WalletId(raw);
            // Its bring-up re-arms the watch (`WalletAdded`/`Readmit`).
            if !self.has_catch_up(id).await {
                continue;
            }
            let w = id.to_string();
            let rows = match self.appdb_op(move |db| db.wallet_contest_watches(&w)).await {
                Ok(rows) => rows,
                Err(e) => {
                    tracing::warn!(wallet_id = %id, error = %e, "could not read the contest watches");
                    continue;
                }
            };
            for row in &rows {
                let due = due_at(row, self.platform.contests.checked(id, row));
                next = Some(next.map_or(due, |n| n.min(due)));
            }
        }
        let wait = next?.saturating_sub(unix_now());
        Some(Instant::now() + Duration::from_secs(wait))
    }
}

/// The SDK, or in tests a script.
pub(crate) enum SdkOrFake {
    Sdk(SdkContests),
    #[cfg(test)]
    Fake(Arc<super::contest_watch_tests::FakePolls>),
}

impl ContestNet for SdkOrFake {
    async fn page(&self, normalized: &str, limit: u16) -> Result<Poll, PlatformError> {
        match self {
            Self::Sdk(net) => net.page(normalized, limit).await,
            #[cfg(test)]
            Self::Fake(net) => net.page(normalized, limit).await,
        }
    }

    async fn end_page(
        &self,
        from: u64,
        limit: u16,
    ) -> Result<BTreeMap<String, u64>, PlatformError> {
        match self {
            Self::Sdk(net) => net.end_page(from, limit).await,
            #[cfg(test)]
            Self::Fake(net) => net.end_page(from, limit).await,
        }
    }
}
