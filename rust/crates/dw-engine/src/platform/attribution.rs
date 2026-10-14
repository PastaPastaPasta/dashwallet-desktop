//! Who a payment was with (DP3-02; DASHPAY F15): the counterparty of
//! history records, contact activity and the frequent strip. Not part of
//! the facade: `payments.rs` holds the records and the calls.
//!
//! - **By address first.** DIP-15 gives each ordered pair of identities its
//!   own address chain, and the Core wallet's DashPay accounts carry both
//!   ids (`DashpayAccountKey`): an output to a receival account's address
//!   is a payment received from that account's friend, and one to an
//!   external account's address, in a transaction the wallet funded, a
//!   payment sent to its friend. Every address of the accounts' pools
//!   counts, used or not.
//! - **The library's word for the rest.** After a restore an external pool
//!   holds only the gap past its last used index as key-wallet sees it, so
//!   a payment sent further out matches no address. The library's DashPay
//!   pass rebuilds those (`reconcile_sent_payments_from_tx_history` walks
//!   past the gap) as `Sent` payment entries: a transaction with exactly
//!   one payee record and `Sent` entries (not failed) naming one contact is
//!   attributed to that contact. Received payments need no fallback: an
//!   output is only detected through a receival pool's address.
//! - **Not attributed:** a payment between two identities of one wallet
//!   (all outputs the wallet's own: an internal transfer), a mixed debit
//!   transaction (`Other`), coinbase and Platform transfers.
//! - **What it shows.** `Counterparty` is display data with no provenance
//!   flag (m4 §5): the contact's id, its username from the contacts' name
//!   cache (fetched in the background as `contacts` fetches it; a landing
//!   name signals `Platform{Contacts}` for the owner) and the cached
//!   contact profile's display name and avatar URL.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::str::FromStr;
use std::sync::Arc;

use dashcore::{ScriptBuf, Transaction, Txid};
use dpp::identity::accessors::IdentityGettersV0;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use key_wallet::account::account_collection::DashpayAccountKey;
use key_wallet::managed_account::ManagedCoreFundsAccount;
use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
use platform_wallet::PlatformWalletInfo;
use platform_wallet::wallet::identity::{PaymentDirection, PaymentStatus};

use super::contacts::{ContactSummary, Relation};
use super::contacts_model::identifier;
use super::errors::{ContactError, IdentityError, PlatformError};
use super::payments::{
    ActivityDirection, ActivityFilter, ActivityItem, ActivityPage, Counterparty,
};
use super::runtime::guard;
use crate::history::{ChainView, Part, TxEntry, TxStatusKind, TxType, decompose, status_of};
use crate::history_ops::ViewParts;
use crate::{EngineError, NetworkSession, WalletId};

/// Items on one page of contact activity.
pub(crate) const ACTIVITY_PAGE: usize = 50;
/// The frequent strip's window (Android's `TIMESPAN`): 90 days.
pub(crate) const FREQUENT_WINDOW_SECS: u64 = 90 * 24 * 60 * 60;
/// The most contacts the frequent strip returns.
pub(crate) const MAX_FREQUENT: u32 = 20;

/// One side of an attributed payment: the wallet's identity, the other
/// identity and which way the money went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Side {
    pub(crate) owner: Identifier,
    pub(crate) contact: Identifier,
    pub(crate) direction: ActivityDirection,
}

/// The wallet's DashPay payees, built from the library's state under its
/// lock and read without it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Payees {
    /// Every address script of the DashPay accounts.
    scripts: HashMap<ScriptBuf, Side>,
    /// The library's `Sent` payment entries (not failed), by transaction:
    /// `(owner, contact)`.
    sent: HashMap<Txid, BTreeSet<(Identifier, Identifier)>>,
    /// Who each contact is, as shown.
    parties: BTreeMap<Identifier, Counterparty>,
}

/// Whether a record part is a payee of a send (not change, not data).
fn is_payee(p: &Part) -> bool {
    matches!(
        p.tx_type,
        TxType::SendToAddress | TxType::SendToOther | TxType::CoinJoinSend
    ) && p.vout.is_some()
}

impl Payees {
    /// From the library's memory: the wallet's DashPay accounts, its
    /// identities' payment entries and cached contact profiles.
    pub(crate) fn of(info: &PlatformWalletInfo, wallet: &WalletId) -> Self {
        let mut payees = Self::default();
        let accounts = &info.core_wallet.accounts;
        for (direction, map) in [
            (
                ActivityDirection::Received,
                &accounts.dashpay_receival_accounts,
            ),
            (ActivityDirection::Sent, &accounts.dashpay_external_accounts),
        ] {
            for (key, account) in map {
                payees.add_account(key, account, direction);
            }
        }
        let identities = || info.identity_manager.wallet_managed_identities(&wallet.0);
        for m in identities() {
            let owner = m.identity.id();
            for (txid, p) in &m.dashpay().payments {
                if p.direction == PaymentDirection::Sent
                    && p.status != PaymentStatus::Failed
                    && let Ok(txid) = Txid::from_str(txid)
                {
                    payees.add_sent(txid, owner, p.counterparty_id);
                }
            }
        }
        // After every party exists: a profile may be cached on another
        // identity than the one whose entry named the contact.
        for m in identities() {
            for (contact, cached) in &m.dashpay().contact_profiles {
                if let (Some(party), Some(profile)) =
                    (payees.parties.get_mut(contact), &cached.profile)
                {
                    party.display_name = profile.display_name.clone();
                    party.avatar_url = profile.avatar_url.clone();
                }
            }
        }
        payees
    }

    fn add_account(
        &mut self,
        key: &DashpayAccountKey,
        account: &ManagedCoreFundsAccount,
        direction: ActivityDirection,
    ) {
        let side = Side {
            owner: Identifier::from(key.user_identity_id),
            contact: Identifier::from(key.friend_identity_id),
            direction,
        };
        for pool in account.managed_account_type().address_pools() {
            for info in pool.addresses.values() {
                self.add_script(info.script_pubkey.clone(), side);
            }
        }
    }

    fn party(&mut self, contact: Identifier) {
        self.parties.entry(contact).or_insert_with(|| Counterparty {
            identity: contact.to_string(Encoding::Base58),
            username: None,
            display_name: None,
            avatar_url: None,
        });
    }

    /// Joins the usernames the contacts' name cache holds.
    pub(crate) fn name(mut self, label: impl Fn(&Identifier) -> Option<String>) -> Self {
        for (contact, party) in &mut self.parties {
            party.username = label(contact);
        }
        self
    }

    /// Each contact a payment could be attributed to, by owner (every
    /// DashPay account and `Sent` entry): the usernames a read may show.
    pub(crate) fn shown(&self) -> BTreeMap<Identifier, BTreeSet<Identifier>> {
        let mut shown: BTreeMap<Identifier, BTreeSet<Identifier>> = BTreeMap::new();
        let pairs = (self.scripts.values()).map(|s| (s.owner, s.contact));
        for (owner, contact) in pairs.chain(self.sent.values().flatten().copied()) {
            shown.entry(owner).or_default().insert(contact);
        }
        shown
    }

    /// The side of each of `parts`, the records of `tx` (`None` where it
    /// is not a contact payment).
    pub(crate) fn sides(&self, txid: &Txid, tx: &Transaction, parts: &[Part]) -> Vec<Option<Side>> {
        let mut sides: Vec<Option<Side>> = (parts.iter())
            .map(|p| {
                let want = match p.tx_type {
                    TxType::RecvWithAddress => ActivityDirection::Received,
                    _ if is_payee(p) => ActivityDirection::Sent,
                    _ => return None,
                };
                let script = &tx.output.get(p.vout? as usize)?.script_pubkey;
                self.scripts
                    .get(script)
                    .filter(|s| s.direction == want)
                    .copied()
            })
            .collect();
        // The library's word, for a send past the resident pool: only when
        // no output pays a DashPay address the wallet knows. Otherwise its
        // entry may name that output (one the wallet owns, as a payment to
        // its own other identity), not the payee.
        let mut payees = (0..parts.len()).filter(|&i| is_payee(&parts[i]));
        if let (Some(only), None) = (payees.next(), payees.next())
            && sides[only].is_none()
            && !(tx.output.iter()).any(|o| self.scripts.contains_key(&o.script_pubkey))
            && let Some(pairs) = self.sent.get(txid)
            && pairs.len() == 1
            && let Some(&(owner, contact)) = pairs.first()
        {
            sides[only] = Some(Side {
                owner,
                contact,
                direction: ActivityDirection::Sent,
            });
        }
        sides
    }

    /// `script` pays or is paid by `side`.
    pub(crate) fn add_script(&mut self, script: ScriptBuf, side: Side) {
        self.scripts.insert(script, side);
        self.party(side.contact);
    }

    /// The library holds a `Sent` entry of `txid` for the pair.
    pub(crate) fn add_sent(&mut self, txid: Txid, owner: Identifier, contact: Identifier) {
        self.sent.entry(txid).or_default().insert((owner, contact));
        self.party(contact);
    }

    /// The counterparty record of `contact`.
    pub(crate) fn counterparty(&self, contact: &Identifier) -> Option<Counterparty> {
        self.parties.get(contact).cloned()
    }
}

impl NetworkSession {
    /// The wallet's payees with the usernames the cache holds now.
    pub(crate) fn payees(&self, info: &PlatformWalletInfo, wallet: &WalletId) -> Payees {
        let names = &self.platform.contacts.names;
        Payees::of(info, wallet).name(|c| names.label(c))
    }

    /// After a read that showed `payees`: fetches the usernames due, as a
    /// contacts read does, so a landing name signals `Platform{Contacts}`
    /// for its owner.
    pub(crate) fn fetch_payee_names(&self, wallet: WalletId, payees: &Payees) {
        for (owner, contacts) in payees.shown() {
            let owner = owner.to_string(Encoding::Base58);
            self.fetch_contact_names(wallet, &owner, contacts);
        }
    }
}

/// A transaction's attributed payments: per side, the duffs of its outputs.
fn payments_of(txid: &Txid, entry: &TxEntry, view: &ChainView<'_>) -> Vec<(Side, u64)> {
    let d = decompose(entry, view);
    let (status, _) = status_of(entry, view, d.all_from_me);
    // Never moved money.
    if matches!(
        status.kind,
        TxStatusKind::Abandoned | TxStatusKind::NotAccepted | TxStatusKind::Conflicted
    ) {
        return Vec::new();
    }
    let sides = view.payees.sides(txid, &entry.tx, &d.parts);
    let mut by_side: Vec<(Side, u64)> = Vec::new();
    for (part, side) in d.parts.iter().zip(sides) {
        let (Some(side), Some(vout)) = (side, part.vout) else {
            continue;
        };
        let value = entry.tx.output.get(vout as usize).map_or(0, |o| o.value);
        match by_side.iter_mut().find(|(s, _)| *s == side) {
            Some((_, sum)) => *sum += value,
            None => by_side.push((side, value)),
        }
    }
    by_side
}

/// The block time of a mined transaction (DEC-114: authoritative only).
fn block_time(entry: &TxEntry) -> Option<u64> {
    entry.context.block_info().map(|b| u64::from(b.timestamp()))
}

/// Where an activity item sorts, newest first: mined ones by block time,
/// unmined ones before them all; then the txid, then the direction (`r`
/// before `s`; one transaction is never both for one pair). Its text is the page cursor: 16 hex digits of the time key,
/// the txid's first 46 hex digits and `s` or `r` (63 characters).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ActivityKey {
    time: Reverse<u64>,
    txid: String,
    direction: char,
}

impl ActivityKey {
    const TXID_DIGITS: usize = 46;

    fn new(entry: &TxEntry, txid: &Txid, direction: ActivityDirection) -> Self {
        let mut txid = txid.to_string();
        txid.truncate(Self::TXID_DIGITS);
        Self {
            time: Reverse(block_time(entry).unwrap_or(u64::MAX)),
            txid,
            direction: match direction {
                ActivityDirection::Sent => 's',
                ActivityDirection::Received => 'r',
            },
        }
    }

    fn cursor(&self) -> String {
        format!("{:016x}{}{}", self.time.0, self.txid, self.direction)
    }

    fn parse(cursor: &str) -> Option<Self> {
        let hex = |s: &str| s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if cursor.len() != 16 + Self::TXID_DIGITS + 1 || !cursor.is_ascii() {
            return None;
        }
        let (time, rest) = cursor.split_at(16);
        let (txid, direction) = rest.split_at(Self::TXID_DIGITS);
        let direction = direction
            .chars()
            .next()
            .filter(|d| matches!(d, 's' | 'r'))?;
        if !hex(time) || !hex(txid) {
            return None;
        }
        Some(Self {
            time: Reverse(u64::from_str_radix(time, 16).ok()?),
            txid: txid.to_owned(),
            direction,
        })
    }
}

/// `owner`'s payments with `contact` in the view, filtered, newest first,
/// the page after `after`.
pub(crate) fn activity(
    view: &ChainView<'_>,
    owner: &Identifier,
    contact: &Identifier,
    filter: ActivityFilter,
    after: Option<&str>,
) -> Result<ActivityPage, PlatformError> {
    let after = (after.map(|c| {
        ActivityKey::parse(c).ok_or_else(|| PlatformError::InvalidArgument {
            detail: "not an activity cursor".into(),
        })
    }))
    .transpose()?;
    let wanted = |d: ActivityDirection| match filter {
        ActivityFilter::All => true,
        ActivityFilter::Sent => d == ActivityDirection::Sent,
        ActivityFilter::Received => d == ActivityDirection::Received,
    };
    let mut items: BTreeMap<ActivityKey, ActivityItem> = BTreeMap::new();
    for (txid, entry) in view.history {
        for (side, amount) in payments_of(txid, entry, view) {
            if side.owner != *owner || side.contact != *contact || !wanted(side.direction) {
                continue;
            }
            let key = ActivityKey::new(entry, txid, side.direction);
            if after.as_ref().is_some_and(|a| key <= *a) {
                continue;
            }
            let height = entry.context.block_info().map(|b| b.height());
            items.insert(
                key,
                ActivityItem {
                    txid: txid.to_string(),
                    direction: side.direction,
                    amount,
                    height,
                    timestamp: block_time(entry).or(entry.first_seen),
                },
            );
        }
    }
    let more = items.len() > ACTIVITY_PAGE;
    let page: Vec<(ActivityKey, ActivityItem)> = items.into_iter().take(ACTIVITY_PAGE).collect();
    Ok(ActivityPage {
        next_cursor: page.last().filter(|_| more).map(|(k, _)| k.cursor()),
        items: page.into_iter().map(|(_, item)| item).collect(),
    })
}

/// How often `owner` paid and was paid by each contact in the window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Score {
    pub(crate) sent: u32,
    pub(crate) last_sent: u64,
    pub(crate) received: u32,
    pub(crate) last_received: u64,
}

/// Each contact's score for `owner`: the transactions in the
/// [`FREQUENT_WINDOW_SECS`] before `now` (by block time; an unmined one
/// counts as now), one per direction.
pub(crate) fn scores(
    view: &ChainView<'_>,
    owner: &Identifier,
    now: u64,
) -> BTreeMap<Identifier, Score> {
    let since = now.saturating_sub(FREQUENT_WINDOW_SECS);
    let mut scores: BTreeMap<Identifier, Score> = BTreeMap::new();
    for (txid, entry) in view.history {
        let at = block_time(entry).unwrap_or(now);
        if at < since {
            continue;
        }
        for (side, _) in payments_of(txid, entry, view) {
            if side.owner != *owner {
                continue;
            }
            let score = scores.entry(side.contact).or_default();
            let (count, last) = match side.direction {
                ActivityDirection::Sent => (&mut score.sent, &mut score.last_sent),
                ActivityDirection::Received => (&mut score.received, &mut score.last_received),
            };
            *count += 1;
            *last = (*last).max(at);
        }
    }
    scores
}

/// The frequent strip (Android's `getTopContacts`): established contacts
/// that are not hidden, the most often paid first, then, to fill `limit`,
/// the most often paying; ties by the latest payment, then the id. Only
/// contacts with a payment in the window.
pub(crate) fn frequent(
    contacts: Vec<(Identifier, ContactSummary)>,
    scores: &BTreeMap<Identifier, Score>,
    limit: usize,
) -> Vec<ContactSummary> {
    let ranked = |count: fn(&Score) -> (u32, u64)| {
        let mut ranked: Vec<_> = (contacts.iter())
            .filter_map(|(id, s)| {
                let c = count(scores.get(id)?);
                (c.0 > 0).then_some((Reverse(c), s.contact.as_str(), id))
            })
            .collect();
        ranked.sort();
        ranked
            .into_iter()
            .map(|(_, _, id)| *id)
            .collect::<Vec<Identifier>>()
    };
    let mut chosen: Vec<Identifier> = ranked(|s| (s.sent, s.last_sent));
    chosen.truncate(limit);
    for id in ranked(|s| (s.received, s.last_received)) {
        if chosen.len() >= limit {
            break;
        }
        if !chosen.contains(&id) {
            chosen.push(id);
        }
    }
    let mut by_id: BTreeMap<Identifier, ContactSummary> = contacts.into_iter().collect();
    (chosen.iter()).filter_map(|id| by_id.remove(id)).collect()
}

impl NetworkSession {
    /// `DashPay::contact_activity`.
    pub(super) async fn contact_activity_of(
        &self,
        wallet: WalletId,
        identity: String,
        contact: String,
        cursor: Option<String>,
        filter: ActivityFilter,
    ) -> Result<ActivityPage, ContactError> {
        let _op = self.enter().await.map_err(PlatformError::from)?;
        self.require_wallet(&wallet).map_err(PlatformError::from)?;
        let owner = identifier(&identity)?;
        let contact = identifier(&contact)?;
        if owner == contact {
            return Err(ContactError::IsSelf);
        }
        let manager = self.manager().map_err(PlatformError::from)?;
        let owned = {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            (wm.get_wallet_info(&wallet.0)).is_some_and(|info| {
                (info.identity_manager.wallet_managed_identities(&wallet.0))
                    .any(|m| m.identity.id() == owner)
            })
        };
        if !owned {
            return Err(PlatformError::from(IdentityError::NotFound).into());
        }
        let page = self
            .with_view(&manager, wallet, |view| {
                activity(view, &owner, &contact, filter, cursor.as_deref())
            })
            .await
            .map_err(PlatformError::from)??;
        Ok(page)
    }

    /// The wallet's view parts, live when the library's state is free, else
    /// the last ones (`None` before any): a sync read never waits.
    fn view_parts_now(&self, wallet: &WalletId) -> Result<Option<Arc<ViewParts>>, EngineError> {
        let manager = self.manager()?;
        let wm = manager.wallet_manager_arc();
        let views = &self.platform.contacts.views;
        if let Ok(wm) = wm.try_read() {
            let parts =
                (wm.get_wallet_info(&wallet.0)).map(|info| Arc::new(self.view_parts(info, wallet)));
            match &parts {
                Some(p) => guard(views).insert(*wallet, Arc::clone(p)),
                None => guard(views).remove(wallet),
            };
            return Ok(parts);
        }
        Ok(guard(views).get(wallet).cloned())
    }

    /// `DashPay::frequent_contacts`: from the contacts read (live or its
    /// snapshot) and the in-memory history.
    pub(super) fn frequent_of(
        &self,
        wallet: WalletId,
        identity: String,
        limit: u32,
    ) -> Result<Vec<ContactSummary>, ContactError> {
        let op = self.try_enter().map_err(PlatformError::from)?;
        let owner = identifier(&identity)?;
        let contacts = self.with_contacts_entered(&op, wallet, &identity, |owned, join, _| {
            Ok((owned.into_iter().flat_map(|o| &o.entries))
                .filter(|e| e.relation == Relation::Contact && !e.private.hidden)
                .map(|e| (e.contact, e.summary(join)))
                .collect::<Vec<_>>())
        })?;
        if contacts.is_empty() {
            return Ok(Vec::new());
        }
        let Some(parts) = self.view_parts_now(&wallet).map_err(PlatformError::from)? else {
            return Ok(Vec::new());
        };
        let network = self.network.core_network();
        let now = crate::events::unix_now();
        let scores = (self.hub.history).read(&wallet, |history| {
            scores(&parts.view(history, network), &owner, now)
        });
        let limit = limit.clamp(1, MAX_FREQUENT) as usize;
        Ok(frequent(contacts, &scores, limit))
    }
}
