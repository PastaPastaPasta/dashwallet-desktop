//! Tests of shielded_pool.rs (0.1 E1): the protocol-version gate, the
//! status and history read from the library, the quotes' fees and
//! refusals, and the spends on requests_tests.rs's fixture over a scripted
//! library ([`FakeShielded`]): what a spend charges its lease, which
//! refusals keep the grant, and how the library's failures map. Then the
//! real library on topup_tests.rs's fixture: the viewing key binds from the
//! vault once and from storage after a restart with the vault locked, and
//! the facade shows the mobile wallets' address for "abandon … about".
//! The testnet round trip is the smoke (`/work/scratch/dw-rel/e1/smoke.md`).

use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dashcore::address::Payload;
use dashcore::hashes::Hash;
use dashcore::{Address, Network, OutPoint, PubkeyHash, Txid};
use dpp::address_funds::{OrchardAddress, PlatformAddress};
use dpp::version::PlatformVersion;
use dw_appdb::dispatch::TopUpSettled;
use dw_vault::{VaultError, VaultSigner};
use platform_wallet::PlatformWalletError;
use platform_wallet::wallet::shielded::note_selection::{ShieldedFeeKind, select_notes_with_fee};
use platform_wallet::wallet::shielded::{
    OrchardKeySet, ShieldedActivityEntry, ShieldedActivityKind as Kind,
    ShieldedActivityStatus as Status, ShieldedDirection as Direction, ShieldedNote,
};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::errors::{PlatformError, ShieldedError};
use super::flows::FlowKind;
use super::lease::{CallPermit, FundingKey, Lease};
use super::registration::RegistrationWait;
use super::registration_flow::{FundError, LockInfo, LockStatus, Spendable};
use super::requests::tests::Fixture;
use super::shielded::{
    SHIELDED_ACTIVATION_VERSION, ShieldState, ShieldedActivityKind, ShieldedActivityStatus,
    ShieldedDirection, ShieldedSpendState, UnresolvedSpend,
};
use super::shielded_pool::{
    self as pool, Applied, Ledger, MAX_LOCK_INPUTS, ShieldedLib, SpendError, SyncState, on_pool,
};
use super::withdrawal::{Rules, expected_duffs};
use crate::{DashNetwork, ImportOptions, NetworkSession};

const LATEST: u32 = 15;
const NOTE: u64 = 300_000_000;
/// Lane V's reference: iOS and Android show this shielded address for
/// "abandon … about" on mainnet.
const ABANDON_MAINNET: &str =
    "dash1zrlpvqcgyx2rag5d5rkhq0wy69hz3q37xe54xlrreqgp8htpv3gmm2l52gznxha0fd3kq9s4l67rj";

/// A shielded address of an unrelated regtest wallet.
fn payee() -> OrchardAddress {
    let keys = OrchardKeySet::from_seed(&[7; 64], Network::Regtest, 0).unwrap();
    let raw = keys.viewing_keys().default_address.to_raw_address_bytes();
    OrchardAddress::from_raw_bytes(&raw).unwrap()
}

fn payee_text() -> String {
    payee().to_bech32m_string(Network::Regtest)
}

fn core_address(n: u8) -> Address {
    Address::new(
        Network::Regtest,
        Payload::PubkeyHash(PubkeyHash::from_byte_array([n; 20])),
    )
}

fn note(position: u64, value: u64) -> ShieldedNote {
    ShieldedNote {
        position,
        cmx: [position as u8; 32],
        nullifier: [position as u8 + 100; 32],
        block_height: 1,
        is_spent: false,
        value,
        note_data: Vec::new(),
    }
}

fn entry(kind: Kind, amount: u64, status: Status) -> ShieldedActivityEntry {
    ShieldedActivityEntry {
        id: [amount as u8; 32],
        kind,
        direction: Direction::Out,
        amount,
        fee: None,
        counterparty: None,
        memo: None,
        block_height: None,
        status,
        created_at_ms: 0,
        min_note_position: None,
        note_cmxs: Vec::new(),
        spent_nullifiers: Vec::new(),
    }
}

fn lock(n: u8, status: LockStatus) -> LockInfo {
    LockInfo {
        outpoint: OutPoint::new(Txid::from_byte_array([n; 32]), 0),
        registration: false,
        identity_index: 0,
        account: 0,
        amount: 1_000_000 * u64::from(n),
        status,
    }
}

/// What a spend handed the library.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Spent {
    operation: &'static str,
    to: String,
    credits: u64,
    memo: Vec<u8>,
    /// The full viewing key of the key set the lease released.
    fvk: [u8; 96],
}

/// platform-wallet's shielded side, scripted.
struct FakeShielded {
    version: u32,
    /// What a proved read of the version returns.
    refreshed: u32,
    refreshes: AtomicU32,
    bound: bool,
    ledger: Option<Ledger>,
    notes: Vec<ShieldedNote>,
    activity: Vec<ShieldedActivityEntry>,
    locks: Vec<LockInfo>,
    /// The locks the wallet's shield steps signed; `None` while the
    /// journal is unread.
    own: Option<HashSet<Txid>>,
    /// The locks the journal records settled.
    settled: Mutex<HashSet<Txid>>,
    /// Every settle, in order.
    settles: Mutex<Vec<(Txid, TopUpSettled)>>,
    /// The next apply's answer.
    applies: Mutex<Option<Result<Applied, PlatformError>>>,
    syncs: AtomicU32,
    spendable: Spendable,
    /// The coins a funding may select.
    coins: Vec<u64>,
    receive: String,
    /// The next spend's failure, after the keys are out.
    fails: Mutex<Option<PlatformWalletError>>,
    spent: Mutex<Vec<Spent>>,
    /// As the library keeps them: the records of unconfirmed spends, and
    /// their activity rows (shown before `activity`).
    unresolved: Mutex<Vec<UnresolvedSpend>>,
    /// What the engine recorded in the journal.
    recorded: Mutex<Vec<UnresolvedSpend>>,
    rows: Mutex<Vec<ShieldedActivityEntry>>,
    /// What the next pass finds of the unconfirmed spends: `Confirmed`
    /// (their notes spent) or `Failed` (the prune backstop released them).
    scan_finds: Mutex<Option<Status>>,
    /// Platform's unproved "already consumed" answers this session.
    taken: Mutex<HashSet<Txid>>,
    /// Asked during each spend's library call; its answers, in order.
    probe: Mutex<Option<Box<dyn Fn() -> bool + Send>>>,
    probed: Mutex<Vec<bool>>,
}

impl Default for FakeShielded {
    fn default() -> Self {
        Self {
            version: LATEST,
            refreshed: LATEST,
            refreshes: AtomicU32::new(0),
            bound: true,
            ledger: Some(Ledger {
                spendable: 2 * NOTE,
                synced_index: Some(42),
            }),
            notes: vec![note(1, NOTE), note(2, NOTE)],
            activity: Vec::new(),
            locks: Vec::new(),
            own: Some(HashSet::new()),
            settled: Mutex::default(),
            settles: Mutex::default(),
            applies: Mutex::new(None),
            syncs: AtomicU32::new(0),
            spendable: Spendable {
                duffs: 10_000_000_000,
                inputs: 1,
                protected: 0,
            },
            coins: vec![10_000_000_000],
            receive: core_address(9).to_string(),
            fails: Mutex::new(None),
            spent: Mutex::new(Vec::new()),
            unresolved: Mutex::default(),
            recorded: Mutex::default(),
            rows: Mutex::default(),
            scan_finds: Mutex::new(None),
            taken: Mutex::default(),
            probe: Mutex::new(None),
            probed: Mutex::default(),
        }
    }
}

impl FakeShielded {
    fn at(version: u32, refreshed: u32) -> Self {
        Self {
            version,
            refreshed,
            ..Self::default()
        }
    }

    fn spent(&self) -> Vec<Spent> {
        self.spent.lock().unwrap().clone()
    }

    fn fail_with(&self, e: PlatformWalletError) {
        *self.fails.lock().unwrap() = Some(e);
    }

    fn spend(
        &self,
        permit: &CallPermit,
        operation: &'static str,
        to: String,
        credits: u64,
        memo: &[u8],
    ) -> Result<(), SpendError> {
        let keys = permit.shielded_keys()?;
        if let Some(probe) = &*self.probe.lock().unwrap() {
            self.probed.lock().unwrap().push(probe());
        }
        // The 0-based index of this spend.
        let n = self.spent.lock().unwrap().len() as u8;
        self.spent.lock().unwrap().push(Spent {
            operation,
            to,
            credits,
            memo: memo.to_vec(),
            fvk: keys.viewing_keys().to_fvk_bytes(),
        });
        match self.fails.lock().unwrap().take() {
            Some(e) => {
                // The library arms its re-drive record and keeps the row
                // pending (`arm_redrive_record`).
                if matches!(e, PlatformWalletError::ShieldedSpendUnconfirmed { .. }) {
                    self.unresolved.lock().unwrap().push(unresolved(n));
                    let mut row = entry(Kind::Sent, credits, Status::Pending);
                    row.id = [n; 32];
                    self.rows.lock().unwrap().push(row);
                }
                Err(SpendError::Library(Box::new(e)))
            }
            None => Ok(()),
        }
    }
}

/// The library's record of the `n`th spend, unconfirmed.
fn unresolved(n: u8) -> UnresolvedSpend {
    UnresolvedSpend {
        activity_id: hex::encode([n; 32]),
        transition: hex::encode(Sha256::digest([n; 8])),
        attempts: 0,
    }
}

impl ShieldedLib for FakeShielded {
    fn network(&self) -> Network {
        Network::Regtest
    }

    fn protocol_version(&self) -> u32 {
        self.version
    }

    async fn refresh_protocol_version(&self) -> u32 {
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        self.refreshed
    }

    async fn bind(&self) -> Result<bool, PlatformError> {
        Ok(self.bound)
    }

    async fn address(&self) -> Option<[u8; 43]> {
        self.bound.then(|| payee().to_raw_bytes())
    }

    async fn ledger(&self) -> Result<Option<Ledger>, PlatformError> {
        Ok(self.ledger)
    }

    async fn unspent(&self) -> Result<Vec<ShieldedNote>, PlatformError> {
        Ok(self.notes.clone())
    }

    async fn activity(
        &self,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<ShieldedActivityEntry>, PlatformError> {
        let rows = self.rows.lock().unwrap().clone();
        Ok((rows.iter().chain(&self.activity))
            .skip(offset as usize)
            .take(limit as usize)
            .cloned()
            .collect())
    }

    async fn sync_now(&self) -> bool {
        self.syncs.fetch_add(1, Ordering::SeqCst);
        if let Some(found) = self.scan_finds.lock().unwrap().take() {
            self.unresolved.lock().unwrap().clear();
            for row in self.rows.lock().unwrap().iter_mut() {
                row.status = found;
            }
        }
        true
    }

    fn sync_state(&self) -> SyncState {
        SyncState {
            syncing: false,
            last_sync_at: Some(1_700_000_000),
        }
    }

    fn prover_ready(&self) -> bool {
        true
    }

    async fn transfer(
        &self,
        permit: &CallPermit,
        to: &OrchardAddress,
        credits: u64,
        memo: [u8; 36],
    ) -> Result<(), SpendError> {
        let to = to.to_bech32m_string(Network::Regtest);
        self.spend(permit, "transfer", to, credits, &memo)
    }

    async fn withdraw(
        &self,
        permit: &CallPermit,
        to: &Address,
        credits: u64,
    ) -> Result<(), SpendError> {
        self.spend(permit, "withdraw", to.to_string(), credits, &[])
    }

    async fn receive_address(&self) -> Result<String, PlatformError> {
        Ok(self.receive.clone())
    }

    async fn shield_locks(&self) -> Vec<LockInfo> {
        self.locks.clone()
    }

    fn own_locks(&self) -> Option<HashSet<Txid>> {
        self.own.clone()
    }

    fn taken_locks(&self) -> HashSet<Txid> {
        self.taken.lock().unwrap().clone()
    }

    fn mark_taken(&self, txid: Txid) {
        self.taken.lock().unwrap().insert(txid);
    }

    async fn settled_locks(&self) -> HashSet<Txid> {
        self.settled.lock().unwrap().clone()
    }

    async fn settle(&self, txid: Txid, how: TopUpSettled) -> Result<(), PlatformError> {
        self.settles.lock().unwrap().push((txid, how));
        self.settled.lock().unwrap().insert(txid);
        Ok(())
    }

    async fn record(&self, spend: &UnresolvedSpend) -> Result<(), PlatformError> {
        self.recorded.lock().unwrap().push(spend.clone());
        Ok(())
    }

    async fn unresolved(&self) -> Result<Vec<UnresolvedSpend>, PlatformError> {
        Ok(self.unresolved.lock().unwrap().clone())
    }

    async fn coin_values(&self) -> Result<Vec<u64>, PlatformError> {
        Ok(self.coins.clone())
    }

    async fn spendable(&self) -> Result<Spendable, PlatformError> {
        Ok(self.spendable)
    }

    async fn fee_bound(&self) -> Result<u64, PlatformError> {
        Ok(1_000)
    }

    fn funding_hold(&self) -> Option<RegistrationWait> {
        None
    }

    async fn funding_permit(
        &self,
        _: &Lease,
        _: String,
        _: FundingKey,
        _: u64,
    ) -> Result<CallPermit, FundError> {
        unreachable!("no test funds a shield")
    }

    async fn fund(&self, _: u64, _: &CallPermit) -> Result<OutPoint, FundError> {
        unreachable!("no test funds a shield")
    }

    async fn wait_proof(&self, _: OutPoint, _: Duration) -> Result<(), FundError> {
        Err(FundError::Refused(PlatformError::Unavailable))
    }

    async fn apply(&self, _: OutPoint, _: &VaultSigner) -> Result<Applied, PlatformError> {
        (self.applies.lock().unwrap().take()).expect("a scripted apply")
    }
}

fn run<T: Send + 'static, Fut>(
    fx: &Fixture,
    lib: &Arc<FakeShielded>,
    body: impl FnOnce(Arc<NetworkSession>, Arc<FakeShielded>) -> Fut,
) -> T
where
    Fut: std::future::Future<Output = T> + Send + 'static,
{
    let fut = body(Arc::clone(&fx.s), Arc::clone(lib));
    fx.engine.block_on(fut)
}

fn code<T: std::fmt::Debug>(r: Result<T, ShieldedError>) -> &'static str {
    r.unwrap_err().code()
}

fn version(v: u32) -> &'static PlatformVersion {
    PlatformVersion::get(v).unwrap()
}

/// The library's fee for a spend of `credits` from the default notes.
fn fee_of(credits: u64, kind: ShieldedFeeKind) -> u64 {
    let notes = FakeShielded::default().notes;
    select_notes_with_fee(&notes, credits, 2, kind, version(LATEST))
        .unwrap()
        .2
}

#[test]
fn the_activation_is_platforms_and_testnet_and_mainnet_are_past_it() {
    assert_eq!(SHIELDED_ACTIVATION_VERSION, 12);
    // The SDK's floor for both (`dash_sdk::sdk::min_protocol_version`).
    for network in [Network::Mainnet, Network::Testnet] {
        assert!(dash_sdk::sdk::min_protocol_version(network) >= SHIELDED_ACTIVATION_VERSION);
    }
}

/// Below the activation every call but the status refuses
/// `shielded.not_active` after one proved read; a proved version at or
/// above it opens them; a held version above it reads nothing.
#[test]
fn the_gate_refuses_below_the_activation_after_a_proved_read() {
    let fx = Fixture::new(0);
    let below = Arc::new(FakeShielded::at(11, 11));
    let status = run(&fx, &below, |_, lib| async move {
        pool::status(&*lib).await.unwrap()
    });
    assert!(!status.active);
    assert_eq!(
        (status.protocol_version, status.activation_version),
        (11, 12)
    );
    assert!(!status.ready && status.balance == 0 && status.pending_shields.is_empty());
    assert_eq!(
        below.refreshes.load(Ordering::SeqCst),
        0,
        "the status reads none"
    );

    let refused = run(&fx, &below, |s, lib| async move {
        let lib = &*lib;
        vec![
            code(pool::receive_address(lib).await),
            code(pool::history(lib, 0, 10).await),
            code(pool::send_quote(lib, &payee_text(), 1).await),
            code(pool::unshield_quote(lib, 1, None).await),
            code(pool::shield_quote(&s, lib, 1).await),
            code(pool::sync(&s, lib).await),
        ]
    });
    assert!(
        refused.iter().all(|c| *c == "shielded.not_active"),
        "{refused:?}"
    );
    assert_eq!(below.refreshes.load(Ordering::SeqCst), 6);

    let proved = Arc::new(FakeShielded::at(11, 12));
    run(&fx, &proved, |_, lib| async move {
        pool::receive_address(&*lib).await.unwrap()
    });
    assert_eq!(proved.refreshes.load(Ordering::SeqCst), 1);

    let held = Arc::new(FakeShielded::at(12, 0));
    run(&fx, &held, |_, lib| async move {
        pool::receive_address(&*lib).await.unwrap()
    });
    assert_eq!(held.refreshes.load(Ordering::SeqCst), 0);
}

#[test]
fn the_status_reads_the_ledger_pending_rows_and_open_shields() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded {
        activity: vec![
            entry(Kind::Sent, 100, Status::Pending),
            entry(Kind::Unshield, 50, Status::Pending),
            entry(Kind::Received, 999, Status::Confirmed),
            entry(Kind::Sent, 7, Status::Pending),
        ],
        locks: vec![
            lock(3, LockStatus::Broadcast),
            lock(1, LockStatus::InstantSendLocked),
            lock(2, LockStatus::Consumed),
            lock(4, LockStatus::RecoveredFromChain),
            // Applied in an earlier session, back from SPV's rescan.
            lock(5, LockStatus::ChainLocked),
        ],
        settled: Mutex::new(HashSet::from([lock(5, LockStatus::ChainLocked)
            .outpoint
            .txid])),
        ..FakeShielded::default()
    });
    let status = run(&fx, &lib, |_, lib| async move {
        pool::status(&*lib).await.unwrap()
    });
    assert!(status.active && status.ready);
    assert_eq!(status.balance, 2 * NOTE);
    assert_eq!(status.synced_index, Some(42));
    // The leading pending rows: the sort puts pending first.
    assert_eq!(status.pending, 150);
    assert_eq!(status.last_sync_at, Some(1_700_000_000));
    assert!(status.prover_ready && !status.syncing);
    let shields: Vec<_> = (status.pending_shields.iter())
        .map(|p| (p.duffs, p.proved))
        .collect();
    assert_eq!(shields, [(1_000_000, true), (3_000_000, false)]);

    // Unbound, or bound with nothing restored or scanned: not a reading.
    for lib in [
        FakeShielded {
            bound: false,
            ..FakeShielded::default()
        },
        FakeShielded {
            ledger: None,
            ..FakeShielded::default()
        },
    ] {
        let lib = Arc::new(lib);
        let status = run(&fx, &lib, |_, lib| async move {
            pool::status(&*lib).await.unwrap()
        });
        assert!(status.active && !status.ready);
        assert_eq!((status.balance, status.synced_index), (0, None));
        let refused = run(&fx, &lib, |_, lib| async move {
            code(pool::send_quote(&*lib, &payee_text(), 1).await)
        });
        assert_eq!(refused, "shielded.not_synced");
    }
}

#[test]
fn the_receive_address_is_the_accounts_bech32m_address() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let address = run(&fx, &lib, |_, lib| async move {
        pool::receive_address(&*lib).await.unwrap()
    });
    assert_eq!(address.address, payee_text());
    assert!(address.address.starts_with("tdash1z"));
    assert_eq!(address.account, 0);
}

#[test]
fn a_send_quote_takes_the_fee_of_the_notes_it_would_select() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let quote = run(&fx, &lib, |_, lib| async move {
        pool::send_quote(&*lib, &format!(" {} ", payee_text()), NOTE).await
    })
    .unwrap();
    let fee = fee_of(NOTE, ShieldedFeeKind::Base);
    assert!(fee > 0);
    assert_eq!(
        (quote.credits, quote.fee_credits, quote.total_credits),
        (NOTE, fee, NOTE + fee)
    );
    assert_eq!(quote.expected_duffs, None);
    assert_eq!(
        (quote.grant.max_duffs, quote.grant.max_credits),
        (0, NOTE + fee)
    );

    let refused = run(&fx, &lib, |_, lib| async move {
        let lib = &*lib;
        vec![
            // Another network's, a Core address, not an address.
            pool::send_quote(lib, ABANDON_MAINNET, 1).await.unwrap_err(),
            pool::send_quote(lib, &core_address(1).to_string(), 1)
                .await
                .unwrap_err(),
            pool::send_quote(lib, "tdash1zxyz", 1).await.unwrap_err(),
            pool::send_quote(lib, &payee_text(), 0).await.unwrap_err(),
            pool::send_quote(lib, &payee_text(), 2 * NOTE)
                .await
                .unwrap_err(),
        ]
    });
    let codes: Vec<_> = refused.iter().map(ShieldedError::code).collect();
    assert_eq!(
        codes,
        [
            "shielded.invalid_address",
            "shielded.invalid_address",
            "shielded.invalid_address",
            "shielded.below_minimum",
            "shielded.insufficient_funds",
        ]
    );
    let needed = 2 * NOTE + fee_of(NOTE, ShieldedFeeKind::Base);
    assert_eq!(
        refused[4],
        ShieldedError::InsufficientFunds {
            needed,
            available: 2 * NOTE,
        }
    );
}

#[test]
fn an_unshield_quote_follows_platforms_withdrawal_rules() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let rules = Rules::at(version(LATEST)).unwrap();
    let credits = rules.min.max(NOTE / 2);
    let quote = run(&fx, &lib, move |_, lib| async move {
        pool::unshield_quote(&*lib, credits, None).await
    })
    .unwrap();
    let fee = fee_of(credits, ShieldedFeeKind::Withdrawal);
    assert_eq!(
        (quote.fee_credits, quote.total_credits),
        (fee, credits + fee)
    );
    assert_eq!(
        quote.expected_duffs,
        Some(expected_duffs(credits, version(LATEST)).unwrap())
    );
    assert!(fee > fee_of(credits, ShieldedFeeKind::Base));

    let refused = run(&fx, &lib, move |_, lib| async move {
        let lib = &*lib;
        let mainnet = Address::new(
            Network::Mainnet,
            Payload::PubkeyHash(PubkeyHash::from_byte_array([1; 20])),
        );
        vec![
            code(pool::unshield_quote(lib, rules.min - 1, None).await),
            code(pool::unshield_quote(lib, rules.max + 1, None).await),
            code(pool::unshield_quote(lib, credits, Some(&mainnet.to_string())).await),
            code(pool::unshield_quote(lib, credits, Some(&payee_text())).await),
        ]
    });
    assert_eq!(
        refused,
        [
            "shielded.below_minimum",
            "invalid_argument",
            "shielded.invalid_address",
            "shielded.invalid_address",
        ]
    );
}

/// A send charges its quote to the lease and hands the library the key set
/// of the wallet's account 0; every refusal before the keys keeps the
/// grant and reaches no library spend.
#[test]
fn a_send_charges_its_quote_and_a_refusal_keeps_the_grant() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let wallet = fx.wallet;
    let grant = fx.grant(10 * NOTE);
    for (to, credits, memo) in [
        (payee_text(), NOTE, Some("x".repeat(37))),
        (ABANDON_MAINNET.to_owned(), NOTE, None),
        (payee_text(), 0, None),
        (payee_text(), 2 * NOTE, None),
    ] {
        let g = grant.clone();
        let refused = run(&fx, &lib, move |s, lib| async move {
            pool::send(&s, wallet, &*lib, (&to, credits), memo.as_deref(), &g).await
        });
        assert!(refused.is_err());
        assert!(fx.unused(&grant));
    }
    assert!(lib.spent().is_empty());

    let lease = fx.flow(FlowKind::ShieldedSpend, vec![grant]);
    let l = lease.clone();
    let sent = run(&fx, &lib, move |s, lib| async move {
        let to = format!("{} ", payee_text());
        pool::send(&s, wallet, &*lib, (&to, NOTE), Some("thanks"), &l).await
    })
    .unwrap();
    assert_eq!((sent.credits, sent.balance), (NOTE, 2 * NOTE));
    assert_eq!(
        (sent.state, sent.unresolved),
        (ShieldedSpendState::Done, None)
    );
    // The balance after is read after a pass finds the change.
    assert_eq!(lib.syncs.load(Ordering::SeqCst), 1);
    assert_eq!(sent.to, payee_text());
    let fee = fee_of(NOTE, ShieldedFeeKind::Base);
    assert_eq!(fx.credits_spent(&lease), NOTE + fee);
    let fvk = fx.s.vault.shielded_viewing_key(&wallet.0).unwrap();
    let mut memo = b"thanks".to_vec();
    memo.resize(36, 0);
    assert_eq!(
        lib.spent(),
        [Spent {
            operation: "transfer",
            to: payee_text(),
            credits: NOTE,
            memo,
            fvk,
        }]
    );

    // Another flow's lease is not a shielded spend's.
    let other = fx.flow(FlowKind::Withdraw, vec![fx.grant(NOTE)]);
    let refused = run(&fx, &lib, move |s, lib| async move {
        code(pool::send(&s, wallet, &*lib, (&payee_text(), 1), None, &other).await)
    });
    assert_eq!(refused, "platform.grant_invalid");
}

/// Once the key set is out the quote stays charged whatever the library
/// returns; its failures map to the facade's kinds.
#[test]
fn a_failed_spend_keeps_its_quote_and_maps_the_librarys_error() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let wallet = fx.wallet;
    let rules = Rules::at(version(LATEST)).unwrap();
    let credits = rules.min.max(NOTE / 2);
    let pool_text = "state transition broadcast failed: Shielded pool has insufficient notes \
                     for outgoing transition: pool has 3 notes but minimum 250 required";
    let cases: [(PlatformWalletError, bool, ShieldedError); 3] = [
        (
            PlatformWalletError::ShieldedBroadcastFailed(pool_text.into()),
            true,
            ShieldedError::PoolTooSmall {
                notes: 3,
                required: 250,
            },
        ),
        (
            PlatformWalletError::ShieldedNoRecordedAnchor("tree moved".into()),
            false,
            ShieldedError::NotSynced,
        ),
        (
            PlatformWalletError::ShieldedInsufficientBalance {
                available: 5,
                required: 9,
            },
            true,
            ShieldedError::InsufficientFunds {
                needed: 9,
                available: 5,
            },
        ),
    ];
    for (failure, unshield, want) in cases {
        lib.fail_with(failure);
        let lease = fx.flow(FlowKind::ShieldedSpend, vec![fx.grant(10 * NOTE)]);
        let l = lease.clone();
        let got = run(&fx, &lib, move |s, lib| async move {
            match unshield {
                true => pool::unshield(&s, wallet, &*lib, credits, None, &l)
                    .await
                    .map(drop),
                false => pool::send(&s, wallet, &*lib, (&payee_text(), credits), None, &l)
                    .await
                    .map(drop),
            }
        });
        assert_eq!(got.unwrap_err(), want);
        let kind = match unshield {
            true => ShieldedFeeKind::Withdrawal,
            false => ShieldedFeeKind::Base,
        };
        assert_eq!(fx.credits_spent(&lease), credits + fee_of(credits, kind));
    }
    // Unconfirmed text is not the pool's floor.
    assert_eq!(
        pool::spend_error(PlatformWalletError::ShieldedBroadcastFailed(
            "rejected".into()
        ))
        .code(),
        PlatformError::from(PlatformWalletError::ShieldedBroadcastFailed(
            "rejected".into()
        ))
        .code()
    );
}

#[test]
fn an_unshield_pays_the_address_given_or_a_fresh_receive_address() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let wallet = fx.wallet;
    let credits = Rules::at(version(LATEST)).unwrap().min.max(NOTE / 2);
    for to in [None, Some(core_address(3).to_string())] {
        let lease = fx.flow(FlowKind::ShieldedSpend, vec![fx.grant(10 * NOTE)]);
        let t = to.clone();
        let out = run(&fx, &lib, move |s, lib| async move {
            pool::unshield(&s, wallet, &*lib, credits, t.as_deref(), &lease).await
        })
        .unwrap();
        let want = to.unwrap_or_else(|| lib.receive.clone());
        assert_eq!(out.to, want);
        let last = lib.spent().pop().unwrap();
        assert_eq!(
            (last.operation, last.to, last.credits),
            ("withdraw", want, credits)
        );
    }
}

#[test]
fn history_shows_the_librarys_rows() {
    let fx = Fixture::new(0);
    let mut memo = b"lunch".to_vec();
    memo.resize(36, 0);
    let platform = PlatformAddress::P2pkh([4; 20]);
    let rows = vec![
        ShieldedActivityEntry {
            counterparty: Some(payee().to_raw_bytes().to_vec()),
            memo: Some(memo),
            fee: Some(20),
            created_at_ms: 1_700_000_000_999,
            ..entry(Kind::Sent, 100, Status::Pending)
        },
        ShieldedActivityEntry {
            counterparty: Some(core_address(5).script_pubkey().to_bytes()),
            block_height: Some(77),
            ..entry(Kind::Withdrawal, 200, Status::Confirmed)
        },
        ShieldedActivityEntry {
            counterparty: Some(platform.to_bytes()),
            ..entry(Kind::Unshield, 30, Status::Failed)
        },
        ShieldedActivityEntry {
            direction: Direction::In,
            ..entry(
                Kind::ShieldFromIdentity {
                    identity_id: [1; 32],
                },
                40,
                Status::Confirmed,
            )
        },
        entry(Kind::ShieldFromAssetLock, 50, Status::Unknown),
    ];
    let lib = Arc::new(FakeShielded {
        activity: rows,
        ..FakeShielded::default()
    });
    let shown = run(&fx, &lib, |_, lib| async move {
        pool::history(&*lib, 0, 500).await.unwrap()
    });
    let kinds: Vec<_> = shown.iter().map(|a| (a.kind, a.status)).collect();
    assert_eq!(
        kinds,
        [
            (ShieldedActivityKind::Sent, ShieldedActivityStatus::Pending),
            (
                ShieldedActivityKind::Withdrawal,
                ShieldedActivityStatus::Confirmed
            ),
            (
                ShieldedActivityKind::Unshield,
                ShieldedActivityStatus::Failed
            ),
            (
                ShieldedActivityKind::Shield,
                ShieldedActivityStatus::Confirmed
            ),
            (
                ShieldedActivityKind::ShieldFromCore,
                ShieldedActivityStatus::Unknown
            ),
        ]
    );
    let sent = &shown[0];
    assert_eq!(sent.id, hex::encode([100u8; 32]));
    assert_eq!(sent.counterparty.as_deref(), Some(payee_text().as_str()));
    assert_eq!(sent.memo.as_deref(), Some("lunch"));
    assert_eq!(
        (sent.fee_credits, sent.created_at),
        (Some(20), Some(1_700_000_000))
    );
    assert_eq!(sent.direction, ShieldedDirection::Out);
    assert_eq!(shown[1].counterparty, Some(core_address(5).to_string()));
    assert_eq!(shown[1].block_height, Some(77));
    assert_eq!(
        shown[2].counterparty,
        Some(platform.to_bech32m_string(Network::Regtest))
    );
    assert_eq!(shown[3].direction, ShieldedDirection::In);
    assert!(shown[4].counterparty.is_none() && shown[4].created_at.is_none());

    let paged = run(&fx, &lib, |_, lib| async move {
        let page = pool::history(&*lib, 1, 2).await.unwrap();
        (page.len(), code(pool::history(&*lib, 0, 501).await))
    });
    assert_eq!(paged, (2, "invalid_argument"));
}

#[test]
fn a_shield_quote_refuses_while_a_lock_pends_or_core_falls_short() {
    let fx = Fixture::new(0);
    let quote = |lib: FakeShielded, duffs: u64| {
        run(&fx, &Arc::new(lib), move |s, lib| async move {
            pool::shield_quote(&s, &*lib, duffs).await
        })
    };
    let pool_fee = pool::shield_pool_fee(version(LATEST)).unwrap();
    let min = pool_fee / 1_000 + 1;
    let q = quote(FakeShielded::default(), 1_000_000).unwrap();
    assert_eq!((q.fee_duffs, q.total_duffs), (1_000, 1_001_000));
    assert_eq!(q.credits, 1_000_000_000 - pool_fee);
    assert_eq!((q.grant.max_duffs, q.grant.max_credits), (1_001_000, 0));

    assert_eq!(
        quote(FakeShielded::default(), min - 1).unwrap_err(),
        ShieldedError::BelowMinimum { min }
    );
    let pending = FakeShielded {
        locks: vec![
            lock(2, LockStatus::Consumed),
            lock(1, LockStatus::Broadcast),
        ],
        ..FakeShielded::default()
    };
    assert_eq!(code(quote(pending, 1_000_000)), "shielded.shield_pending");
    let short = FakeShielded {
        spendable: Spendable {
            duffs: 1_000_500,
            inputs: 1,
            protected: 0,
        },
        ..FakeShielded::default()
    };
    assert_eq!(
        quote(short, 1_000_000).unwrap_err(),
        ShieldedError::FundingInsufficient {
            needed: 1_001_000,
            available: 1_000_500,
        }
    );
    let locked = FakeShielded {
        spendable: Spendable {
            protected: 1,
            ..FakeShielded::default().spendable
        },
        ..FakeShielded::default()
    };
    assert_eq!(code(quote(locked, 1_000_000)), "shielded.coins_locked");
}

#[test]
fn resume_refuses_an_unknown_lock_and_waits_for_a_proof() {
    let fx = Fixture::new(0);
    let wallet = fx.wallet;
    let grant = fx.grant(0);
    let lib = Arc::new(FakeShielded {
        locks: vec![lock(1, LockStatus::Broadcast)],
        ..FakeShielded::default()
    });
    let txid = lock(1, LockStatus::Broadcast).outpoint.txid.to_string();
    let g = grant.clone();
    let (unknown, waiting) = run(&fx, &lib, move |s, lib| async move {
        let unknown = code(pool::resume(&s, wallet, &*lib, &"ab".repeat(32), &g).await);
        let waiting = pool::resume(&s, wallet, &*lib, &txid, &g).await.unwrap();
        (unknown, waiting)
    });
    assert_eq!(unknown, "invalid_argument");
    assert_eq!(waiting.state, super::shielded::ShieldState::ProofWaiting);
    assert!(fx.unused(&grant));
}

/// E0-04 Q15, ruling (c): a spend whose broadcast the library could not
/// confirm is `pending`, never an error, with the transition the library
/// keeps to re-send (its hash) in the outcome and the status. A pass that
/// sees it executed resolves it to done (the row confirms), one where the
/// prune backstop released the notes to not executed (the row fails), and
/// no second transition is built either way: the facade has no resend.
#[test]
fn an_unconfirmed_spend_stays_pending_until_a_scan_resolves_it() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let wallet = fx.wallet;
    let credits = Rules::at(version(LATEST)).unwrap().min.max(NOTE / 2);
    let unconfirmed = || PlatformWalletError::ShieldedSpendUnconfirmed {
        operation: "transfer",
        reason: "proof fetch failed".into(),
    };
    let row = |n: u8| {
        let id = hex::encode([n; 32]);
        let lib = Arc::clone(&lib);
        let history = run(&fx, &lib, |_, lib| async move {
            pool::history(&*lib, 0, 10).await.unwrap()
        });
        (history.into_iter().find(|r| r.id == id)).unwrap().status
    };
    let status = || {
        run(&fx, &lib, |_, lib| async move {
            pool::status(&*lib).await.unwrap()
        })
    };

    // No pass can be admitted while a spend's library call runs (one could
    // re-send what the library stores on an unknown result unchecked).
    let s = Arc::clone(&fx.s);
    *lib.probe.lock().unwrap() = Some(Box::new(move || {
        s.platform.shielded_passes.try_read().is_err()
    }));

    for (n, unshield, found, want) in [
        (
            0u8,
            false,
            Status::Confirmed,
            ShieldedActivityStatus::Confirmed,
        ),
        (1, true, Status::Failed, ShieldedActivityStatus::Failed),
    ] {
        lib.fail_with(unconfirmed());
        let lease = fx.flow(FlowKind::ShieldedSpend, vec![fx.grant(10 * NOTE)]);
        let l = lease.clone();
        let syncs = lib.syncs.load(Ordering::SeqCst);
        let out = run(&fx, &lib, move |s, lib| async move {
            if unshield {
                pool::unshield(&s, wallet, &*lib, credits, None, &l).await
            } else {
                pool::send(&s, wallet, &*lib, (&payee_text(), credits), None, &l).await
            }
        })
        .unwrap();
        assert_eq!(out.state, ShieldedSpendState::Pending);
        assert_eq!(out.unresolved, Some(unresolved(n)));
        assert_eq!(
            lib.probed.lock().unwrap().last(),
            Some(&true),
            "passes held"
        );
        assert!(
            fx.s.platform.shielded_passes.try_read().is_ok(),
            "and let go"
        );
        // On record in the journal against its activity.
        assert_eq!(lib.recorded.lock().unwrap().last(), Some(&unresolved(n)));
        // The quote stays charged; no pass ran for it.
        let kind = if unshield {
            ShieldedFeeKind::Withdrawal
        } else {
            ShieldedFeeKind::Base
        };
        assert_eq!(fx.credits_spent(&lease), credits + fee_of(credits, kind));
        assert_eq!(lib.syncs.load(Ordering::SeqCst), syncs);
        assert_eq!(status().unresolved_spends, [unresolved(n)]);
        assert_eq!(row(n), ShieldedActivityStatus::Pending);

        // The session's next pass resolves it.
        *lib.scan_finds.lock().unwrap() = Some(found);
        run(
            &fx,
            &lib,
            |_, lib| async move { assert!(lib.sync_now().await) },
        );
        assert_eq!(row(n), want);
        assert!(status().unresolved_spends.is_empty());
        assert_eq!(
            lib.spent().len(),
            usize::from(n) + 1,
            "no second transition"
        );
    }
}

/// Q15's lease confinement for the facade's own passes: a shielded call
/// runs in `on_pool`'s task under the session's operation guard, so the
/// close waits for it and never drops it partway (a shield between its
/// lock and the apply, a spend after its keys are out).
#[test]
fn a_close_waits_for_a_shielded_call_and_never_drops_it() {
    use std::sync::atomic::AtomicBool;
    let fx = Fixture::new(0);
    let (wallet, session) = (fx.wallet, Arc::clone(&fx.s));
    let (entered_tx, entered) = std::sync::mpsc::channel();
    let (release, released) = tokio::sync::oneshot::channel::<()>();
    let finished = Arc::new(AtomicBool::new(false));
    let f = Arc::clone(&finished);
    std::thread::scope(|scope| {
        let call = scope.spawn(|| {
            fx.engine
                .block_on(on_pool(&session, wallet, move |_, _| async move {
                    entered_tx.send(()).unwrap();
                    released.await.ok();
                    f.store(true, Ordering::SeqCst);
                    Ok(())
                }))
        });
        entered.recv_timeout(Duration::from_secs(30)).unwrap();
        let network = fx.s.network.clone();
        let closing = scope.spawn(|| fx.engine.block_on(fx.engine.close_network(network)));
        std::thread::sleep(Duration::from_millis(300));
        assert!(!closing.is_finished(), "the close waits for the call");
        release.send(()).unwrap();
        assert!(closing.join().unwrap().unwrap());
        assert!(finished.load(Ordering::SeqCst), "the call ran to its end");
        call.join().unwrap().unwrap();
    });
}

/// A lock Platform calls consumed without proof pends, as does one of ours
/// a restore read back from the chain, and blocks a new shield until the
/// user's word settles it; a recovered lock no shield step signed is
/// history. Nothing is told apart before the journal is read.
#[test]
fn a_shield_lock_of_unknown_consumption_pends_until_the_user_resolves_it() {
    let fx = Fixture::new(0);
    let wallet = fx.wallet;
    let txid = |n: u8| lock(n, LockStatus::Broadcast).outpoint.txid;
    let grant = || {
        let purpose = dw_vault::GrantPurpose::PlatformOp {
            max_duffs: 1,
            max_credits: 0,
        };
        let granted =
            fx.s.vault
                .authorize(purpose, Some(&wallet.0), dw_vault::Credential::None);
        granted.unwrap().id
    };

    // Platform answers "already consumed" to the apply.
    let lib = Arc::new(FakeShielded {
        locks: vec![lock(1, LockStatus::InstantSendLocked)],
        own: Some(HashSet::from([txid(1)])),
        applies: Mutex::new(Some(Ok(Applied::Taken))),
        ..FakeShielded::default()
    });
    let g = grant();
    let resumed = run(&fx, &lib, move |s, lib| async move {
        pool::resume(&s, wallet, &*lib, &txid(1).to_string(), &g).await
    })
    .unwrap();
    assert_eq!(resumed.state, ShieldState::ConsumptionUnknown);
    assert!(lib.settles.lock().unwrap().is_empty(), "nothing settled");
    // It pends as such, though the library's status of it is unchanged
    // (no ChainLock proof promoted it), and the user may resolve it.
    let pending = run(&fx, &lib, |_, lib| async move {
        pool::status(&*lib).await.unwrap().pending_shields
    });
    assert!(pending[0].consumption_unknown, "{pending:?}");
    run(&fx, &lib, move |_, lib| async move {
        pool::resolve(wallet, &*lib, &txid(1).to_string()).await
    })
    .unwrap();
    assert_eq!(
        *lib.settles.lock().unwrap(),
        [(txid(1), TopUpSettled::Resolved)]
    );

    // The library then holds it `RecoveredFromChain`, as a restore reads
    // ours back; lock 6 is another install's.
    let lib = Arc::new(FakeShielded {
        locks: vec![
            lock(1, LockStatus::RecoveredFromChain),
            lock(6, LockStatus::RecoveredFromChain),
        ],
        own: Some(HashSet::from([txid(1)])),
        ..FakeShielded::default()
    });
    let pending = run(&fx, &lib, |_, lib| async move {
        pool::status(&*lib).await.unwrap().pending_shields
    });
    let pending: Vec<_> = (pending.iter())
        .map(|p| (p.txid.clone(), p.proved, p.consumption_unknown))
        .collect();
    assert_eq!(pending, [(txid(1).to_string(), true, true)]);
    let quote = |lib: &Arc<FakeShielded>| {
        run(&fx, lib, |s, lib| async move {
            pool::shield_quote(&s, &*lib, 1_000_000).await.map(drop)
        })
    };
    assert_eq!(code(quote(&lib)), "shielded.shield_pending");
    let resolved = run(&fx, &lib, move |_, lib| async move {
        let history = code(pool::resolve(wallet, &*lib, &txid(6).to_string()).await);
        (
            history,
            pool::resolve(wallet, &*lib, &txid(1).to_string()).await,
        )
    });
    assert_eq!(resolved.0, "invalid_argument");
    resolved.1.unwrap();
    assert_eq!(
        *lib.settles.lock().unwrap(),
        [(txid(1), TopUpSettled::Resolved)]
    );
    quote(&lib).unwrap();

    // Before the journal is read: no pending list, the shield calls wait.
    let unread = Arc::new(FakeShielded {
        locks: vec![lock(1, LockStatus::Broadcast)],
        own: None,
        ..FakeShielded::default()
    });
    let status = run(&fx, &unread, |_, lib| async move {
        pool::status(&*lib).await.unwrap()
    });
    assert!(status.pending_shields.is_empty());
    assert_eq!(code(quote(&unread)), "platform.unavailable");
}

/// A shield's lock spends at most 50 coins (its InstantSend proof stops
/// fitting a transition from about 65 inputs): the quote refuses when the
/// fewest coins that cover it, largest first, are more.
#[test]
fn a_shield_quote_refuses_a_lock_of_more_than_50_coins() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded {
        // 100 coins of 30,000 duffs; the fee bound is 1,000.
        coins: vec![30_000; 100],
        spendable: Spendable {
            duffs: 3_000_000,
            inputs: 100,
            protected: 0,
        },
        ..FakeShielded::default()
    });
    let quote = |duffs: u64| {
        run(&fx, &lib, move |s, lib| async move {
            pool::shield_quote(&s, &*lib, duffs).await
        })
    };
    // 50 coins cover 1,499,000 + 1,000 exactly; one duff more takes 51.
    assert_eq!(MAX_LOCK_INPUTS, 50);
    quote(1_499_000).unwrap();
    assert_eq!(
        quote(1_499_001).unwrap_err(),
        ShieldedError::TooManyInputs {
            inputs: 51,
            max: 50
        }
    );
    // Larger coins first: one big coin covers it.
    let lib = Arc::new(FakeShielded {
        coins: [vec![30_000; 99], vec![2_000_000]].concat(),
        ..FakeShielded::default()
    });
    run(&fx, &lib, |s, lib| async move {
        pool::shield_quote(&s, &*lib, 1_499_001).await
    })
    .unwrap();
}

// MARK: the real library

/// The account's address through the real library: bound or not, and the
/// address once bound.
fn bound_address(
    engine: &crate::Engine,
    session: &Arc<NetworkSession>,
    wallet: crate::WalletId,
) -> (bool, Option<String>) {
    engine
        .block_on(on_pool(session, wallet, |_, lib| async move {
            let bound = lib.bind().await?;
            let address = (lib.address().await)
                .map(|raw| OrchardAddress::from_raw_bytes(&raw).unwrap())
                .map(|a| a.to_bech32m_string(Network::Regtest));
            Ok((bound, address))
        }))
        .unwrap()
}

/// The viewing key binds from the vault while it needs no prompt, and is
/// persisted, so after a restart the account binds with the vault locked;
/// a wallet never bound stays unbound while the vault is locked.
#[test]
fn the_viewing_key_binds_once_and_from_storage_after_a_restart() {
    use super::topup_tests::{fixture, options};
    let mut f = fixture(true);
    let fvk = f.session.vault.shielded_viewing_key(&f.wallet.0).unwrap();
    let views =
        platform_wallet::wallet::shielded::AccountViewingKeys::from_fvk_bytes(&fvk).unwrap();
    let want = OrchardAddress::from_raw_bytes(&views.default_address.to_raw_address_bytes())
        .unwrap()
        .to_bech32m_string(Network::Regtest);
    assert_eq!(
        bound_address(&f.engine, &f.session, f.wallet),
        (true, Some(want.clone()))
    );

    f.engine
        .block_on(f.engine.close_network(DashNetwork::Regtest))
        .unwrap();
    f.session = (f.engine)
        .block_on(f.engine.open_network(DashNetwork::Regtest, options()))
        .unwrap();
    f.session.vault.lock();
    assert_eq!(
        f.session.vault.shielded_viewing_key(&f.wallet.0),
        Err(VaultError::Locked)
    );
    assert_eq!(
        bound_address(&f.engine, &f.session, f.wallet),
        (true, Some(want))
    );
    assert!(f.session.data_dir().join(pool::SHIELDED_DB_FILE).exists());

    let fresh = fixture(true);
    fresh.session.vault.lock();
    assert_eq!(
        bound_address(&fresh.engine, &fresh.session, fresh.wallet),
        (false, None)
    );
}

/// DEC-1032 on the real library: before any pass, each spend the library
/// stored to re-send is on record in the dispatch journal against its
/// activity, with Platform's hash of the stored bytes; while its wallet has
/// no background lease (the vault locked) no pass is admitted, and with no
/// stored spend a pass is.
#[test]
fn a_pass_records_each_stored_spend_and_waits_for_the_wallets_lease() {
    use platform_wallet::wallet::shielded::store::PendingRedrive;
    use platform_wallet::wallet::shielded::{ShieldedStore, SubwalletId};
    let f = super::topup_tests::fixture(true);
    let (engine, session, wallet) = (&f.engine, &f.session, f.wallet);
    assert!(bound_address(engine, session, wallet).0);
    let manager = session.manager().unwrap();
    let st_bytes = b"a signed transition".to_vec();
    let s = Arc::clone(session);
    let m = Arc::clone(&manager);
    engine.block_on(async move {
        assert!(pool::admit_pass(&s, &m).await.is_ok(), "nothing stored");
        let coordinator = pool::coordinator(&s, &m).await.unwrap();
        let redrive = PendingRedrive {
            activity_id: [5; 32],
            anchor: [0; 32],
            nullifiers: vec![[6; 32]],
            st_bytes,
            attempts: 0,
            identity_nonce_finalized: false,
            identity_user_abandoned: false,
        };
        let subwallet = SubwalletId::new(wallet.0, dw_vault::SHIELDED_ACCOUNT);
        (coordinator.store().write().await)
            .arm_redrive(subwallet, redrive)
            .unwrap();
        s.ensure_background().await;
        assert!(s.leases.background(&wallet).is_some());
        assert!(pool::admit_pass(&s, &m).await.is_ok());
    });
    let recorded = engine
        .block_on(session.leases.shielded_redrives(wallet))
        .unwrap();
    let hash: [u8; 32] = Sha256::digest(b"a signed transition").into();
    assert_eq!(recorded, [([5; 32], hash)]);
    // The facade names the same hash.
    let s = Arc::clone(session);
    let unresolved = engine
        .block_on(on_pool(&s, wallet, |_, lib| async move {
            Ok(lib.unresolved().await?)
        }))
        .unwrap();
    assert_eq!(unresolved[0].transition, hex::encode(hash));

    engine.block_on(session.lock_vault()).unwrap();
    assert!(session.leases.background(&wallet).is_none());
    let s = Arc::clone(session);
    let held = engine.block_on(async move { pool::admit_pass(&s, &manager).await.err() });
    assert_eq!(held, Some(pool::PassHeld::NoLease(wallet)));
}

/// Lane V's reference through the facade: a mainnet session of the
/// "abandon … about" wallet shows the mobile wallets' shielded address.
#[test]
fn the_facade_shows_the_mobile_wallets_address_for_the_abandon_wallet() {
    use super::journal_tests::{ABANDON_12, Recorder, engine};
    use super::topup_tests::options;
    let dir = dw_testutil::private_tempdir();
    let engine = engine(dir.path(), Arc::new(Recorder::default()));
    let s = engine
        .block_on(engine.open_network(DashNetwork::Mainnet, options()))
        .unwrap();
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let wallet = engine
        .block_on(s.import_wallet(
            Zeroizing::new(ABANDON_12.to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions::default(),
        ))
        .unwrap();
    let address = engine
        .block_on(s.dashpay(wallet).shielded_receive_address())
        .unwrap();
    assert_eq!(address.address, ABANDON_MAINNET);
    assert_eq!(address.account, 0);
}
