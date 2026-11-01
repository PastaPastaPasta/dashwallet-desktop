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
use dw_vault::{VaultError, VaultSigner};
use platform_wallet::PlatformWalletError;
use platform_wallet::wallet::shielded::note_selection::{ShieldedFeeKind, select_notes_with_fee};
use platform_wallet::wallet::shielded::{
    OrchardKeySet, ShieldedActivityEntry, ShieldedActivityKind as Kind,
    ShieldedActivityStatus as Status, ShieldedDirection as Direction, ShieldedNote,
};
use zeroize::Zeroizing;

use super::errors::{PlatformError, ShieldedError};
use super::flows::FlowKind;
use super::lease::{CallPermit, FundingKey, Lease};
use super::registration::RegistrationWait;
use super::registration_flow::{FundError, LockInfo, LockStatus, Spendable};
use super::requests::tests::Fixture;
use super::shielded::{
    SHIELDED_ACTIVATION_VERSION, ShieldedActivityKind, ShieldedActivityStatus, ShieldedDirection,
};
use super::shielded_pool::{self as pool, Ledger, ShieldedLib, SpendError, SyncState, on_pool};
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
    /// The locks the journal records settled.
    settled: Mutex<HashSet<Txid>>,
    syncs: AtomicU32,
    spendable: Spendable,
    receive: String,
    /// The next spend's failure, after the keys are out.
    fails: Mutex<Option<PlatformWalletError>>,
    spent: Mutex<Vec<Spent>>,
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
            settled: Mutex::default(),
            syncs: AtomicU32::new(0),
            spendable: Spendable {
                duffs: 10_000_000_000,
                inputs: 1,
                protected: 0,
            },
            receive: core_address(9).to_string(),
            fails: Mutex::new(None),
            spent: Mutex::new(Vec::new()),
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
        self.spent.lock().unwrap().push(Spent {
            operation,
            to,
            credits,
            memo: memo.to_vec(),
            fvk: keys.viewing_keys().to_fvk_bytes(),
        });
        match self.fails.lock().unwrap().take() {
            Some(e) => Err(SpendError::Library(Box::new(e))),
            None => Ok(()),
        }
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
        Ok((self.activity.iter())
            .skip(offset as usize)
            .take(limit as usize)
            .cloned()
            .collect())
    }

    async fn sync_now(&self) {
        self.syncs.fetch_add(1, Ordering::SeqCst);
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

    async fn settled_locks(&self) -> HashSet<Txid> {
        self.settled.lock().unwrap().clone()
    }

    async fn settle(&self, txid: Txid) {
        self.settled.lock().unwrap().insert(txid);
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

    async fn apply(&self, _: OutPoint, _: &VaultSigner) -> Result<(), PlatformError> {
        unreachable!("no test applies a shield")
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
    let cases: [(PlatformWalletError, bool, ShieldedError); 4] = [
        (
            PlatformWalletError::ShieldedBroadcastFailed(pool_text.into()),
            true,
            ShieldedError::PoolTooSmall {
                notes: 3,
                required: 250,
            },
        ),
        (
            PlatformWalletError::ShieldedSpendUnconfirmed {
                operation: "transfer",
                reason: "proof fetch failed".into(),
            },
            false,
            PlatformError::BroadcastUnknown {
                artifact: "shielded/transfer".into(),
            }
            .into(),
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
        pool::spend_error(
            PlatformWalletError::ShieldedBroadcastFailed("rejected".into()),
            "withdraw"
        )
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
