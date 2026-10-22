//! DP5-02 §3–§5: an invitation's claim as DP1-02's entry state, against a
//! real session, vault (the link stash) and app.sqlite, with the library
//! scripted ([`Fake`]): every persisted state, kills, the request to the
//! inviter, the link's wipe, and no duff of the wallet's, ever.

use dashcore::hashes::Hash;
use dashcore::{OutPoint, Txid};
use dpp::identity::identity_public_key::v0::IdentityPublicKeyV0;
use dpp::identity::v0::IdentityV0;
use dpp::identity::{Identity, IdentityPublicKey, KeyID, KeyType, Purpose, SecurityLevel};

use super::*;
use crate::platform::BearerSecret;
use crate::platform::contacts::Eligibility;
use crate::platform::errors::ContactError;
use crate::platform::flows::CONTACT_REQUEST_CREDITS;
use crate::platform::invitation_check::Checked;
use crate::platform::invitations::{InvitationInvalidReason, InvitationStatus};
use crate::platform::payments::Counterparty;
use crate::platform::registration_rows::Decoded;

/// The stashed link; the fake reads no more of it than its text.
pub(super) const LINK: &str =
    "dashpay://invite?du=bob&assetlocktx=abab&pk=cVoucherKeyForTests&islock=null";
const INVITER: &str = "Bob1nviterIdentity";
/// 0.05 DASH: covers an uncontested claim, not a contested one.
const FUNDING: u64 = 5_000_000;

/// How a scripted claim ends.
#[derive(Debug, Clone, Copy)]
pub(super) enum ClaimScript {
    /// Spent, and the identity at its id is not ours (or there is none).
    Claimed,
    Invalid,
    NotChainLocked,
    Unavailable,
    /// Platform created the identity; the answer was lost.
    LandsThenUnavailable,
    /// Refused as consumed: our identity is on Platform, the library did
    /// not adopt it.
    ConsumedOurs,
    /// The library answers with an identity that is not the claim's.
    OtherId,
    /// Another device's identity at the index appears; the claim fails.
    OtherAppearsThenUnavailable,
    /// Signed, then the network failed: the claim may have landed.
    ReleasedThenUnavailable,
    /// Platform refused the claim's keys as another identity's (F11).
    DuplicateKeys,
}

/// Another identity of the same seed (another device's).
pub(super) const OTHER: &str = "Other1dentityAtTheIndex";

/// The voucher's outpoint, the claim's identity derives from.
pub(super) fn outpoint() -> OutPoint {
    OutPoint::new(Txid::from_byte_array([0xab; 32]), 1)
}

fn claimed_id() -> String {
    flow::b58(&flow::identity_for(outpoint()))
}

fn key(id: u32, byte: u8) -> IdentityPublicKey {
    IdentityPublicKey::V0(IdentityPublicKeyV0 {
        id,
        purpose: Purpose::AUTHENTICATION,
        security_level: SecurityLevel::MASTER,
        contract_bounds: None,
        key_type: KeyType::ECDSA_SECP256K1,
        read_only: false,
        data: vec![byte; 33].into(),
        disabled_at: None,
    })
}

/// The keys a scripted claim submits.
pub(super) fn our_keys() -> BTreeMap<KeyID, IdentityPublicKey> {
    BTreeMap::from([(0, key(0, 2)), (1, key(1, 3))])
}

/// The identity at the voucher's id on Platform, with `keys`.
pub(super) fn landed(keys: &BTreeMap<KeyID, IdentityPublicKey>) -> Option<Identity> {
    Some(Identity::V0(IdentityV0 {
        id: flow::identity_for(outpoint()),
        public_keys: keys.clone(),
        balance: 0,
        revision: 0,
    }))
}

/// The fake's default: valid, [`FUNDING`], from [`INVITER`].
pub(super) fn valid() -> InvitationStatus {
    worth(FUNDING)
}

fn worth(funding_duffs: u64) -> InvitationStatus {
    InvitationStatus::Valid {
        inviter: Some(Counterparty {
            identity: INVITER.into(),
            username: Some("bob".into()),
            display_name: None,
            avatar_url: None,
        }),
        funding_duffs,
        contested_allowed: Checked::contested_allowed(funding_duffs),
        expires_at: Some(1),
    }
}

fn stash(f: &Fixture) -> String {
    f.engine
        .block_on(f.session.stash_invitation(BearerSecret::new(LINK.into())))
        .unwrap()
}

fn stashed(f: &Fixture, link_id: &str) -> bool {
    f.engine
        .block_on(f.session.read_invitation(link_id.into()))
        .unwrap()
        .is_some()
}

fn invitation_row(link_id: &str, inviter: Option<&str>) -> NewRegistration {
    NewRegistration {
        funding: Funding::Invitation {
            link_id: link_id.into(),
            inviter: inviter.map(Into::into),
            identity: claimed_id(),
            request_error: None,
        }
        .encode(),
        ..core_row(PLAIN)
    }
}

fn invite(link_id: &str) -> RegistrationRequest {
    RegistrationRequest {
        funding: RegistrationFunding::Invitation {
            link_id: link_id.into(),
            request_inviter: true,
        },
        ..request(PLAIN)
    }
}

/// A registration lease of the claim's grant: credits, no duffs (D8).
fn claim_lease(f: &Fixture) -> Lease {
    let g = f.grant(0, 10 * COIN * 1000, Credential::None);
    f.engine
        .block_on(
            f.session
                .begin_lease(f.wallet, FlowKind::Registration, &[g]),
        )
        .unwrap()
}

fn done(f: &Fixture, draft: i64) {
    let row = f.row(draft);
    assert_eq!((phase(&row), row.error.as_deref()), (Phase::Done, None));
    assert_eq!(row.identity, Some(claimed_id()));
}

/// The acceptance on the fake: an identity, a name and a profile paid by
/// the invitation, the request to the inviter, the link wiped, and nothing
/// from the wallet's coins (a funded wallet's, too).
#[test]
fn a_claim_registers_publishes_and_asks_the_inviter_with_no_duffs() {
    let f = fixture(false);
    let lib = f.fake();
    let link_id = stash(&f);
    let mut req = invite(&link_id);
    req.initial_profile = Some(InitialProfile {
        display_name: Some("Carol".into()),
        public_message: None,
        avatar_candidate: None,
    });
    let quote = f.engine.block_on(registration::quote(&*lib, &req)).unwrap();
    let credits = flow::IDENTITY_CREATE_CREDITS
        + NAME_CREDITS
        + flow::PROFILE_CREDITS
        + CONTACT_REQUEST_CREDITS;
    assert_eq!(
        (
            quote.lock_duffs,
            quote.fee_duffs,
            quote.total_duffs,
            quote.available_duffs
        ),
        (FUNDING, 0, 0, 0)
    );
    assert_eq!(
        (quote.grant.max_duffs, quote.grant.max_credits),
        (0, credits)
    );
    assert_eq!(
        quote.remaining_credits,
        FUNDING * flow::CREDITS_PER_DUFF - credits
    );
    let grant = f.grant(0, quote.grant.max_credits, Credential::None);
    let draft: i64 = f
        .engine
        .block_on(registration::start(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            req,
            grant,
        ))
        .unwrap()
        .parse()
        .unwrap();
    f.settle(draft);
    done(&f, draft);
    let row = f.row(draft);
    assert_eq!(
        Funding::decode(&row.funding),
        Decoded::Known(Funding::Invitation {
            link_id: link_id.clone(),
            inviter: Some(INVITER.into()),
            identity: claimed_id(),
            request_error: None,
        })
    );
    let id = claimed_id();
    let w = lib.w();
    assert_eq!(w.identities, [(0, id.clone())]);
    assert_eq!(w.names, [(id.clone(), PLAIN.to_owned())]);
    assert!(w.profiles.contains_key(&id));
    assert_eq!(w.requests, [(id, INVITER.to_owned())]);
    // §3: no funding call, no lock, and the lease had no funding signer.
    assert_eq!((w.funds, w.claims), (0, 1));
    assert!(w.locks.is_empty());
    assert_eq!(w.claim_funders, [false]);
    drop(w);
    assert!(
        !stashed(&f, &link_id),
        "D13: wiped once the claim is durable"
    );
    assert_eq!(
        f.engine
            .block_on(f.session.invitation_status(link_id))
            .unwrap(),
        InvitationStatus::Expired
    );
}

/// No coins, SPV not caught up: a claim needs neither (D9); a Core-funded
/// registration in the same state waits for the sync.
#[test]
fn a_wallet_without_coins_or_sync_claims() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().spendable = Spendable {
        duffs: 0,
        inputs: 0,
        protected: 0,
    };
    lib.ready.store(false, Ordering::SeqCst);
    let draft = f.insert(invitation_row(&stash(&f), Some(INVITER)));
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    done(&f, draft);
    assert_eq!(lib.w().funds, 0);
    // The claim hold itself still holds.
    let f = fixture(false);
    let lib = f.fake();
    lib.w().claim_wait = Some(RegistrationWait::Sync);
    let draft = f.insert(invitation_row(&stash(&f), Some(INVITER)));
    let lease = claim_lease(&f);
    f.drive(&lib, draft, Some(lease.clone()), false);
    assert_eq!(phase(&f.row(draft)), Phase::Draft);
    assert_eq!(f.status(&lib, draft).waiting, Some(RegistrationWait::Sync));
}

/// §3 start: what cannot be claimed is refused before a row exists.
#[test]
fn the_start_refuses_what_cannot_be_claimed() {
    let f = fixture(false);
    let lib = f.fake();
    let link_id = stash(&f);
    let refused = |req: &RegistrationRequest| {
        let grant = f.grant(0, 10 * COIN * 1000, Credential::None);
        let started = f.engine.block_on(registration::start(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            req.clone(),
            grant,
        ));
        let quoted = f.engine.block_on(registration::quote(&*lib, req));
        assert_eq!(
            started.as_ref().map(drop).map_err(|e| e.code()),
            quoted.as_ref().map(drop).map_err(|e| e.code())
        );
        started.map(drop).map_err(|e| e.code())
    };
    for (status, code) in [
        (InvitationStatus::Claimed, "invitation.claimed"),
        (InvitationStatus::Expired, "invitation.expired"),
        (
            InvitationStatus::Invalid {
                reason: InvitationInvalidReason::WrongNetwork,
            },
            "invitation.invalid",
        ),
    ] {
        lib.w().invitation = Some(status);
        assert_eq!(refused(&invite(&link_id)), Err(code));
    }
    // A contested name needs a contested lock's worth.
    let contested = RegistrationRequest {
        label: CONTESTED.into(),
        temporary_label: Some("alice9".into()),
        ..invite(&link_id)
    };
    lib.w().invitation = Some(worth(FUNDING));
    assert_eq!(refused(&contested), Err("name.unavailable_for_invite"));
    lib.w().invitation = Some(worth(flow::CONTESTED_LOCK_DUFFS));
    let quote = f.engine.block_on(registration::quote(&*lib, &contested));
    assert!(quote.is_ok(), "{quote:?}");
    // D8: a grant with duffs.
    let grant = f.grant(1, 10 * COIN * 1000, Credential::None);
    let started = f.engine.block_on(registration::start(
        &f.session,
        f.wallet,
        Arc::clone(&lib),
        invite(&link_id),
        grant,
    ));
    assert_eq!(
        started.map(drop).map_err(|e| e.code()),
        Err("platform.grant_invalid")
    );
    // ...and a lease issued with a funding signer.
    let funded = f.grant(1, 10 * COIN * 1000, Credential::None);
    let lease = f
        .engine
        .block_on(
            f.session
                .begin_lease(f.wallet, FlowKind::Registration, &[funded]),
        )
        .unwrap();
    let started = f.engine.block_on(registration::start(
        &f.session,
        f.wallet,
        Arc::clone(&lib),
        invite(&link_id),
        crate::platform::lease::lease_string(&lease.id()),
    ));
    assert_eq!(
        started.map(drop).map_err(|e| e.code()),
        Err("platform.grant_invalid")
    );
    // Too little for what the plan writes.
    lib.w().invitation = Some(worth(300_000));
    let err = f
        .engine
        .block_on(registration::quote(&*lib, &invite(&link_id)))
        .unwrap_err();
    let needed = (flow::IDENTITY_CREATE_CREDITS + NAME_CREDITS + CONTACT_REQUEST_CREDITS)
        .div_ceil(flow::CREDITS_PER_DUFF);
    assert_eq!(
        err,
        RegistrationError::FundingInsufficient {
            needed,
            available: 300_000
        }
    );
    // D7: a wallet with an identity.
    lib.w().invitation = None;
    lib.w().identities.push((0, "Other1dentity".into()));
    assert_eq!(
        refused(&invite(&link_id)),
        Err("invitation.already_has_identity")
    );
    assert!(f.rows().is_empty(), "no row for a refused start");
    assert!(stashed(&f, &link_id));
}

/// §3: another device claimed it, or the lock was used another way.
#[test]
fn a_claim_spent_elsewhere_fails_for_good_and_spends_nothing() {
    let f = fixture(false);
    let lib = f.fake();
    let link_id = stash(&f);
    lib.w()
        .claim_script
        .push_back(invitation::ClaimScript::Claimed);
    let draft = f.insert(invitation_row(&link_id, Some(INVITER)));
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    let row = f.row(draft);
    assert_eq!(phase(&row), Phase::KeysPrepared);
    assert_eq!(
        (row.error.as_deref(), row.retryable),
        (Some("invitation.claimed"), false)
    );
    let w = lib.w();
    assert_eq!((w.funds, w.identity_creates), (0, 0));
    assert!(w.requests.is_empty());
    drop(w);
    assert!(stashed(&f, &link_id), "the user dismisses it");
    // Consumed by another claim: proved not ours, so discarded (D11).
    assert_eq!(discard_code(&f, &lib, draft), Ok(()));
    // A link this build cannot claim fails the same way.
    let lib = f.fake();
    lib.w()
        .claim_script
        .push_back(invitation::ClaimScript::Invalid);
    let draft = f.insert(invitation_row(&stash(&f), Some(INVITER)));
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    assert_eq!(f.row(draft).error.as_deref(), Some("invitation.invalid"));
}

/// A stale islock before the funding block is chain-locked: the claim
/// waits, and goes through once it is.
#[test]
fn a_stale_islock_waits_for_the_chainlock() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w()
        .claim_script
        .push_back(invitation::ClaimScript::NotChainLocked);
    let draft = f.insert(invitation_row(&stash(&f), Some(INVITER)));
    // CHAINLOCK_POLL (120 s) keeps the live run in its wait.
    f.spawn(&lib, draft, Some(claim_lease(&f)));
    eventually("the ChainLock wait", || {
        f.status(&lib, draft).waiting == Some(RegistrationWait::ChainLock)
    });
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.error.as_deref()),
        (Phase::KeysPrepared, None)
    );
    f.kill(draft);
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    done(&f, draft);
    assert_eq!(lib.w().claims, 2);
}

/// Every step killed before and after the library acted: one identity,
/// one name, one profile, one request; the link wiped.
#[test]
fn a_kill_at_any_claim_step_resumes_to_one_claim() {
    let points = [
        (Call::Claim, When::Before),
        (Call::Claim, When::After),
        (Call::RegisterName, When::Before),
        (Call::RegisterName, When::After),
        (Call::CreateProfile, When::After),
        (Call::Request, When::Before),
        (Call::Request, When::After),
    ];
    for (call, when) in points {
        let f = fixture(false);
        let lib = f.fake();
        lib.hang_at(call, when);
        let link_id = stash(&f);
        let mut row = invitation_row(&link_id, Some(INVITER));
        row.initial_profile = core_row(PLAIN).initial_profile;
        let draft = f.insert(row);
        f.spawn(&lib, draft, Some(claim_lease(&f)));
        eventually("the scripted call is reached", || {
            lib.hung.load(Ordering::SeqCst)
        });
        f.kill(draft);
        // D13: the link outlives every kill before the identity's record.
        let killed = f.row(draft);
        assert_eq!(
            stashed(&f, &link_id),
            killed.identity.is_none(),
            "{call:?} {when:?}"
        );
        lib.w().hang = None;
        f.drive(&lib, draft, Some(claim_lease(&f)), true);
        done(&f, draft);
        let w = lib.w();
        assert_eq!(
            (w.funds, w.claims, w.identity_creates),
            (0, 1, 1),
            "{call:?} {when:?}"
        );
        assert_eq!(w.names.len(), 1, "{call:?} {when:?}");
        assert_eq!(w.profiles.len(), 1, "{call:?} {when:?}");
        assert_eq!(w.requests.len(), 1, "{call:?} {when:?}");
        drop(w);
        assert!(!stashed(&f, &link_id), "{call:?} {when:?}");
    }
}

/// Review m2: after a restart at `KeysPrepared` the claim step waits for
/// the bring-up's discovery before its checks, so a claim that landed
/// unrecorded is adopted even when its link is gone since.
#[test]
fn a_restart_at_the_claim_waits_for_discovery_before_the_link() {
    let f = fixture(false);
    let lib = f.fake();
    lib.hang_at(Call::Claim, When::Before);
    let link_id = stash(&f);
    let draft = f.insert(invitation_row(&link_id, Some(INVITER)));
    f.spawn(&lib, draft, Some(claim_lease(&f)));
    eventually("the claim is reached", || lib.hung.load(Ordering::SeqCst));
    f.kill(draft);
    lib.w().hang = None;
    let index = f.row(draft).identity_index.expect("keys prepared");
    // Held, the link is not forgotten; gone from the disk, it is gone.
    gone(&f, &link_id);
    lib.w().claim_wait = Some(RegistrationWait::Startup);
    let lease = claim_lease(&f);
    f.drive(&lib, draft, Some(lease.clone()), false);
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.error.as_deref()),
        (Phase::KeysPrepared, None)
    );
    assert_eq!(
        f.status(&lib, draft).waiting,
        Some(RegistrationWait::Startup)
    );
    // Discovery finds the identity the lost answer left.
    lib.w().claim_wait = None;
    lib.w().identities.push((index, claimed_id()));
    f.drive(&lib, draft, Some(lease), true);
    done(&f, draft);
    assert_eq!(lib.w().claims, 0);
}

/// A claim Platform executed whose answer was lost: its identity is
/// adopted, never claimed again.
#[test]
fn a_claim_that_landed_before_its_error_is_adopted() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w()
        .claim_script
        .push_back(invitation::ClaimScript::LandsThenUnavailable);
    let draft = f.insert(invitation_row(&stash(&f), Some(INVITER)));
    // Adopted at once: no wait, and no second claim.
    f.drive(&lib, draft, Some(claim_lease(&f)), false);
    done(&f, draft);
    let w = lib.w();
    assert_eq!((w.claims, w.identity_creates), (1, 1));
    drop(w);
    // The network failing outright waits and tries again.
    let lib = f.fake();
    lib.w()
        .claim_script
        .push_back(invitation::ClaimScript::Unavailable);
    let draft = f.insert(invitation_row(&stash(&f), Some(INVITER)));
    let lease = claim_lease(&f);
    f.drive(&lib, draft, Some(lease.clone()), false);
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.error.as_deref()),
        (Phase::KeysPrepared, None)
    );
    f.drive(&lib, draft, Some(lease), true);
    done(&f, draft);
    let w = lib.w();
    assert_eq!((w.claims, w.identity_creates), (2, 1));
}

/// D12: a link gone before the claim fails for good; D11: a restored row
/// adopts the seed's identity at its index without the link.
#[test]
fn a_link_gone_before_the_claim() {
    let f = fixture(false);
    let lib = f.fake();
    let link_id = stash(&f);
    let draft = f.insert(invitation_row(&link_id, Some(INVITER)));
    f.engine
        .block_on(f.session.forget_invitation(link_id.clone()))
        .unwrap();
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    let row = f.row(draft);
    assert_eq!(
        (row.error.as_deref(), row.retryable),
        (Some("invitation.expired"), false)
    );
    assert_eq!(lib.w().claims, 0);

    let f = fixture(false);
    let lib = f.fake();
    // D26: ours among the seed's identities at the index, listed after
    // another one.
    lib.w().identities.push((0, OTHER.into()));
    lib.w().identities.push((0, claimed_id()));
    let mut row = invitation_row(&"0".repeat(32), Some(INVITER));
    row.phase = Phase::KeysPrepared.as_str().into();
    row.identity_index = Some(0);
    let draft = f.insert(row);
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    done(&f, draft);
    assert_eq!(lib.w().claims, 0);
}

/// A sealed link while the vault is locked parks the claim for the unlock.
#[test]
fn a_sealed_link_waits_for_the_unlock() {
    let f = fixture(true);
    let lib = f.fake();
    let mut row = invitation_row(&stash(&f), Some(INVITER));
    row.phase = Phase::KeysPrepared.as_str().into();
    row.identity_index = Some(0);
    let draft = f.insert(row);
    f.session.lock_vault_sync().unwrap();
    f.drive(&lib, draft, None, false);
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.error.as_deref()),
        (Phase::KeysPrepared, None)
    );
    assert_eq!(
        f.status(&lib, draft).waiting,
        Some(RegistrationWait::Unlock)
    );
    assert_eq!(lib.w().claims, 0);
}

/// D7 again after the hold: the bring-up found an identity meanwhile.
#[test]
fn an_identity_found_after_the_hold_refuses_the_claim() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().identities.push((0, "Other1dentity".into()));
    let draft = f.insert(invitation_row(&stash(&f), Some(INVITER)));
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    let row = f.row(draft);
    assert_eq!(phase(&row), Phase::Draft);
    assert_eq!(
        row.error.as_deref(),
        Some("invitation.already_has_identity")
    );
    assert_eq!(lib.w().claims, 0);
}

/// §4: the request is idempotent, a refusal for good ends `Done`, the
/// network waits, and a link without an inviter sends none.
#[test]
fn the_request_to_the_inviter() {
    // Refused for good (the inviter cannot receive one): Done, logged.
    let f = fixture(false);
    let lib = f.fake();
    lib.w().request_error = Some(ContactError::Ineligible {
        reason: Eligibility::ContactCannotReceive,
    });
    let draft = f.insert(invitation_row(&stash(&f), Some(INVITER)));
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    done(&f, draft);
    assert!(lib.w().requests.is_empty());

    // The network: waits at ProfileCreated, then sends.
    let lib = f.fake();
    lib.w().request_error = Some(ContactError::Platform(PlatformError::Unavailable));
    let draft = f.insert(invitation_row(&stash(&f), Some(INVITER)));
    let lease = claim_lease(&f);
    f.drive(&lib, draft, Some(lease.clone()), false);
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.error.as_deref()),
        (Phase::ProfileCreated, None)
    );
    lib.w().request_error = None;
    f.drive(&lib, draft, Some(lease), true);
    done(&f, draft);
    assert_eq!(lib.w().requests.len(), 1);

    // No inviter: no request, straight to Done.
    let f = fixture(false);
    let lib = f.fake();
    let draft = f.insert(invitation_row(&stash(&f), None));
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    done(&f, draft);
    assert!(lib.w().requests.is_empty());
}

/// D21: a link without an islock claims once its funding is chain-locked;
/// the funding checks' refusals map to the claim's.
#[test]
fn the_claim_spends_the_vouchers_outpoint_once_it_can_be_proved() {
    use crate::platform::invitation_check::Voucher;
    use flow::{ClaimError, claim_outpoint};
    let voucher = |chain_locked| {
        Ok(Ok(Voucher {
            outpoint: outpoint(),
            duffs: 20_000_000,
            chain_locked,
        }))
    };
    assert_eq!(claim_outpoint(true, voucher(false)), Ok(outpoint()));
    assert_eq!(claim_outpoint(true, voucher(true)), Ok(outpoint()));
    assert_eq!(claim_outpoint(false, voucher(true)), Ok(outpoint()));
    assert_eq!(
        claim_outpoint(false, voucher(false)),
        Err(ClaimError::NotChainLocked)
    );
    assert_eq!(
        claim_outpoint(false, Ok(Err(InvitationInvalidReason::AssetLockNotFound))),
        Err(ClaimError::Failed(PlatformError::Unavailable))
    );
    assert_eq!(
        claim_outpoint(true, Ok(Err(InvitationInvalidReason::WrongNetwork))),
        Err(ClaimError::Invalid)
    );
    assert_eq!(
        claim_outpoint(true, Err(PlatformError::Unavailable)),
        Err(ClaimError::Failed(PlatformError::Unavailable))
    );
}

/// Review M1: a consumed lock is "claimed" only when the engine's own
/// lookup finds no identity there, or someone else's; a failed lookup
/// retries the claim, and ours is adopted (D26).
#[test]
fn a_consumed_lock_is_claimed_only_when_the_identity_is_not_ours() {
    use flow::{ClaimError, consumed_verdict};
    let ours = our_keys();
    let adopted = Ok(claimed_id());
    let retried = Err(ClaimError::Failed(PlatformError::Unavailable));
    let claimed = Err(ClaimError::Claimed);
    assert_eq!(consumed_verdict(Ok(landed(&ours)), &ours), adopted);
    assert_eq!(
        consumed_verdict(Err(PlatformError::Unavailable), &ours),
        retried
    );
    assert_eq!(consumed_verdict(Ok(None), &ours), claimed);
    let theirs = BTreeMap::from([(0, key(0, 2)), (1, key(1, 4))]);
    assert_eq!(consumed_verdict(Ok(landed(&theirs)), &ours), claimed);
    let fewer = BTreeMap::from([(0, key(0, 2))]);
    assert_eq!(consumed_verdict(Ok(landed(&fewer)), &ours), claimed);
    let more = BTreeMap::from([(0, key(0, 2)), (1, key(1, 3)), (2, key(2, 5))]);
    assert_eq!(consumed_verdict(Ok(landed(&more)), &ours), claimed);
    // The lookups: again while nothing is found, three at most; a failed
    // lookup once.
    let f = fixture(false);
    let lookups = |script: Vec<Result<Option<Identity>, PlatformError>>| {
        let script = Mutex::new(VecDeque::from(script));
        let calls = AtomicU32::new(0);
        let verdict = f.engine.block_on(flow::consumed_lookup(
            || {
                calls.fetch_add(1, Ordering::SeqCst);
                let next = script.lock().unwrap().pop_front().expect("scripted");
                async move { next }
            },
            &ours,
        ));
        (verdict, calls.load(Ordering::SeqCst))
    };
    assert_eq!(
        lookups(vec![Ok(None), Ok(None), Ok(landed(&ours))]),
        (adopted, 3)
    );
    assert_eq!(
        lookups(vec![Ok(None), Ok(None), Ok(None)]),
        (claimed.clone(), 3)
    );
    assert_eq!(lookups(vec![Err(PlatformError::Unavailable)]), (retried, 1));
    assert_eq!(lookups(vec![Ok(landed(&theirs))]), (claimed, 1));
}

/// Review m1 (D26): a claim refused as consumed whose identity the
/// engine's own lookup finds with our keys is the claim's result at once,
/// never a second claim of the same bytes.
#[test]
fn a_consumed_claim_of_our_identity_is_adopted_without_a_resend() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().claim_script.push_back(ClaimScript::ConsumedOurs);
    let link_id = stash(&f);
    let draft = f.insert(invitation_row(&link_id, Some(INVITER)));
    let lease = claim_lease(&f);
    // A step that waits ends this run: the first run reaches the identity.
    f.drive(&lib, draft, Some(lease.clone()), false);
    let row = f.row(draft);
    assert!(phase(&row) >= Phase::IdentityRegistered, "{row:?}");
    assert_eq!(row.identity, Some(claimed_id()));
    f.drive(&lib, draft, Some(lease), true);
    done(&f, draft);
    let w = lib.w();
    assert_eq!((w.funds, w.claims, w.identity_creates), (0, 1, 1));
    drop(w);
    assert!(!stashed(&f, &link_id));
}

/// Review m4 and r2 n7 (D26): another identity the seed made at the row's
/// index (on another device, during the claim) is not adopted: its credits
/// are not spent and the link is kept. It holds the claim's keys, which
/// Platform would refuse, so the row fails before any claim is sent.
#[test]
fn another_identity_at_the_index_is_not_the_claims() {
    let unclaimed = |f: &Fixture, lib: &Fake, draft: i64, link_id: &str| {
        let row = f.row(draft);
        assert_eq!(
            (
                phase(&row),
                row.identity,
                row.error.as_deref(),
                row.retryable
            ),
            (
                Phase::KeysPrepared,
                None,
                Some("invitation.already_has_identity"),
                false
            )
        );
        let w = lib.w();
        assert!(w.names.is_empty() && w.requests.is_empty());
        drop(w);
        assert!(stashed(f, link_id));
    };
    let keys_prepared = |f: &Fixture, link_id: &str| {
        let mut row = invitation_row(link_id, Some(INVITER));
        row.phase = Phase::KeysPrepared.as_str().into();
        row.identity_index = Some(0);
        f.insert(row)
    };
    // Found before the claim: no claim is sent.
    let f = fixture(false);
    let lib = f.fake();
    let link_id = stash(&f);
    let draft = keys_prepared(&f, &link_id);
    lib.w().identities.push((0, OTHER.into()));
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    unclaimed(&f, &lib, draft, &link_id);
    assert_eq!(lib.w().claims, 0);
    // Found after a failed claim: not adopted; the next run fails the row.
    let f = fixture(false);
    let lib = f.fake();
    let link_id = stash(&f);
    let draft = keys_prepared(&f, &link_id);
    lib.w()
        .claim_script
        .push_back(ClaimScript::OtherAppearsThenUnavailable);
    let lease = claim_lease(&f);
    f.drive(&lib, draft, Some(lease.clone()), false);
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.identity, row.error),
        (Phase::KeysPrepared, None, None)
    );
    f.drive(&lib, draft, Some(lease), true);
    unclaimed(&f, &lib, draft, &link_id);
    assert_eq!(lib.w().claims, 1);
}

/// Review r2 n5 (D26): an identity the library answers with that is not
/// the stored one is never adopted; the row fails, the link kept.
#[test]
fn a_claim_answering_another_identity_fails_without_adopting_it() {
    let f = fixture(false);
    let lib = f.fake();
    let link_id = stash(&f);
    lib.w().claim_script.push_back(ClaimScript::OtherId);
    let draft = f.insert(invitation_row(&link_id, Some(INVITER)));
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    let row = f.row(draft);
    assert_eq!(
        (
            phase(&row),
            row.identity,
            row.error.as_deref(),
            row.retryable
        ),
        (Phase::KeysPrepared, None, Some("internal"), false)
    );
    let w = lib.w();
    assert_eq!(w.claims, 1);
    assert!(w.names.is_empty() && w.requests.is_empty());
    drop(w);
    assert!(stashed(&f, &link_id));
}

/// Review r2 m6 (D8): "Finish transfers" never runs a claim, whatever
/// grant it is given: a claim holds no lock, and runs only under a grant
/// its resume checked for duffs.
#[test]
fn finish_transfers_leaves_claims_to_their_resume() {
    let f = fixture(false);
    let lib = f.fake();
    let link_id = stash(&f);
    let mut row = invitation_row(&link_id, Some(INVITER));
    row.phase = Phase::IdentityRegistered.as_str().into();
    row.identity_index = Some(0);
    row.identity = Some(claimed_id());
    let draft = f.insert(row);
    lib.w().identities.push((0, claimed_id()));
    let before = f.row(draft);
    let report = f
        .engine
        .block_on(registration::finish(
            &f.session,
            f.wallet,
            &*lib,
            f.grant(COIN, 10 * COIN * 1000, Credential::None),
        ))
        .unwrap();
    assert_eq!(
        (report.resumed, report.completed, report.still_pending),
        (0, 0, 0)
    );
    assert_eq!(f.row(draft), before);
    assert!(lib.w().names.is_empty());
    // Its resume finishes it.
    let grant = f.grant(0, 10 * COIN * 1000, Credential::None);
    f.engine
        .block_on(registration::resume(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            &draft.to_string(),
            Some(grant),
        ))
        .unwrap();
    f.settle(draft);
    done(&f, draft);
    assert_eq!(lib.w().funds, 0);
}

/// Review m2: D8 holds for a resumed claim too: a grant with duffs, or a
/// lease with a funding signer, is refused, and the row is left as it was.
#[test]
fn a_resumed_claim_refuses_a_grant_with_duffs() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().claim_script.push_back(ClaimScript::Unavailable);
    let draft = f.insert(invitation_row(&stash(&f), Some(INVITER)));
    f.drive(&lib, draft, Some(claim_lease(&f)), false);
    let before = f.row(draft);
    assert_eq!(phase(&before), Phase::KeysPrepared);
    let resume = |grant: String| {
        f.engine
            .block_on(registration::resume(
                &f.session,
                f.wallet,
                Arc::clone(&lib),
                &draft.to_string(),
                Some(grant),
            ))
            .map_err(|e| e.code())
    };
    assert_eq!(
        resume(f.grant(1, 10 * COIN * 1000, Credential::None)),
        Err("platform.grant_invalid")
    );
    let funded = f
        .engine
        .block_on(f.session.begin_lease(
            f.wallet,
            FlowKind::Registration,
            &[f.grant(1, 10 * COIN * 1000, Credential::None)],
        ))
        .unwrap();
    assert_eq!(
        resume(crate::platform::lease::lease_string(&funded.id())),
        Err("platform.grant_invalid")
    );
    assert_eq!(f.row(draft), before, "refused before anything is written");
    assert_eq!(lib.w().claims, 1);
    // A grant without duffs resumes it.
    let grant = f.grant(0, 10 * COIN * 1000, Credential::None);
    assert_eq!(resume(grant), Ok(()));
    f.settle(draft);
    done(&f, draft);
    assert_eq!(lib.w().funds, 0);
}

/// The stash entry's hold (DP5-03 D12), as the vault keeps it on disk.
fn held(f: &Fixture, link_id: &str) -> bool {
    f.session
        .vault()
        .dir()
        .join("links")
        .join(format!("{link_id}.hold"))
        .exists()
}

fn discard_code(f: &Fixture, lib: &Arc<Fake>, draft: i64) -> Result<(), &'static str> {
    f.discard(lib, draft).map_err(|e| e.code())
}

/// The stash entry `link_id` deleted out of band (another process, a
/// restore): an open claim's hold keeps it from the vault's own deletes.
fn gone(f: &Fixture, link_id: &str) {
    let links = f.session.vault().dir().join("links");
    std::fs::remove_file(links.join(format!("{link_id}.dwl"))).unwrap();
    assert!(!stashed(f, link_id));
}

/// DP5-02 F5 (DP5-03 D11, D12): `start` holds the claim's link; a claim
/// that released its signatures and failed retryably cannot be discarded
/// (it may have landed), and resumes to one identity.
#[test]
fn a_claim_that_may_have_landed_is_not_discarded() {
    let f = fixture(false);
    let lib = f.fake();
    let link_id = stash(&f);
    lib.hang_at(Call::Claim, When::Before);
    let quote = f
        .engine
        .block_on(registration::quote(&*lib, &invite(&link_id)))
        .unwrap();
    let grant = f.grant(0, quote.grant.max_credits, Credential::None);
    let draft: i64 = f
        .engine
        .block_on(registration::start(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            invite(&link_id),
            grant,
        ))
        .unwrap()
        .parse()
        .unwrap();
    eventually("the claim is reached", || lib.hung.load(Ordering::SeqCst));
    f.kill(draft);
    lib.w().hang = None;
    assert!(held(&f, &link_id), "held from the start");
    lib.w()
        .claim_script
        .push_back(invitation::ClaimScript::ReleasedThenUnavailable);
    let lease = claim_lease(&f);
    f.drive(&lib, draft, Some(lease.clone()), false);
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.error.as_deref()),
        (Phase::KeysPrepared, None)
    );
    assert_eq!(discard_code(&f, &lib, draft), Err("invalid_argument"));
    assert!(stashed(&f, &link_id) && held(&f, &link_id));
    f.drive(&lib, draft, Some(lease), true);
    done(&f, draft);
    assert_eq!(lib.w().identity_creates, 1);
    // Done: the link is wiped with its hold.
    assert!(!stashed(&f, &link_id) && !held(&f, &link_id));
}

/// DP5-03 D11: a claim row is discarded while nothing of its claim can be
/// on the network (killed before any signature, or refused for good), and
/// the discard lets the link go; once its identity exists it is not.
#[test]
fn a_claim_row_is_discarded_only_while_nothing_landed() {
    // Killed before the library signed: NotSent.
    let f = fixture(false);
    let lib = f.fake();
    lib.hang_at(Call::Claim, When::Before);
    let link_id = stash(&f);
    let draft = f.insert(invitation_row(&link_id, Some(INVITER)));
    f.spawn(&lib, draft, Some(claim_lease(&f)));
    eventually("the claim is reached", || lib.hung.load(Ordering::SeqCst));
    assert!(held(&f, &link_id), "held by the claim step");
    f.kill(draft);
    lib.w().hang = None;
    assert_eq!(discard_code(&f, &lib, draft), Ok(()));
    assert!(stashed(&f, &link_id), "the link stays for another try");
    assert!(!held(&f, &link_id), "let go by the discard");

    // Refused for good after signing: nothing more can land.
    let lib = f.fake();
    let link_id = stash(&f);
    lib.w()
        .claim_script
        .push_back(invitation::ClaimScript::DuplicateKeys);
    let draft = f.insert(invitation_row(&link_id, Some(INVITER)));
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    assert!(!held(&f, &link_id), "let go once failed for good");
    assert_eq!(discard_code(&f, &lib, draft), Ok(()));

    // The identity registered, the name not yet: refused.
    let lib = f.fake();
    lib.hang_at(Call::RegisterName, When::Before);
    let link_id = stash(&f);
    let draft = f.insert(invitation_row(&link_id, Some(INVITER)));
    f.spawn(&lib, draft, Some(claim_lease(&f)));
    eventually("the name is reached", || lib.hung.load(Ordering::SeqCst));
    f.kill(draft);
    lib.w().hang = None;
    assert!(f.row(draft).identity.is_some());
    assert_eq!(discard_code(&f, &lib, draft), Err("invalid_argument"));
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    done(&f, draft);
    assert_eq!(discard_code(&f, &lib, draft), Ok(()));

    // A restored row with its identity: no signature of this process is
    // out, and still refused.
    let lib = f.fake();
    let link_id = stash(&f);
    let draft = f.insert(NewRegistration {
        identity_index: Some(0),
        identity: Some(claimed_id()),
        phase: Phase::IdentityRegistered.as_str().into(),
        ..invitation_row(&link_id, Some(INVITER))
    });
    assert_eq!(discard_code(&f, &lib, draft), Err("invalid_argument"));
}

/// DP5-03 D12: `start` holds the link before the claim step (here while
/// the bring-up runs), and a claim failed for good before its claim step
/// lets it go.
#[test]
fn a_started_claim_holds_its_link_until_it_fails_for_good() {
    let f = fixture(false);
    let lib = f.fake();
    let link_id = stash(&f);
    lib.w().claim_wait = Some(RegistrationWait::Startup);
    let quote = f
        .engine
        .block_on(registration::quote(&*lib, &invite(&link_id)))
        .unwrap();
    let grant = f.grant(0, quote.grant.max_credits, Credential::None);
    let draft: i64 = f
        .engine
        .block_on(registration::start(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            invite(&link_id),
            grant,
        ))
        .unwrap()
        .parse()
        .unwrap();
    eventually("the claim waits for the bring-up", || {
        f.status(&lib, draft).waiting == Some(RegistrationWait::Startup)
    });
    assert_eq!(phase(&f.row(draft)), Phase::Draft);
    assert!(held(&f, &link_id), "held from the start");
    f.kill(draft);
    // The bring-up finds an identity of the seed (DP5-02 D7).
    lib.w().claim_wait = None;
    lib.w().identities.push((0, OTHER.into()));
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.error.as_deref(), row.retryable),
        (Phase::Draft, Some("invitation.already_has_identity"), false)
    );
    assert!(stashed(&f, &link_id) && !held(&f, &link_id));
}

/// DP5-02 F11 (DP5-03 D13): Platform's duplicate-key refusal fails the
/// claim for good as `invitation.already_has_identity`; a resume does not
/// claim (and pay) again.
#[test]
fn a_duplicate_key_refusal_fails_for_good() {
    let f = fixture(false);
    let lib = f.fake();
    let link_id = stash(&f);
    lib.w()
        .claim_script
        .push_back(invitation::ClaimScript::DuplicateKeys);
    let draft = f.insert(invitation_row(&link_id, Some(INVITER)));
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.error.as_deref(), row.retryable),
        (
            Phase::KeysPrepared,
            Some("invitation.already_has_identity"),
            false
        )
    );
    assert_eq!(row.identity, None);
    assert!(stashed(&f, &link_id), "the user dismisses it");
    let resumed = f.engine.block_on(registration::resume(
        &f.session,
        f.wallet,
        Arc::clone(&lib),
        &draft.to_string(),
        None,
    ));
    assert!(resumed.is_err(), "{resumed:?}");
    assert_eq!(lib.w().claims, 1);
}

/// The classifier, on every shape the SDK reports a refusal in.
#[test]
fn duplicate_keys_are_recognised_in_every_shape() {
    use dash_sdk::error::StateTransitionBroadcastError;
    use dpp::consensus::ConsensusError;
    use dpp::consensus::basic::BasicError;
    use dpp::consensus::basic::identity::DuplicatedIdentityPublicKeyIdBasicError;
    use dpp::consensus::state::identity::duplicated_identity_public_key_id_state_error::DuplicatedIdentityPublicKeyIdStateError;
    use dpp::consensus::state::identity::duplicated_identity_public_key_state_error::DuplicatedIdentityPublicKeyStateError;
    use dpp::consensus::state::state_error::StateError;
    let state: ConsensusError = StateError::DuplicatedIdentityPublicKeyIdStateError(
        DuplicatedIdentityPublicKeyIdStateError::new(vec![0]),
    )
    .into();
    let basic: ConsensusError = BasicError::DuplicatedIdentityPublicKeyIdBasicError(
        DuplicatedIdentityPublicKeyIdBasicError::new(vec![1]),
    )
    .into();
    // Another refusal of a key, not the state check's.
    let other: ConsensusError = StateError::DuplicatedIdentityPublicKeyStateError(
        DuplicatedIdentityPublicKeyStateError::new(vec![0]),
    )
    .into();
    let broadcast = |cause: Option<ConsensusError>| {
        dash_sdk::Error::StateTransitionBroadcastError(StateTransitionBroadcastError {
            code: 1,
            message: "refused".into(),
            cause,
        })
    };
    let check_tx = |e: ConsensusError| {
        dash_sdk::Error::Protocol(dpp::ProtocolError::ConsensusError(Box::new(e)))
    };
    for e in [
        broadcast(Some(state.clone())),
        broadcast(Some(basic.clone())),
        check_tx(state.clone()),
        check_tx(basic),
        dash_sdk::Error::NoAvailableAddressesToRetry(Box::new(broadcast(Some(state)))),
    ] {
        assert!(flow::is_duplicate_keys(&e), "{e:?}");
        assert!(flow::claim_refused_for_keys(&e, &OutPoint::null()), "{e:?}");
    }
    for e in [
        broadcast(None),
        broadcast(Some(other.clone())),
        check_tx(other),
        dash_sdk::Error::Generic("duplicated identity public key id".into()),
    ] {
        assert!(!flow::is_duplicate_keys(&e), "{e:?}");
    }
}

/// DP5-03 D24: a claim refused for the seed's other identity refuses the
/// next claim's start, before a row or a call, until it is discarded. A Core
/// registration is not refused by it, nor is a claim by a row that ends
/// another way.
#[test]
fn a_claim_refused_for_its_keys_refuses_the_next_claim() {
    let f = fixture(false);
    let lib = f.fake();
    let link_id = stash(&f);
    lib.w()
        .claim_script
        .push_back(invitation::ClaimScript::DuplicateKeys);
    let draft = f.insert(invitation_row(&link_id, Some(INVITER)));
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    let start = |req: RegistrationRequest, duffs: u64| {
        let grant = f.grant(duffs, 10 * COIN * 1000, Credential::None);
        f.engine
            .block_on(registration::start(
                &f.session,
                f.wallet,
                Arc::clone(&lib),
                req,
                grant,
            ))
            .map_err(|e| e.code())
    };
    assert_eq!(
        start(invite(&link_id), 0),
        Err("invitation.already_has_identity")
    );
    assert_eq!((f.rows().len(), lib.w().claims), (1, 1));
    // A Core registration's refusal is its own (the grant has no duffs).
    assert_eq!(start(request(PLAIN), 0), Err("platform.grant_exceeded"));
    let fail = |code: &str, retryable: bool| {
        let row = f.row(draft);
        let failed = RegistrationChange {
            failure: Some(Some((code.into(), retryable))),
            ..Default::default()
        };
        let wallet = f.wallet.to_string();
        (f.appdb())
            .update_registration(&wallet, draft, row.owner, &row.phase, true, &failed, 2)
            .unwrap();
    };
    // A row that may run again is the start's blocker before the gate.
    fail("invitation.already_has_identity", true);
    assert_eq!(start(invite(&link_id), 0), Err("registration.in_progress"));
    fail("invitation.expired", false);
    let started = start(invite(&link_id), 0).expect("another code refuses nothing");
    f.settle(started.parse().unwrap());
}

/// DP5-03 D24: Platform's replay refusal of the claim's own lock reads as
/// the refusal it repeats; another lock's, or another refusal, does not.
#[test]
fn a_replayed_claim_is_recognised_for_its_own_lock_only() {
    use dash_sdk::error::StateTransitionBroadcastError;
    use dpp::consensus::ConsensusError;
    use dpp::consensus::basic::BasicError;
    use dpp::consensus::basic::identity::{
        IdentityAssetLockStateTransitionReplayError,
        IdentityAssetLockTransactionOutPointAlreadyConsumedError,
    };
    let own = OutPoint::new(Txid::from_byte_array([7; 32]), 1);
    let replay = |txid: Txid, vout: usize| -> ConsensusError {
        BasicError::IdentityAssetLockStateTransitionReplayError(
            IdentityAssetLockStateTransitionReplayError::new(txid, vout, [0; 32].into()),
        )
        .into()
    };
    let broadcast = |cause: ConsensusError| {
        dash_sdk::Error::StateTransitionBroadcastError(StateTransitionBroadcastError {
            code: 1,
            message: "refused".into(),
            cause: Some(cause),
        })
    };
    let check_tx = |e: ConsensusError| {
        dash_sdk::Error::Protocol(dpp::ProtocolError::ConsensusError(Box::new(e)))
    };
    let ours = replay(own.txid, 1);
    for e in [
        broadcast(ours.clone()),
        check_tx(ours.clone()),
        dash_sdk::Error::NoAvailableAddressesToRetry(Box::new(broadcast(ours))),
    ] {
        assert!(flow::is_claim_replay(&e, &own), "{e:?}");
        assert!(flow::claim_refused_for_keys(&e, &own), "{e:?}");
        assert!(!flow::is_duplicate_keys(&e), "{e:?}");
    }
    let consumed: ConsensusError =
        BasicError::IdentityAssetLockTransactionOutPointAlreadyConsumedError(
            IdentityAssetLockTransactionOutPointAlreadyConsumedError::new(own.txid, 1),
        )
        .into();
    for e in [
        broadcast(replay(own.txid, 0)),
        broadcast(replay(Txid::from_byte_array([8; 32]), 1)),
        check_tx(consumed),
        dash_sdk::Error::Generic("is trying to be replayed".into()),
    ] {
        assert!(!flow::claim_refused_for_keys(&e, &own), "{e:?}");
    }
}

/// DP5-03 D7: the opt-out stores no inviter, plans no request credits and
/// sends nothing; a request refused for good is kept on the done row (D8).
#[test]
fn the_opt_out_and_the_refused_request_are_on_the_row() {
    let f = fixture(false);
    let lib = f.fake();
    let link_id = stash(&f);
    let mut req = invite(&link_id);
    let asked = f.engine.block_on(registration::quote(&*lib, &req)).unwrap();
    if let RegistrationFunding::Invitation {
        request_inviter, ..
    } = &mut req.funding
    {
        *request_inviter = false;
    }
    let quote = f.engine.block_on(registration::quote(&*lib, &req)).unwrap();
    assert_eq!(
        asked.grant.max_credits - quote.grant.max_credits,
        CONTACT_REQUEST_CREDITS
    );
    let grant = f.grant(0, quote.grant.max_credits, Credential::None);
    let draft: i64 = f
        .engine
        .block_on(registration::start(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            req,
            grant,
        ))
        .unwrap()
        .parse()
        .unwrap();
    f.settle(draft);
    done(&f, draft);
    assert!(lib.w().requests.is_empty());
    let status = f.status(&lib, draft);
    assert_eq!(
        status.invitation,
        Some(registration::RegistrationInvitation {
            link_id,
            inviter: None,
            request_failure: None,
        })
    );

    let lib = f.fake();
    let refusal = ContactError::Ineligible {
        reason: Eligibility::ContactCannotReceive,
    };
    let code = refusal.code();
    lib.w().request_error = Some(refusal);
    let link_id = stash(&f);
    let draft = f.insert(invitation_row(&link_id, Some(INVITER)));
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    done(&f, draft);
    let status = f.status(&lib, draft);
    assert_eq!(
        status.invitation,
        Some(registration::RegistrationInvitation {
            link_id,
            inviter: Some(INVITER.into()),
            request_failure: Some(code.into()),
        })
    );
    assert_eq!(
        f.status(&lib, f.insert(core_row("other1"))).invitation,
        None
    );
}

/// DP5-03 D20 (Opus r1 H1): a claim row's resume grant is the row's own,
/// with no duffs: the credits of the steps it has left, whatever the
/// invitation's status reads now. A Core row has none.
#[test]
fn a_claim_rows_resume_grant_is_its_own() {
    let f = fixture(false);
    let lib = f.fake();
    let link_id = stash(&f);
    let grant = |row: NewRegistration| {
        let draft = f.insert(row);
        f.engine
            .block_on(registration::resume_grant(
                &f.session,
                f.wallet,
                &*lib,
                &draft.to_string(),
            ))
            .map(|g| (g.max_duffs, g.max_credits))
            .map_err(|e| e.code())
    };
    let at = |phase: Phase, inviter: Option<&str>| NewRegistration {
        phase: phase.as_str().into(),
        identity_index: Some(0),
        identity: (phase >= Phase::IdentityRegistered).then(claimed_id),
        temp_label: None,
        initial_profile: None,
        ..invitation_row(&link_id, inviter)
    };
    let all = flow::IDENTITY_CREATE_CREDITS + NAME_CREDITS + CONTACT_REQUEST_CREDITS;
    assert_eq!(grant(at(Phase::Draft, Some(INVITER))), Ok((0, all)));
    assert_eq!(
        grant(at(Phase::KeysPrepared, None)),
        Ok((0, flow::IDENTITY_CREATE_CREDITS + NAME_CREDITS)),
        "opted out: no request"
    );
    assert_eq!(
        grant(NewRegistration {
            temp_label: Some("bob-tmp-1".into()),
            initial_profile: core_row(PLAIN).initial_profile,
            ..at(Phase::Draft, Some(INVITER))
        }),
        Ok((0, all + NAME_CREDITS + flow::PROFILE_CREDITS)),
        "the temporary name and the profile"
    );
    assert_eq!(
        grant(at(Phase::IdentityRegistered, Some(INVITER))),
        Ok((0, NAME_CREDITS + CONTACT_REQUEST_CREDITS))
    );
    assert_eq!(
        grant(at(Phase::NameRegistered, Some(INVITER))),
        Ok((0, CONTACT_REQUEST_CREDITS))
    );
    assert_eq!(grant(at(Phase::Done, Some(INVITER))), Ok((0, 0)));
    // The row's claim landed: the invitation reads claimed, the grant holds.
    lib.w().invitation = Some(InvitationStatus::Claimed);
    assert_eq!(
        grant(at(Phase::IdentityRegistered, Some(INVITER))),
        Ok((0, NAME_CREDITS + CONTACT_REQUEST_CREDITS))
    );
    assert_eq!(grant(core_row(PLAIN)), Err("invalid_argument"));
}

/// DP5-02 F5 (Opus r1 M1): after its signatures were released, a claim
/// whose link then went is still refused a discard: the link's absence
/// does not prove the claim did not land. Its hold is let go.
#[test]
fn a_released_claim_whose_link_went_is_not_discarded() {
    let f = fixture(false);
    let lib = f.fake();
    let link_id = stash(&f);
    lib.w()
        .claim_script
        .push_back(invitation::ClaimScript::ReleasedThenUnavailable);
    let draft = f.insert(invitation_row(&link_id, Some(INVITER)));
    let lease = claim_lease(&f);
    f.drive(&lib, draft, Some(lease.clone()), false);
    assert!(held(&f, &link_id));
    // Held, the vault does not forget it; gone from the disk, it is gone.
    let forgot = f
        .engine
        .block_on(f.session.forget_invitation(link_id.clone()));
    assert!(forgot.is_err(), "{forgot:?}");
    gone(&f, &link_id);
    f.drive(&lib, draft, Some(lease), true);
    let row = f.row(draft);
    assert_eq!(
        (row.error.as_deref(), row.retryable),
        (Some("invitation.expired"), false)
    );
    assert!(!held(&f, &link_id), "let go once failed for good");
    assert_eq!(discard_code(&f, &lib, draft), Err("invalid_argument"));
    assert_eq!(lib.w().claims, 1);
}

/// DP5-03 D12 (Opus r1 M3, L4): the stash is the network's; a claim that
/// ends lets its link go unless another wallet's open claim names it, and
/// the bring-up lets go of every hold no open claim names.
#[test]
fn holds_settle_across_wallets_and_at_the_bring_up() {
    let f = fixture(false);
    let lib = f.fake();
    let link_id = stash(&f);
    let other = stash_other(&f);
    let hold = |id: &str| {
        f.engine
            .block_on(f.session.hold_invitation(id.into(), true))
    };
    hold(&link_id);
    hold(&other);
    // Another wallet's open claim of the same link, failed retryably.
    let theirs = f
        .appdb()
        .insert_registration(THEIRS, &invitation_row(&link_id, None), 1)
        .unwrap();
    let row = f.appdb().registration_row(THEIRS, theirs).unwrap().unwrap();
    let failed = RegistrationChange {
        failure: Some(Some(("platform.unavailable".into(), true))),
        ..Default::default()
    };
    f.appdb()
        .update_registration(THEIRS, theirs, row.owner, &row.phase, false, &failed, 2)
        .unwrap();
    let row = f.appdb().registration_row(THEIRS, theirs).unwrap().unwrap();
    assert_eq!(
        (row.error.as_deref(), row.retryable),
        (Some("platform.unavailable"), true)
    );
    // Ours fails for good: the link stays held for theirs.
    lib.w().identities.push((0, OTHER.into()));
    let draft = f.insert(invitation_row(&link_id, Some(INVITER)));
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    assert_eq!(
        f.row(draft).error.as_deref(),
        Some("invitation.already_has_identity")
    );
    assert!(held(&f, &link_id), "another wallet's claim holds it");
    // The bring-up: `other` names no open claim.
    f.engine.block_on(f.session.settle_invitation_holds());
    assert!(held(&f, &link_id) && !held(&f, &other));
    // Theirs ends (here: removed with its wallet); the next bring-up lets go.
    let row = f.appdb().registration_row(THEIRS, theirs).unwrap().unwrap();
    assert!(
        f.appdb()
            .delete_registration(THEIRS, theirs, row.owner, &row.phase, None)
            .unwrap()
    );
    f.engine.block_on(f.session.settle_invitation_holds());
    assert!(!held(&f, &link_id));
    assert!(stashed(&f, &link_id) && stashed(&f, &other), "entries kept");
    // A newer build's claim row (a phase this build does not know) is
    // open: its hold stays (DEC-171 (3), Opus r2 9).
    hold(&other);
    f.insert(NewRegistration {
        phase: "aPhaseOfANewerBuild".into(),
        ..invitation_row(&other, None)
    });
    f.engine.block_on(f.session.settle_invitation_holds());
    assert!(held(&f, &other));
}

/// Another wallet of the network.
const THEIRS: &str = "00000000-0000-4000-8000-0000000000aa";

/// A second stashed invitation, not [`LINK`].
fn stash_other(f: &Fixture) -> String {
    f.engine
        .block_on(
            f.session
                .stash_invitation(BearerSecret::new(format!("{LINK}&n=2"))),
        )
        .unwrap()
}

/// DP5-03 D12 (Opus r2 E20): a claim the index assignment fails for good
/// (every identity index used) lets its link go too.
#[test]
fn a_claim_without_an_identity_index_lets_its_link_go() {
    let f = fixture(false);
    let lib = f.fake();
    let link_id = stash(&f);
    f.engine
        .block_on(f.session.hold_invitation(link_id.clone(), true));
    f.insert(NewRegistration {
        identity_index: Some(crate::platform::registration_ledger::MAX_IDENTITY_INDEX),
        phase: Phase::KeysPrepared.as_str().into(),
        ..core_row(PLAIN)
    });
    let draft = f.insert(invitation_row(&link_id, Some(INVITER)));
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.error.as_deref(), row.retryable),
        (Phase::Draft, Some("internal"), false)
    );
    assert!(stashed(&f, &link_id) && !held(&f, &link_id));
    assert_eq!(lib.w().claims, 0);
}
