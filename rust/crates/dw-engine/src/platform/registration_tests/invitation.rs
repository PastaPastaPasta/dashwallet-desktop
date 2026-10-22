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
}

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
        }
        .encode(),
        ..core_row(PLAIN)
    }
}

fn invite(link_id: &str) -> RegistrationRequest {
    RegistrationRequest {
        funding: RegistrationFunding::Invitation {
            link_id: link_id.into(),
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
    f.engine
        .block_on(f.session.forget_invitation(link_id))
        .unwrap();
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

/// Review m4 (D26): another identity the seed made at the row's index (on
/// another device, during the claim) is not adopted: its credits are not
/// spent, the link is not wiped, and the claim is the row's own.
#[test]
fn another_identity_at_the_index_is_not_the_claims() {
    const OTHER: &str = "Other1dentityAtTheIndex";
    // Before the claim: the claim runs, and is its own identity.
    let f = fixture(false);
    let lib = f.fake();
    let link_id = stash(&f);
    let mut row = invitation_row(&link_id, Some(INVITER));
    row.phase = Phase::KeysPrepared.as_str().into();
    row.identity_index = Some(0);
    let draft = f.insert(row);
    lib.w().identities.push((0, OTHER.into()));
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    done(&f, draft);
    let w = lib.w();
    assert_eq!(w.claims, 1);
    assert!(w.names.iter().all(|(id, _)| *id == claimed_id()));
    assert_eq!(w.requests, [(claimed_id(), INVITER.to_owned())]);
    drop(w);
    // After a failed claim: the error stands, the link stays.
    let f = fixture(false);
    let lib = f.fake();
    let link_id = stash(&f);
    let mut row = invitation_row(&link_id, Some(INVITER));
    row.phase = Phase::KeysPrepared.as_str().into();
    row.identity_index = Some(0);
    let draft = f.insert(row);
    lib.w().identities.push((0, OTHER.into()));
    lib.w().claim_script.push_back(ClaimScript::Claimed);
    f.drive(&lib, draft, Some(claim_lease(&f)), true);
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.identity, row.error.as_deref()),
        (Phase::KeysPrepared, None, Some("invitation.claimed"))
    );
    let w = lib.w();
    assert!(w.names.is_empty() && w.requests.is_empty());
    drop(w);
    assert!(stashed(&f, &link_id));
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
