//! Platform addresses behind the facade calls of `platform_addresses.rs`
//! (release 0.1, lane E2; m4 §2.12): the balance as the sync loop left it,
//! the activity rows, and "Move to Dash wallet". Not part of the facade.
//!
//! **Activity.** platform-wallet keeps balances, not transfers. Each
//! `platform_activity` read compares the synced balances with the snapshot
//! the last read stored (app.sqlite, the wallet's local scope) and records a
//! row for each address whose balance rose (`incoming`) or fell
//! (`outgoing`). The first read records every funded address as incoming. A
//! move's own spend is no `outgoing` row: while it is pending its inputs are
//! not compared, and once it is done the first fall of each input, up to
//! what the move took from it plus its fee estimate, is the move's.
//!
//! **Moves.** A move withdraws to the wallet's current Core receiving
//! address. Its inputs, the nonces it signs at and the credits it takes are
//! fixed first, and recorded `pending` durably (and read back) before the
//! permit's signer can sign anything; it signs at those nonces
//! (`ExplicitWithNonces`), so it executes at most once. An answer with the
//! proved balances settles it `done`; a failure with no signature released
//! is `failed`; any other failure reads the inputs' nonces once: all at the
//! signed ones is `done`, else it stays `pending`. A later read settles a
//! pending move the same way, and calls it `failed` once ten minutes pass
//! without (a nonce taken by another device's spend reads as done: 0.1 has
//! one device per wallet). Nothing retries a move; a pending one refuses the
//! next (`credits.move_pending`).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use dash_sdk::platform::FetchMany;
use dash_sdk::query_types::AddressInfo;
use dpp::address_funds::{AddressFundsFeeStrategy, AddressFundsFeeStrategyStep, PlatformAddress};
use dpp::balances::credits::CREDITS_PER_DUFF;
use dpp::dashcore::Network;
use dpp::state_transition::address_credit_withdrawal_transition::MIN_CORE_FEE_PER_BYTE;
use dpp::version::PlatformVersion;
use key_wallet::PlatformP2PKHAddress;
use key_wallet::bip32::DerivationPath;
use platform_wallet::wallet::platform_addresses::{InputSelection, WithdrawalPlan};
use platform_wallet::{PlatformWallet, PlatformWalletError};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::credits::WithdrawAmount;
use super::errors::{CreditsError, PlatformError};
use super::flows::{FlowKind, GrantRequest};
use super::names::{NameAuthority, check_credit_cap};
use super::platform_addresses::{
    MoveToCoreOutcome, MoveToCoreQuote, PlatformActivity, PlatformActivityKind,
    PlatformActivityState, PlatformAddressBalance, PlatformAddressRow,
};
use super::requests::{leased, write_permit};
use super::withdrawal::{self, Rules};
use crate::events::unix_now;
use crate::session::Manager;
use crate::{NetworkSession, PlatformChange, WalletId};

/// The DIP-17 payment account the wallet uses (key class 0).
const ACCOUNT: u32 = 0;
/// The Core fee rate of the asset unlock, Platform's minimum (as DP6-02).
const CORE_FEE_PER_BYTE: u32 = MIN_CORE_FEE_PER_BYTE;
/// How long a move may stay pending before it is called failed.
const PENDING_FOR: u64 = 600;
/// The change rows kept, the newest.
const ROWS_KEPT: usize = 200;
/// How long `sync_platform_addresses` waits for a pass already running.
const PASS_WAIT: Duration = Duration::from_secs(120);

const SNAPSHOT_KEY: &str = "platform_address/snapshot";
const ROW_PREFIX: &str = "platform_address/row/";
const MOVE_PREFIX: &str = "platform_address/move/";

/// One move as recorded before it is signed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MoveRecord {
    pub(crate) id: String,
    pub(crate) inputs: Vec<MoveInput>,
    /// What the addresses lose, the fee estimate included.
    pub(crate) credits: u64,
    pub(crate) fee_credits: u64,
    pub(crate) expected_duffs: u64,
    pub(crate) to: String,
    pub(crate) at: u64,
    pub(crate) state: PlatformActivityState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MoveInput {
    /// bech32m.
    pub(crate) address: String,
    /// The nonce the transition signs at (Platform's stored one + 1).
    pub(crate) nonce: u32,
    /// What the move withdraws from it.
    pub(crate) credits: u64,
    /// Once the move is done: the fall of the address's balance still to
    /// be read as the move's (module doc), 0 once read.
    #[serde(default)]
    pub(crate) unseen: u64,
}

impl MoveRecord {
    fn activity(&self) -> PlatformActivity {
        PlatformActivity {
            id: self.id.clone(),
            kind: PlatformActivityKind::MoveToCore,
            state: self.state,
            credits: self.credits,
            address: None,
            to: Some(self.to.clone()),
            expected_duffs: Some(self.expected_duffs),
            at: self.at,
        }
    }

    /// Done: each input's fall is the move's, up to its credits and the fee.
    fn done(&mut self) {
        self.state = PlatformActivityState::Done;
        for input in &mut self.inputs {
            input.unseen = input.credits.saturating_add(self.fee_credits);
        }
    }
}

/// address (bech32m) → credits.
pub(crate) type Snapshot = BTreeMap<String, u64>;

/// The change rows from `before` (`None`: no read yet) to `now`, and the
/// snapshot to store. `held` (a pending move's inputs) is not compared and
/// keeps its stored value; `unseen` (address → a done move's fall still to
/// read) absorbs the first fall of its address and is then spent.
pub(crate) fn changes(
    before: Option<&Snapshot>,
    now: &Snapshot,
    held: &BTreeSet<String>,
    unseen: &mut BTreeMap<String, u64>,
    at: u64,
) -> (Vec<PlatformActivity>, Snapshot) {
    let empty = Snapshot::new();
    let before_map = before.unwrap_or(&empty);
    let mut rows = Vec::new();
    let mut next = Snapshot::new();
    let addresses: BTreeSet<&String> = before_map.keys().chain(now.keys()).collect();
    for address in addresses {
        let (was, is) = (
            before_map.get(address).copied().unwrap_or(0),
            now.get(address).copied().unwrap_or(0),
        );
        if held.contains(address) {
            if let Some(&was) = before_map.get(address) {
                next.insert(address.clone(), was);
            }
            continue;
        }
        if is > 0 {
            next.insert(address.clone(), is);
        }
        let row = |kind, credits| PlatformActivity {
            id: format!("seen-{at}-{address}"),
            kind,
            state: PlatformActivityState::Done,
            credits,
            address: Some(address.clone()),
            to: None,
            expected_duffs: None,
            at,
        };
        if is > was {
            rows.push(row(PlatformActivityKind::Incoming, is - was));
        } else if was > is {
            let fell = was - is;
            let ours = unseen.get_mut(address).map_or(0, std::mem::take);
            if fell > ours {
                rows.push(row(PlatformActivityKind::Outgoing, fell - ours));
            }
        }
    }
    (rows, next)
}

/// Where `record` stands given its inputs' proved nonces (`None`: the read
/// failed, or the address is unknown to Platform) at `now`.
pub(crate) fn settled(
    record: &MoveRecord,
    nonces: &BTreeMap<String, Option<u32>>,
    now: u64,
) -> PlatformActivityState {
    let reached = record.inputs.iter().all(|i| {
        nonces
            .get(&i.address)
            .copied()
            .flatten()
            .is_some_and(|n| n >= i.nonce)
    });
    if reached {
        PlatformActivityState::Done
    } else if now >= record.at.saturating_add(PENDING_FOR) {
        PlatformActivityState::Failed
    } else {
        PlatformActivityState::Pending
    }
}

/// The inputs that withdraw `want` of `plan`'s (an "all" plan: each input's
/// balance, the fee source's less the fee estimate), and the fee strategy
/// that keeps the fee on the plan's fee source: the fee source first, then
/// the largest; a last input below `min_input` borrows from the one before.
/// `None` when no such set exists.
pub(crate) fn select(
    plan: &WithdrawalPlan,
    want: u64,
    min_input: u64,
) -> Option<(BTreeMap<PlatformAddress, u64>, AddressFundsFeeStrategy)> {
    let source = plan.fee_strategy.iter().find_map(|s| match s {
        AddressFundsFeeStrategyStep::DeductFromInput(i) => {
            plan.inputs.keys().nth(usize::from(*i)).copied()
        }
        AddressFundsFeeStrategyStep::ReduceOutput(_) => None,
    })?;
    let mut order: Vec<(PlatformAddress, u64)> =
        plan.inputs.iter().map(|(a, c)| (*a, *c)).collect();
    order.sort_by_key(|(a, c)| (*a != source, std::cmp::Reverse(*c), *a));
    let mut taken: Vec<(PlatformAddress, u64)> = Vec::new();
    let mut left = want;
    for (address, credits) in order {
        if left == 0 {
            break;
        }
        let take = credits.min(left);
        taken.push((address, take));
        left -= take;
    }
    if left > 0 {
        return None;
    }
    if let [.., before, last] = taken.as_mut_slice()
        && last.1 < min_input
    {
        let short = min_input - last.1;
        if before.1 < min_input.saturating_add(short) {
            return None;
        }
        before.1 -= short;
        last.1 = min_input;
    }
    if taken.iter().any(|(_, c)| *c < min_input) {
        return None;
    }
    let inputs: BTreeMap<PlatformAddress, u64> = taken.into_iter().collect();
    let index = inputs.keys().position(|a| *a == source)?;
    let index = u16::try_from(index).ok()?;
    Some((
        inputs,
        vec![AddressFundsFeeStrategyStep::DeductFromInput(index)],
    ))
}

/// One wallet's moves and activity reads, one at a time.
fn wallet_lock(wallet: WalletId) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: LazyLock<Mutex<HashMap<WalletId, Arc<tokio::sync::Mutex<()>>>>> =
        LazyLock::new(Mutex::default);
    let mut locks = LOCKS.lock().unwrap_or_else(PoisonError::into_inner);
    Arc::clone(locks.entry(wallet).or_default())
}

/// The wallet's records in app.sqlite (its local scope).
struct Store {
    appdb: Arc<dw_appdb::AppDb>,
    scope: String,
}

impl Store {
    fn of(session: &NetworkSession, wallet: WalletId) -> Result<Self, PlatformError> {
        Ok(Self {
            appdb: session.live()?.appdb,
            scope: dw_appdb::local_scope(&wallet.to_string()),
        })
    }

    async fn blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce(&dw_appdb::AppDb, &str) -> Result<T, dw_appdb::AppDbError> + Send + 'static,
    ) -> Result<T, PlatformError> {
        let (appdb, scope) = (Arc::clone(&self.appdb), self.scope.clone());
        tokio::task::spawn_blocking(move || f(&appdb, &scope))
            .await
            .map_err(|e| PlatformError::from(crate::EngineError::from(e)))?
            .map_err(|e| PlatformError::Storage {
                detail: e.to_string(),
            })
    }

    /// The values under `prefix`; one that does not parse is skipped.
    async fn all<T: DeserializeOwned + Send + 'static>(
        &self,
        prefix: &'static str,
    ) -> Result<Vec<(String, T)>, PlatformError> {
        let rows = self
            .blocking(move |db, scope| db.settings_with_prefix(scope, prefix))
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(key, value)| match serde_json::from_str(&value) {
                Ok(v) => Some((key, v)),
                Err(e) => {
                    tracing::warn!(%key, "an unreadable Platform-address record: {e}");
                    None
                }
            })
            .collect())
    }

    /// Writes `value` at `key` durably and reads it back.
    async fn put(&self, key: String, value: &impl Serialize) -> Result<(), PlatformError> {
        let json = serde_json::to_string(value).map_err(|e| PlatformError::Internal {
            detail: e.to_string(),
        })?;
        let read = self
            .blocking(move |db, scope| {
                db.set_setting_durable(scope, &key, Some(&json))?;
                Ok(db.setting(scope, &key)? == Some(json))
            })
            .await?;
        if read {
            Ok(())
        } else {
            Err(PlatformError::Storage {
                detail: "a Platform-address record did not read back".into(),
            })
        }
    }

    async fn delete(&self, key: String) -> Result<(), PlatformError> {
        self.blocking(move |db, scope| db.set_setting_durable(scope, &key, None))
            .await
    }

    async fn snapshot(&self) -> Result<Option<Snapshot>, PlatformError> {
        Ok(self
            .all(SNAPSHOT_KEY)
            .await?
            .into_iter()
            .next()
            .map(|(_, s)| s))
    }

    async fn moves(&self) -> Result<Vec<MoveRecord>, PlatformError> {
        Ok(self
            .all(MOVE_PREFIX)
            .await?
            .into_iter()
            .map(|(_, m)| m)
            .collect())
    }

    async fn put_move(&self, record: &MoveRecord) -> Result<(), PlatformError> {
        self.put(format!("{MOVE_PREFIX}{}", record.id), record)
            .await
    }
}

fn bech32m(address: &PlatformAddress, network: Network) -> String {
    address.to_bech32m_string(network)
}

/// The wallet's funded Platform addresses (bech32m → credits) as the last
/// sync pass left them.
async fn balances(wallet: &PlatformWallet, network: Network) -> Snapshot {
    wallet
        .platform()
        .addresses_with_balances()
        .await
        .into_iter()
        .filter(|(_, c)| *c > 0)
        .map(|(a, c)| (bech32m(&a, network), c))
        .collect()
}

/// `DashPay::platform_addresses`.
pub(crate) async fn balance(
    manager: &Manager,
    wallet: &PlatformWallet,
    network: Network,
) -> PlatformAddressBalance {
    let mut addresses: Vec<PlatformAddressRow> = balances(wallet, network)
        .await
        .into_iter()
        .map(|(address, credits)| PlatformAddressRow { address, credits })
        .collect();
    addresses.sort_by(|a, b| b.credits.cmp(&a.credits).then(a.address.cmp(&b.address)));
    let credits = addresses.iter().map(|r| r.credits).sum::<u64>();
    let sync = manager.platform_address_sync();
    PlatformAddressBalance {
        credits,
        duffs: credits / CREDITS_PER_DUFF,
        addresses,
        synced_at: sync.last_sync_unix_seconds(),
        syncing: sync.is_syncing(),
    }
}

/// `DashPay::sync_platform_addresses`: one pass for the wallet, or the end
/// of the one already running.
pub(crate) async fn sync(
    session: &NetworkSession,
    manager: &Manager,
    wallet_id: WalletId,
) -> Result<(), CreditsError> {
    if !session.platform.enabled() {
        return Err(PlatformError::FeatureOff {
            feature: "platform".into(),
        }
        .into());
    }
    let sync = manager.platform_address_sync_arc();
    let handle = tokio::runtime::Handle::current();
    let pass = Arc::clone(&sync);
    // The library's pass is not `Send`: it runs on the blocking pool.
    let ran = tokio::task::spawn_blocking(move || {
        handle
            .block_on(pass.sync_wallet(&wallet_id.0))
            .map(drop)
            .map_err(Box::new)
    })
    .await
    .map_err(|e| PlatformError::from(crate::EngineError::from(e)))?;
    match ran {
        Ok(()) => {}
        Err(e) if matches!(*e, PlatformWalletError::AddressSync(_)) && sync.is_syncing() => {
            let until = tokio::time::Instant::now() + PASS_WAIT;
            while sync.is_syncing() {
                if tokio::time::Instant::now() >= until {
                    return Err(PlatformError::Timeout.into());
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        Err(e) => return Err(PlatformError::from(*e).into()),
    }
    session
        .hub
        .pump
        .mark_platform(wallet_id, [PlatformChange::PlatformAddresses]);
    Ok(())
}

/// The inputs' proved nonces (bech32m → nonce; `None` for an address
/// Platform does not hold).
async fn nonces(
    sdk: &dash_sdk::Sdk,
    addresses: BTreeSet<PlatformAddress>,
    network: Network,
) -> Result<BTreeMap<String, Option<u32>>, PlatformError> {
    let infos = AddressInfo::fetch_many(sdk, addresses).await?;
    Ok(infos
        .into_iter()
        .map(|(a, info)| (bech32m(&a, network), info.map(|i| i.nonce)))
        .collect())
}

fn parse(address: &str) -> Option<PlatformAddress> {
    PlatformAddress::from_bech32m_string(address).ok()
}

/// The wallet's moves, a pending one settled where Platform can tell
/// (module doc); a changed record is stored.
async fn settle_moves(
    sdk: &dash_sdk::Sdk,
    store: &Store,
    network: Network,
) -> Result<(Vec<MoveRecord>, bool), PlatformError> {
    let mut moves = store.moves().await?;
    let pending: BTreeSet<PlatformAddress> = (moves.iter())
        .filter(|m| m.state == PlatformActivityState::Pending)
        .flat_map(|m| m.inputs.iter().filter_map(|i| parse(&i.address)))
        .collect();
    if pending.is_empty() {
        return Ok((moves, false));
    }
    let proved = match nonces(sdk, pending, network).await {
        Ok(proved) => proved,
        Err(e) => {
            // Nothing is known: the moves stay pending.
            tracing::info!("reading a pending move's nonces: {e}");
            return Ok((moves, false));
        }
    };
    let now = unix_now();
    let mut changed = false;
    for record in moves
        .iter_mut()
        .filter(|m| m.state == PlatformActivityState::Pending)
    {
        match settled(record, &proved, now) {
            PlatformActivityState::Pending => continue,
            PlatformActivityState::Done => record.done(),
            PlatformActivityState::Failed => record.state = PlatformActivityState::Failed,
        }
        store.put_move(record).await?;
        changed = true;
    }
    Ok((moves, changed))
}

/// `DashPay::platform_activity`.
pub(crate) async fn activity(
    session: &NetworkSession,
    manager: &Manager,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
) -> Result<Vec<PlatformActivity>, CreditsError> {
    let lock = wallet_lock(wallet_id);
    let _one = lock.lock().await;
    let network = session.vault.network();
    let store = Store::of(session, wallet_id)?;
    let (mut moves, mut changed) = settle_moves(&manager.sdk_arc(), &store, network).await?;
    let held: BTreeSet<String> = (moves.iter())
        .filter(|m| m.state == PlatformActivityState::Pending)
        .flat_map(|m| m.inputs.iter().map(|i| i.address.clone()))
        .collect();
    let mut unseen: BTreeMap<String, u64> = BTreeMap::new();
    for input in moves.iter().flat_map(|m| &m.inputs) {
        *unseen.entry(input.address.clone()).or_default() += input.unseen;
    }
    let before = store.snapshot().await?;
    let now = balances(wallet, network).await;
    let (rows, next) = changes(before.as_ref(), &now, &held, &mut unseen, unix_now());
    for row in &rows {
        let key = format!(
            "{ROW_PREFIX}{:020}/{}",
            row.at,
            row.address.as_deref().unwrap_or("")
        );
        store.put(key, row).await?;
    }
    // A done move's fall, once read, is spent.
    for record in &mut moves {
        let before = record.clone();
        for input in &mut record.inputs {
            if unseen.get(&input.address) == Some(&0) {
                input.unseen = 0;
            }
        }
        if *record != before {
            store.put_move(record).await?;
        }
    }
    if before.as_ref() != Some(&next) {
        store.put(SNAPSHOT_KEY.to_owned(), &next).await?;
    }
    changed |= !rows.is_empty();
    let mut stored = store.all::<PlatformActivity>(ROW_PREFIX).await?;
    // Keys sort by time: the oldest beyond the kept ones go.
    while stored.len() > ROWS_KEPT {
        let (key, _) = stored.remove(0);
        store.delete(key).await?;
    }
    if changed {
        session
            .hub
            .pump
            .mark_platform(wallet_id, [PlatformChange::PlatformAddresses]);
    }
    let mut out: Vec<PlatformActivity> = (stored.into_iter().map(|(_, r)| r))
        .chain(moves.iter().map(MoveRecord::activity))
        .collect();
    out.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| b.id.cmp(&a.id)));
    Ok(out)
}

/// What a move of `amount` takes, at the network's protocol version.
struct Planned {
    inputs: BTreeMap<PlatformAddress, u64>,
    fee_strategy: AddressFundsFeeStrategy,
    /// What reaches Core, in credits.
    credits: u64,
    fee: u64,
    expected_duffs: u64,
    version: &'static PlatformVersion,
}

async fn plan(
    sdk: &dash_sdk::Sdk,
    wallet: &PlatformWallet,
    amount: WithdrawAmount,
) -> Result<Planned, CreditsError> {
    let platform = wallet.platform();
    let total = platform.total_credits().await;
    let planned = platform
        .preflight_withdrawal(ACCOUNT, CORE_FEE_PER_BYTE)
        .await;
    // After the plan's proved read, which ratchets the SDK's version.
    let version = sdk.version();
    let rules = Rules::at(version)?;
    let short = |needed: u64| PlatformError::InsufficientCredits {
        needed,
        available: total,
    };
    let plan = match planned {
        Ok(plan) => plan,
        Err(
            PlatformWalletError::OnlyDustInputs { .. } | PlatformWalletError::AddressOperation(_),
        ) => {
            return Err(short(rules.min).into());
        }
        Err(e) => return Err(PlatformError::from(e).into()),
    };
    let fee = plan.estimated_fee;
    let all: u64 = plan.inputs.values().sum();
    let (inputs, fee_strategy, credits) = match amount {
        WithdrawAmount::All => {
            if all < rules.min {
                return Err(short(rules.min.saturating_add(fee)).into());
            }
            (plan.inputs.clone(), plan.fee_strategy.clone(), all)
        }
        WithdrawAmount::Credits { credits } => {
            let out_of_range = CreditsError::WithdrawalOutOfRange {
                min: rules.min,
                max: rules.max,
            };
            if credits < rules.min || credits > rules.max {
                return Err(out_of_range);
            }
            if credits > all {
                return Err(short(credits.saturating_add(fee)).into());
            }
            let min_input = platform.min_input_amount();
            let (inputs, strategy) = select(&plan, credits, min_input).ok_or(out_of_range)?;
            (inputs, strategy, credits)
        }
    };
    if credits > rules.max {
        return Err(CreditsError::WithdrawalOutOfRange {
            min: rules.min,
            max: rules.max,
        });
    }
    Ok(Planned {
        inputs,
        fee_strategy,
        credits,
        fee,
        expected_duffs: withdrawal::expected_duffs(credits, version)?,
        version,
    })
}

/// `DashPay::move_to_core_quote`.
pub(crate) async fn quote(
    session: &NetworkSession,
    manager: &Manager,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
    amount: WithdrawAmount,
) -> Result<MoveToCoreQuote, CreditsError> {
    let planned = plan(&manager.sdk_arc(), wallet, amount).await?;
    let to = session
        .current_receiving_address(wallet_id)
        .await
        .map_err(PlatformError::from)?;
    let credits = planned.credits.saturating_add(planned.fee);
    Ok(MoveToCoreQuote {
        credits,
        fee_credits: planned.fee,
        expected_duffs: planned.expected_duffs,
        to: to.to_string(),
        grant: GrantRequest {
            max_duffs: 0,
            max_credits: credits,
        },
    })
}

/// The DIP-17 path of each of `addresses` (HASH160 → path), from the
/// payment account's address pool. One the pool does not hold yet (before
/// the first sync pass after a restart) is `platform.unavailable`.
pub(crate) async fn input_paths<'a>(
    wallet: &PlatformWallet,
    addresses: impl IntoIterator<Item = &'a PlatformAddress>,
) -> Result<BTreeMap<[u8; 20], DerivationPath>, PlatformError> {
    let wm = wallet.wallet_manager().read().await;
    let info = wm
        .get_wallet_info(&wallet.wallet_id())
        .ok_or(PlatformError::WalletNotFound)?;
    let account = info
        .core_wallet
        .platform_payment_managed_account_at_index(ACCOUNT)
        .ok_or_else(|| PlatformError::Internal {
            detail: "the wallet has no Platform payment account".into(),
        })?;
    let pool: HashMap<[u8; 20], &DerivationPath> = (account.addresses.addresses.values())
        .filter_map(|a| {
            let p2pkh = PlatformP2PKHAddress::from_address(&a.address).ok()?;
            Some((p2pkh.to_bytes(), &a.path))
        })
        .collect();
    addresses
        .into_iter()
        .map(|address| match address {
            PlatformAddress::P2pkh(hash) => pool
                .get(hash)
                .map(|path| (*hash, (*path).clone()))
                .ok_or(PlatformError::Unavailable),
            PlatformAddress::P2sh(_) => Err(PlatformError::Internal {
                detail: "a P2SH Platform address is no payment address".into(),
            }),
        })
        .collect()
}

/// `DashPay::move_to_core` (module doc).
pub(crate) async fn move_to_core(
    session: &Arc<NetworkSession>,
    manager: &Manager,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
    amount: WithdrawAmount,
    grant: &str,
) -> Result<MoveToCoreOutcome, CreditsError> {
    let lock = wallet_lock(wallet_id);
    let _one = lock.lock().await;
    let network = session.vault.network();
    let sdk = manager.sdk_arc();
    let store = Store::of(session, wallet_id)?;
    let (moves, _) = settle_moves(&sdk, &store, network).await?;
    if let Some(pending) = moves
        .iter()
        .find(|m| m.state == PlatformActivityState::Pending)
    {
        return Err(CreditsError::MovePending {
            id: pending.id.clone(),
        });
    }
    let planned = plan(&sdk, wallet, amount).await?;
    let quote = planned.credits.saturating_add(planned.fee);
    // The authority before anything is redeemed: a refusal keeps the grant.
    let authority = match session.lease_for(&wallet_id, grant) {
        Some(lease) => {
            let lease = lease.map_err(PlatformError::from)?;
            if lease.flow() != FlowKind::PlatformAddressMove {
                return Err(PlatformError::GrantInvalid.into());
            }
            NameAuthority::Lease(lease)
        }
        None => NameAuthority::Grant(grant.to_owned()),
    };
    check_credit_cap(session, wallet_id, &authority, quote).await?;
    let to = session
        .current_receiving_address(wallet_id)
        .await
        .map_err(PlatformError::from)?
        .to_string();
    let script = withdrawal::destination(network, &to)?;
    let paths = input_paths(wallet, planned.inputs.keys()).await?;
    let proved = AddressInfo::fetch_many(
        sdk.as_ref(),
        planned.inputs.keys().copied().collect::<BTreeSet<_>>(),
    )
    .await
    .map_err(PlatformError::from)?;
    let mut inputs = BTreeMap::new();
    for (address, credits) in &planned.inputs {
        let info = proved.get(address).cloned().flatten();
        let available = info.as_ref().map_or(0, |i| i.balance);
        let Some(info) = info.filter(|i| i.balance >= *credits) else {
            return Err(PlatformError::InsufficientCredits {
                needed: *credits,
                available,
            }
            .into());
        };
        inputs.insert(*address, (info.nonce.saturating_add(1), *credits));
    }
    let lease = match authority {
        NameAuthority::Lease(lease) => lease,
        NameAuthority::Grant(grant) => session
            .begin_lease(wallet_id, FlowKind::PlatformAddressMove, &[grant])
            .await
            .map_err(PlatformError::from)?,
    };
    let permit = write_permit(&lease, "move_to_core", quote)?;
    let signer = match permit.platform_address_signer(paths) {
        Ok(signer) => signer,
        Err(e) => {
            permit.finish(false);
            return Err(PlatformError::from(e).into());
        }
    };
    let first = inputs
        .iter()
        .next()
        .map(|(a, (n, _))| (bech32m(a, network), *n));
    let (first_address, first_nonce) = first.unwrap_or_default();
    let mut record = MoveRecord {
        id: format!("move-{first_address}-{first_nonce}"),
        inputs: (inputs.iter())
            .map(|(a, (nonce, credits))| MoveInput {
                address: bech32m(a, network),
                nonce: *nonce,
                credits: *credits,
                unseen: 0,
            })
            .collect(),
        credits: quote,
        fee_credits: planned.fee,
        expected_duffs: planned.expected_duffs,
        to: to.clone(),
        at: unix_now(),
        state: PlatformActivityState::Pending,
    };
    // Recorded and read back before anything is signed.
    if let Err(e) = store.put_move(&record).await {
        drop(signer);
        permit.finish(false);
        return Err(e.into());
    }
    session
        .hub
        .pump
        .mark_platform(wallet_id, [PlatformChange::PlatformAddresses]);
    let sent = leased(
        &lease,
        wallet.platform().withdraw(
            ACCOUNT,
            InputSelection::ExplicitWithNonces(inputs),
            script,
            CORE_FEE_PER_BYTE,
            planned.fee_strategy,
            Some(planned.version),
            &signer,
        ),
    )
    .await;
    let signed = permit.signatures().released() > 0;
    drop(signer);
    permit.finish(sent.is_ok());
    let failure = match sent {
        Ok(_) => {
            record.done();
            None
        }
        Err(e) if !signed => {
            record.state = PlatformActivityState::Failed;
            Some(e)
        }
        Err(e) => {
            tracing::info!(id = %record.id, "a move's broadcast failed: {e}");
            let addresses = record
                .inputs
                .iter()
                .filter_map(|i| parse(&i.address))
                .collect();
            match nonces(&sdk, addresses, network).await {
                Ok(proved)
                    if settled(&record, &proved, record.at) == PlatformActivityState::Done =>
                {
                    record.done();
                }
                read => {
                    if let Err(again) = read {
                        tracing::info!(id = %record.id, "reading the move's nonces: {again}");
                    }
                }
            }
            None
        }
    };
    if let Err(e) = store.put_move(&record).await {
        // The pending record stands and settles on a later read.
        tracing::warn!(id = %record.id, "storing a move's outcome: {e}");
    }
    session
        .hub
        .pump
        .mark_platform(wallet_id, [PlatformChange::PlatformAddresses]);
    if let Some(e) = failure {
        return Err(PlatformError::from(e).into());
    }
    Ok(MoveToCoreOutcome {
        id: record.id,
        credits: record.credits,
        expected_duffs: record.expected_duffs,
        to,
        state: record.state,
    })
}

impl NetworkSession {
    /// **Test only** (dwcli `platform-addr fund`, the release-0.1 testnet
    /// smoke; the app never calls it, and it is no facade call): funds the
    /// wallet's first Platform payment address from its Core balance with
    /// an asset lock of `duffs`. `address_grant` is a `PlatformOp` grant
    /// with credits (the address's signer), `spend_grant` a `Spend` grant
    /// (the asset lock's inputs and credit key, which no Platform scope
    /// reaches). No lease, no fence: a lock broadcast and not consumed is
    /// left to the library's tracked asset locks.
    #[doc(hidden)]
    pub async fn fund_platform_addresses_for_test(
        self: &Arc<Self>,
        wallet_id: WalletId,
        duffs: u64,
        address_grant: String,
        spend_grant: String,
    ) -> Result<PlatformAddressBalance, CreditsError> {
        self.dashpay(wallet_id)
            .on_wallet(move |session, manager, wallet| async move {
                let vault = session.vault.clone();
                let w = wallet_id.0;
                let (address_signer, lock_signer) = tokio::task::spawn_blocking(move || {
                    let token = vault.redeem_grant(
                        &address_grant,
                        dw_vault::GrantKind::PlatformOp,
                        Some(&w),
                    )?;
                    let address = vault.platform_signer(
                        &w,
                        &token,
                        dw_vault::SignerScope::PlatformAddress,
                    )?;
                    let token =
                        vault.redeem_grant(&spend_grant, dw_vault::GrantKind::Spend, Some(&w))?;
                    Ok::<_, dw_vault::VaultError>((address, vault.signer(&w, &token)?))
                })
                .await
                .map_err(|e| PlatformError::from(crate::EngineError::from(e)))?
                .map_err(PlatformError::from)?;
                let (hash, path) = {
                    let wm = wallet.wallet_manager().read().await;
                    let info = (wm.get_wallet_info(&wallet.wallet_id()))
                        .ok_or(PlatformError::WalletNotFound)?;
                    let account = (info.core_wallet)
                        .platform_payment_managed_account_at_index(ACCOUNT)
                        .ok_or(PlatformError::Unavailable)?;
                    let first = (account.addresses.addresses.values().next())
                        .ok_or(PlatformError::Unavailable)?;
                    let p2pkh =
                        PlatformP2PKHAddress::from_address(&first.address).map_err(|e| {
                            PlatformError::Internal {
                                detail: e.to_string(),
                            }
                        })?;
                    (p2pkh.to_bytes(), first.path.clone())
                };
                let signer = super::signers::VaultPlatformAddressSigner::new(
                    address_signer,
                    [(hash, path)].into(),
                    Arc::new(super::lease::SignatureCount::default()),
                )
                .map_err(PlatformError::from)?;
                wallet
                    .platform()
                    .fund_from_asset_lock(
                        platform_wallet::AssetLockFunding::FromWalletBalance {
                            amount_duffs: duffs,
                            account_index: 0,
                        },
                        ACCOUNT,
                        [(PlatformAddress::P2pkh(hash), None)].into(),
                        Vec::new(),
                        &signer,
                        &lock_signer,
                        None,
                    )
                    .await
                    .map_err(PlatformError::from)?;
                session
                    .hub
                    .pump
                    .mark_platform(wallet_id, [PlatformChange::PlatformAddresses]);
                Ok(balance(&manager, &wallet, session.vault.network()).await)
            })
            .await
    }
}

#[cfg(test)]
#[path = "platform_moves_tests.rs"]
mod tests;
