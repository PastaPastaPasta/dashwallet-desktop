//! `Governance::vote` end to end on a regtest session (MG-05 §5, §7): a
//! voting lease, the vault's vote signer, the dispatch fence and the store.

use std::collections::HashMap;

use dashcore::hashes::{Hash, hash160};
use dashcore::secp256k1::{PublicKey, SecretKey};
use dashcore::{OutPoint, Txid};
use dw_governance::object::{GovernanceObject, display_hex};
use dw_governance::store::ResolvedMasternode;
use dw_governance::vote::GovernanceVote;
use dw_vault::{Credential, GrantPurpose};

use super::super::collateral::tests::{snapshot, summary};
use super::super::reads::now;
use super::super::{Governance, GovernanceError, VoteCast, VoteOutcome};
use crate::masternode_keys::MasternodeKeyRole;
use crate::platform::FlowKind;
use crate::platform::lease::ArtifactId;
use crate::platform::lease::session_tests::{Fixture, fixture};
use crate::platform::{DispatchState, LeaseStateView};

/// The list's masternodes: A and B vote with the keychain keys at indexes
/// 0 and 3, C with an attached key, D's key is someone else's, E's
/// collateral is unresolved, and G is only listed.
const A: [u8; 32] = [0xa1; 32];
const B: [u8; 32] = [0xb2; 32];
const C: [u8; 32] = [0xc3; 32];
const D: [u8; 32] = [0xd4; 32];
const E: [u8; 32] = [0xe5; 32];
const G: [u8; 32] = [0x67; 32];
/// Not listed.
const F: [u8; 32] = [0xf6; 32];
const ATTACHED: [u8; 32] = [7; 32];

struct Voting {
    f: Fixture,
    gov: Governance,
    proposal: String,
    parent: [u8; 32],
    collateral: HashMap<[u8; 32], OutPoint>,
}

fn collateral_of(mn: &[u8; 32]) -> OutPoint {
    OutPoint::new(Txid::from_byte_array([mn[0] ^ 0xff; 32]), 1)
}

fn attached_id() -> [u8; 20] {
    let sk = SecretKey::from_secret_bytes(ATTACHED).unwrap();
    hash160::Hash::hash(&PublicKey::from_secret_key(&sk).serialize()).to_byte_array()
}

/// A synced store holding one proposal and the list above, with A–D's
/// collaterals resolved.
fn voting() -> Voting {
    let f = fixture(false);
    let gov = f.session.governance();
    let keys = f
        .engine
        .block_on(
            f.session
                .masternode_keys(f.wallet, MasternodeKeyRole::Voting, 0, 20),
        )
        .unwrap();
    let id = |index: usize| {
        let pk = hex::decode(&keys[index].public_key_hex).unwrap();
        hash160::Hash::hash(&pk).to_byte_array()
    };
    f.engine
        .block_on(
            f.session
                .vault_op(|v| v.store_masternode_key(&C, "voting", &ATTACHED)),
        )
        .unwrap();

    let rt = &f.session.governance;
    rt.set_enabled_flag(true);
    rt.shared.progress().last_synced_at = Some(1);
    let t = now();
    let data = format!(
        r#"{{"end_epoch":{},"name":"p","payment_address":"yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf","payment_amount":5,"start_epoch":1,"type":1,"url":"http://x.y"}}"#,
        t + 100_000
    );
    let p = GovernanceObject::new_proposal(1, t - 10, data.into_bytes());
    let parent = p.hash();
    {
        let mut store = rt.shared.store();
        store.set_confirmations(1);
        store.add_object(p, None, t).unwrap();
    }
    let entries = [
        (A, id(0)),
        (B, id(3)),
        (C, attached_id()),
        (D, [9; 20]),
        (E, id(5)),
        (G, [8; 20]),
    ];
    rt.apply_list(snapshot(
        entries
            .iter()
            .map(|(h, k)| summary(*h, *k, false))
            .collect(),
    ));
    let collateral: HashMap<_, _> = [A, B, C, D]
        .iter()
        .map(|h| (*h, collateral_of(h)))
        .collect();
    let resolved = entries
        .iter()
        .filter_map(|(h, k)| {
            Some((
                *collateral.get(h)?,
                ResolvedMasternode {
                    pro_tx_hash: *h,
                    voting_key_id: *k,
                    operator_key: [2; 48],
                    weight: 1,
                },
            ))
        })
        .collect();
    rt.shared.store().set_resolved(resolved);
    Voting {
        f,
        gov,
        proposal: display_hex(&parent),
        parent,
        collateral,
    }
}

impl Voting {
    fn grant(&self) -> String {
        self.f.grant(GrantPurpose::Governance, Credential::None)
    }

    fn vote(
        &self,
        outcome: VoteOutcome,
        mns: &[[u8; 32]],
        grant: String,
    ) -> Result<Vec<VoteCast>, GovernanceError> {
        let mns = mns.iter().map(display_hex).collect();
        self.f.engine.block_on(self.gov.vote(
            self.f.wallet,
            self.proposal.clone(),
            outcome,
            mns,
            grant,
        ))
    }

    /// The outpoints of the store's current votes on the proposal.
    fn stored(&self) -> Vec<OutPoint> {
        let store = self.f.session.governance.shared.store();
        let mut out: Vec<_> = store.votes_on(&self.parent).map(|v| v.outpoint).collect();
        out.sort();
        out
    }

    /// The session's leases that are not ended.
    fn leases(&self) -> usize {
        let views = self.f.session.leases().unwrap();
        views
            .iter()
            .filter(|l| l.state != LeaseStateView::Ended)
            .count()
    }
}

fn reasons(cast: &[VoteCast]) -> Vec<(String, Option<&str>)> {
    cast.iter()
        .map(|c| (c.pro_tx_hash.clone(), c.rejected.as_deref()))
        .collect()
}

/// One vote per masternode: the keychain's and the attached key's are
/// signed, stored and fenced as sent; the others are refused each with its
/// reason; the lease ends with the call.
#[test]
fn vote_signs_and_stores_one_vote_per_masternode() {
    let v = voting();
    let cast = v
        .vote(VoteOutcome::Yes, &[A, B, C, D, E, F], v.grant())
        .unwrap();
    assert_eq!(
        reasons(&cast),
        vec![
            (display_hex(&A), None),
            (display_hex(&B), None),
            (display_hex(&C), None),
            (display_hex(&D), Some("no_voting_key")),
            (display_hex(&E), Some("collateral_unresolved")),
            (display_hex(&F), Some("unknown_masternode")),
        ]
    );
    let mut expected: Vec<_> = [A, B, C].iter().map(|h| v.collateral[h]).collect();
    expected.sort();
    assert_eq!(v.stored(), expected);
    for c in &cast[..3] {
        // No peer is connected: stored, announced on the next connection.
        assert_eq!(c.peers, 0);
        let hash = dw_governance::object::parse_display_hex(c.hash.as_ref().unwrap()).unwrap();
        assert_eq!(
            v.f.session
                .leases
                .dispatch_status(v.f.wallet, ArtifactId(hash)),
            Some(DispatchState::Sent)
        );
    }
    assert!(cast[3..].iter().all(|c| c.hash.is_none() && c.peers == 0));
    assert_eq!(v.leases(), 0, "the lease ends with the call");

    // A second batch at once is refused by the store's rate rules (Core's
    // `CGovernanceObject::ProcessVote`): the same vote again is a
    // duplicate, a changed one too often.
    let again = v.vote(VoteOutcome::Yes, &[A], v.grant()).unwrap();
    assert_eq!(again[0].rejected.as_deref(), Some("duplicate"));
    let changed = v.vote(VoteOutcome::No, &[A], v.grant()).unwrap();
    assert!(
        matches!(
            changed[0].rejected.as_deref(),
            Some("too_often" | "same_timestamp")
        ),
        "{changed:?}"
    );
    assert_eq!(v.stored(), expected);
}

/// Whole-call errors come before the lease: the grant is not consumed and
/// votes afterwards.
#[test]
fn whole_call_errors_leave_the_grant_unused() {
    let v = voting();
    let grant = v.grant();
    let code = |r: Result<Vec<VoteCast>, GovernanceError>| r.unwrap_err().code();
    let a = display_hex(&A);
    for mns in [vec![], vec![A, A], vec![A, B, C, D, E, F, G]] {
        assert_eq!(
            code(v.vote(VoteOutcome::Yes, &mns, grant.clone())),
            "invalid_argument",
            "{mns:?}"
        );
    }
    let malformed = v.f.engine.block_on(v.gov.vote(
        v.f.wallet,
        v.proposal.clone(),
        VoteOutcome::Yes,
        vec![a[..63].to_owned()],
        grant.clone(),
    ));
    assert_eq!(code(malformed), "invalid_argument");
    let unknown = v.f.engine.block_on(v.gov.vote(
        v.f.wallet,
        display_hex(&[0x55; 32]),
        VoteOutcome::Yes,
        vec![a.clone()],
        grant.clone(),
    ));
    assert_eq!(code(unknown), "governance.proposal_not_found");
    let rt = &v.f.session.governance;
    rt.shared.progress().last_synced_at = None;
    assert_eq!(
        code(v.vote(VoteOutcome::Yes, &[A], grant.clone())),
        "governance.not_synced"
    );
    rt.shared.progress().last_synced_at = Some(1);
    rt.set_enabled_flag(false);
    assert_eq!(
        code(v.vote(VoteOutcome::Yes, &[A], grant.clone())),
        "governance.sync_disabled"
    );
    rt.set_enabled_flag(true);
    assert_eq!(v.leases(), 0);
    let cast = v.vote(VoteOutcome::Yes, &[A], grant.clone()).unwrap();
    assert_eq!(cast[0].rejected, None);
    // Then it is spent.
    assert_eq!(
        code(v.vote(VoteOutcome::Yes, &[B], grant)),
        "platform.grant_invalid"
    );
}

/// A voting lease takes a `Governance` grant only, and a vote enters the
/// fence only under a voting lease.
#[test]
fn a_vote_needs_a_governance_grant_and_a_voting_lease() {
    let v = voting();
    for purpose in [
        GrantPurpose::Spend { max_duffs: 1 },
        GrantPurpose::MasternodeOp { max_duffs: 1 },
        GrantPurpose::SignMessage,
    ] {
        let g = v.f.grant(purpose, Credential::None);
        let e = v.vote(VoteOutcome::Yes, &[A], g).unwrap_err();
        assert!(matches!(e, GovernanceError::Platform(_)), "{e:?}");
        assert_eq!(e.code(), "platform.grant_invalid");
    }
    assert!(v.stored().is_empty());

    // A vote handed off under a masternode lease is refused by the fence.
    let g = v.f.grant(
        GrantPurpose::MasternodeOp { max_duffs: 1 },
        Credential::None,
    );
    let lease = v.f.lease(FlowKind::MasternodeRegistration, &[g]).unwrap();
    let vote = GovernanceVote::funding(
        v.collateral[&A],
        v.parent,
        dw_governance::vote::VoteOutcome::Yes,
        now(),
    );
    let r = v.f.engine.block_on(v.gov.hand_off(&lease, &v.parent, vote));
    assert_eq!(r, Err("cancelled"));
    assert!(v.stored().is_empty());
}

/// A lock revokes the voting lease: a signed vote handed off after it is
/// refused and never reaches the store.
#[test]
fn a_lock_before_hand_off_cancels_the_vote() {
    let v = voting();
    let lease = v.f.lease(FlowKind::Voting, &[v.grant()]).unwrap();
    let signer = lease.vote_signer().unwrap();
    v.f.session.lock_vault_sync().unwrap();
    assert_eq!(
        signer.key_id(&dw_vault::VotingKey::Keychain { index: 0 }),
        Err(dw_vault::SignerError::Locked)
    );
    let vote = GovernanceVote::funding(
        v.collateral[&A],
        v.parent,
        dw_governance::vote::VoteOutcome::Yes,
        now(),
    );
    let r = v.f.engine.block_on(v.gov.hand_off(&lease, &v.parent, vote));
    assert_eq!(r, Err("cancelled"));
    assert!(v.stored().is_empty());
}
