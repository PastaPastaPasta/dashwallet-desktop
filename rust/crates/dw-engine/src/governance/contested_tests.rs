//! `contested_names` and `vote_name` on a regtest session (MG-07 §5): a
//! voting lease, the vault's name-vote signer and the dispatch fence, with
//! Platform scripted.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dash_sdk::error::StateTransitionBroadcastError;
use dashcore::hashes::{Hash, hash160, sha256};
use dashcore::secp256k1::{PublicKey, SecretKey};
use dpp::consensus::ConsensusError;
use dpp::consensus::codes::ErrorWithCode;
use dpp::consensus::signature::{
    BasicECDSAError, IdentityNotFoundError, MissingPublicKeyError, PublicKeyIsDisabledError,
};
use dpp::consensus::state::state_error::StateError;
use dpp::consensus::state::voting::masternode_incorrect_voter_identity_id_error::MasternodeIncorrectVoterIdentityIdError;
use dpp::consensus::state::voting::masternode_incorrect_voting_address_error::MasternodeIncorrectVotingAddressError;
use dpp::consensus::state::voting::masternode_not_found_error::MasternodeNotFoundError;
use dpp::consensus::state::voting::masternode_vote_already_present_error::MasternodeVoteAlreadyPresentError;
use dpp::consensus::state::voting::masternode_voted_too_many_times::MasternodeVotedTooManyTimesError;
use dpp::consensus::state::voting::vote_choice_not_allowed_for_vote_poll_error::VoteChoiceNotAllowedForVotePollError;
use dpp::consensus::state::voting::vote_poll_not_available_for_voting_error::VotePollNotAvailableForVotingError;
use dpp::consensus::state::voting::vote_poll_not_found_error::VotePollNotFoundError;
use dpp::identifier::MasternodeIdentifiers;
use dpp::platform_value::Bytes20;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::state_transition::StateTransition;
use dpp::state_transition::masternode_vote_transition::MasternodeVoteTransition;
use dpp::version::PlatformVersion;
use dpp::voting::vote_choices::resource_vote_choice::ResourceVoteChoice;
use dpp::voting::vote_info_storage::contested_document_vote_poll_stored_info::ContestedDocumentVotePollStatus;
use dpp::voting::vote_polls::VotePoll;
use dpp::voting::vote_polls::contested_document_resource_vote_poll::ContestedDocumentResourceVotePoll;
use dpp::voting::votes::Vote;
use dpp::voting::votes::resource_vote::accessors::v0::ResourceVoteGettersV0;
use dw_governance::object::display_hex;
use dw_vault::{Credential, GrantPurpose};

use super::super::collateral::tests::summary;
use super::super::records::{
    ContestedName, MasternodeType, NameChoice, NameContender, NameVoteCast, NameVoter,
    NameVoterState, NameVotingKey,
};
use super::super::{Governance, GovernanceError};
use super::{
    Broadcast, Cast, NameVoteNet, Refusal, broadcast_outcome, consensus, platform_order,
    proof_outcome, refusal, set_votes_used,
};
use crate::events::unix_now;
use crate::masternode_keys::MasternodeKeyRole;
use crate::masternodes::AttachedKeyRole;
use crate::masternodes::follow::set_key_id;
use crate::platform::DispatchState;
use crate::platform::contest_watch::{ContestNet, Poll, PollWinner};
use crate::platform::lease::ArtifactId;
use crate::platform::lease::session_tests::{Fixture, fixture};
use crate::platform::{LeaseStateView, PlatformError};

/// The list's masternodes (wire order): A and B vote with the keychain
/// keys at indexes 0 and 3 (B an evonode), C with an attached key, D's key
/// is someone else's, G is only listed, and H has an attached key that is
/// not the list's.
const A: [u8; 32] = [0xa1; 32];
const B: [u8; 32] = [0xb2; 32];
const C: [u8; 32] = [0xc3; 32];
const D: [u8; 32] = [0xd4; 32];
const G: [u8; 32] = [0x67; 32];
const H: [u8; 32] = [0x78; 32];
/// Not listed.
const F: [u8; 32] = [0xf6; 32];
const ATTACHED: [u8; 32] = [7; 32];
const STRAY: [u8; 32] = [8; 32];
/// Two contenders.
const ID1: [u8; 32] = [0x11; 32];
const ID2: [u8; 32] = [0x22; 32];
const LABEL: &str = "c0ntest";

/// Platform, scripted: the end-date index, the polls, each masternode's
/// choices, nonces, and what each broadcast comes to (`Counted` once the
/// script is empty; a scripted `Counted` or executed refusal is the
/// broadcast's `Accepted`, then the wait's answer). `delay` holds each
/// broadcast that long; `delivered` counts the broadcasts that ran to
/// their end.
#[derive(Default)]
pub(crate) struct FakeNames {
    ends: Mutex<BTreeMap<String, u64>>,
    polls: Mutex<HashMap<String, Poll>>,
    votes: Mutex<HashMap<[u8; 32], BTreeMap<String, NameChoice>>>,
    nonces: Mutex<HashMap<[u8; 32], u64>>,
    no_nonce: Mutex<HashSet<[u8; 32]>>,
    casts: Mutex<VecDeque<Cast>>,
    sent: Mutex<Vec<Vec<u8>>>,
    diagnosis: Mutex<Option<&'static str>>,
    down: AtomicBool,
    on_cast: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    delay: Mutex<Option<Duration>>,
    delivered: AtomicUsize,
    pending: Mutex<Option<Cast>>,
    /// Accepted broadcasts get no proof.
    unproved: AtomicBool,
    /// Broadcasts DAPI takes are answered "already exists".
    duplicate: AtomicBool,
    waits: AtomicUsize,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl FakeNames {
    fn up(&self) -> Result<(), PlatformError> {
        match self.down.load(Ordering::SeqCst) {
            true => Err(PlatformError::Unavailable),
            false => Ok(()),
        }
    }

    fn sent(&self) -> Vec<MasternodeVoteTransition> {
        lock(&self.sent)
            .iter()
            .map(|b| {
                let st = StateTransition::deserialize_from_bytes_untrusted_exact_in_version(
                    b,
                    PlatformVersion::latest(),
                )
                .unwrap();
                let StateTransition::MasternodeVote(v) = st else {
                    panic!("{st:?}");
                };
                v
            })
            .collect()
    }
}

impl ContestNet for FakeNames {
    async fn page(&self, normalized: &str, _: u16) -> Result<Poll, PlatformError> {
        self.up()?;
        Ok(lock(&self.polls)
            .get(normalized)
            .cloned()
            .unwrap_or_default())
    }

    async fn end_page(
        &self,
        from: u64,
        limit: u16,
    ) -> Result<BTreeMap<String, u64>, PlatformError> {
        self.up()?;
        let mut ends: Vec<_> = lock(&self.ends)
            .iter()
            .filter(|(_, e)| **e >= from)
            .map(|(l, e)| (l.clone(), *e))
            .collect();
        ends.sort_by_key(|(_, e)| *e);
        Ok(ends.into_iter().take(usize::from(limit)).collect())
    }
}

impl NameVoteNet for FakeNames {
    fn votes_allowed(&self) -> u32 {
        5
    }

    async fn votes_of(
        &self,
        pro_tx_hash: [u8; 32],
    ) -> Result<BTreeMap<String, NameChoice>, PlatformError> {
        self.up()?;
        Ok(lock(&self.votes)
            .get(&pro_tx_hash)
            .cloned()
            .unwrap_or_default())
    }

    async fn nonce(&self, voter: [u8; 32]) -> Result<u64, PlatformError> {
        if lock(&self.no_nonce).contains(&voter) {
            return Err(PlatformError::Unavailable);
        }
        let mut nonces = lock(&self.nonces);
        let n = nonces.entry(voter).or_default();
        *n += 1;
        Ok(*n)
    }

    async fn broadcast(&self, bytes: Vec<u8>) -> Broadcast {
        lock(&self.sent).push(bytes);
        if let Some(hook) = lock(&self.on_cast).as_ref() {
            hook();
        }
        let delay = *lock(&self.delay);
        if let Some(d) = delay {
            tokio::time::sleep(d).await;
        }
        self.delivered.fetch_add(1, Ordering::SeqCst);
        match lock(&self.casts).pop_front().unwrap_or(Cast::Counted) {
            c @ (Cast::Counted | Cast::Refused { executed: true, .. }) => {
                *lock(&self.pending) = Some(c);
                match self.duplicate.load(Ordering::SeqCst) {
                    true => Broadcast::Duplicate,
                    false => Broadcast::Accepted,
                }
            }
            Cast::Refused { refusal, .. } => Broadcast::Refused(refusal),
            Cast::NotSent => Broadcast::NotSent,
            Cast::Unknown => Broadcast::Unknown,
        }
    }

    async fn wait(&self, _: Vec<u8>) -> Cast {
        self.waits.fetch_add(1, Ordering::SeqCst);
        let answer = lock(&self.pending).take().unwrap_or(Cast::Counted);
        match self.unproved.load(Ordering::SeqCst) {
            true => Cast::Unknown,
            false => answer,
        }
    }

    async fn diagnose(&self, _: [u8; 32], _: [u8; 20]) -> Option<&'static str> {
        *lock(&self.diagnosis)
    }
}

struct Names {
    f: Fixture,
    gov: Governance,
    net: Arc<FakeNames>,
    /// The keychain's voting key ids by index.
    keychain: Vec<[u8; 20]>,
    now: u64,
}

fn key_id(secret: [u8; 32]) -> [u8; 20] {
    let sk = SecretKey::from_secret_bytes(secret).unwrap();
    hash160::Hash::hash(&PublicKey::from_secret_key(&sk).serialize()).to_byte_array()
}

fn b58(id: [u8; 32]) -> String {
    Identifier::from(id).to_string(Encoding::Base58)
}

fn towards(id: [u8; 32]) -> NameChoice {
    NameChoice::TowardsIdentity { identity: b58(id) }
}

/// The list above, C's and H's attached keys with their recorded ids, and
/// Platform with two open contests (`c0ntest`, `dashx`), one resolved
/// (`w0n`) and one ended (`ended`). A already chose ID1 on `c0ntest`.
fn names() -> Names {
    let f = fixture(false);
    let gov = f.session.governance();
    let keys = f
        .engine
        .block_on(
            f.session
                .masternode_keys(f.wallet, MasternodeKeyRole::Voting, 0, 20),
        )
        .unwrap();
    let keychain: Vec<[u8; 20]> = keys
        .iter()
        .map(|k| hash160::Hash::hash(&hex::decode(&k.public_key_hex).unwrap()).to_byte_array())
        .collect();
    let appdb = f.session.live().unwrap().appdb;
    for (mn, key) in [(C, ATTACHED), (H, STRAY)] {
        f.engine
            .block_on(
                f.session
                    .vault_op(move |v| v.store_masternode_key(&mn, "voting", &key)),
            )
            .unwrap();
        set_key_id(
            &appdb,
            &display_hex(&mn),
            AttachedKeyRole::Voting,
            Some(key_id(key)),
        )
        .unwrap();
    }
    let entries = vec![
        summary(A, keychain[0], false),
        summary(B, keychain[3], true),
        summary(C, key_id(ATTACHED), false),
        summary(D, [9; 20], false),
        summary(G, [8; 20], false),
        summary(H, [0x11; 20], false),
    ];
    let rt = &f.session.governance;
    *lock(&rt.name_list) = Some(entries);

    let now = unix_now();
    let net = Arc::new(FakeNames::default());
    lock(&net.ends).extend([
        (LABEL.to_owned(), now + 3600),
        ("dashx".to_owned(), now + 7200),
        ("w0n".to_owned(), now + 100),
        ("ended".to_owned(), now - 10),
    ]);
    let open = Poll {
        winner: None,
        contenders: vec![(ID1.into(), Some(3)), (ID2.into(), Some(1))],
        lock_votes: Some(1),
        abstain_votes: Some(0),
    };
    lock(&net.polls).extend([
        (LABEL.to_owned(), open.clone()),
        (
            "dashx".to_owned(),
            Poll {
                contenders: vec![(ID2.into(), None)],
                ..Poll::default()
            },
        ),
        (
            "w0n".to_owned(),
            Poll {
                winner: Some((PollWinner::Locked, 1)),
                ..open.clone()
            },
        ),
        ("ended".to_owned(), open),
    ]);
    lock(&net.votes).insert(
        platform_order(&A),
        BTreeMap::from([(LABEL.to_owned(), towards(ID1))]),
    );
    *lock(&rt.name_net) = Some(Arc::clone(&net));
    Names {
        f,
        gov,
        net,
        keychain,
        now,
    }
}

impl Names {
    fn grant(&self) -> String {
        self.f.grant(GrantPurpose::Governance, Credential::None)
    }

    fn vote(
        &self,
        label: &str,
        choice: NameChoice,
        mns: &[[u8; 32]],
        grant: String,
    ) -> Result<Vec<NameVoteCast>, GovernanceError> {
        let mns = mns.iter().map(display_hex).collect();
        self.f.engine.block_on(
            self.gov
                .vote_name(self.f.wallet, label.into(), choice, mns, grant),
        )
    }

    fn leases(&self) -> usize {
        let views = self.f.session.leases().unwrap();
        views
            .iter()
            .filter(|l| l.state != LeaseStateView::Ended)
            .count()
    }

    fn status(&self, transition: &str) -> Option<DispatchState> {
        let hash: [u8; 32] = hex::decode(transition).unwrap().try_into().unwrap();
        (self.f.session.leases).dispatch_status(self.f.wallet, ArtifactId(hash))
    }
}

fn reasons(cast: &[NameVoteCast]) -> Vec<(String, Option<&str>)> {
    cast.iter()
        .map(|c| (c.pro_tx_hash.clone(), c.rejected.as_deref()))
        .collect()
}

/// The open contests soonest ending first (resolved and ended ones left
/// out), and the voters: keychain and attached keys, not someone else's
/// or an attached key that is not the list's.
#[test]
fn contested_names_lists_open_contests_and_the_wallets_voters() {
    let n = names();
    let names =
        n.f.engine
            .block_on(n.gov.contested_names(n.f.wallet))
            .unwrap();
    assert_eq!(
        names.contests,
        vec![
            ContestedName {
                label: LABEL.into(),
                ends_at: n.now + 3600,
                contenders: vec![
                    NameContender {
                        identity: b58(ID1),
                        votes: Some(3),
                    },
                    NameContender {
                        identity: b58(ID2),
                        votes: Some(1),
                    },
                ],
                lock_votes: Some(1),
                abstain_votes: Some(0),
                contenders_complete: true,
            },
            ContestedName {
                label: "dashx".into(),
                ends_at: n.now + 7200,
                contenders: vec![NameContender {
                    identity: b58(ID2),
                    votes: None,
                }],
                lock_votes: None,
                abstain_votes: None,
                contenders_complete: true,
            },
        ]
    );
    let voter = |mn: &[u8; 32], kind, weight, key, current| NameVoter {
        pro_tx_hash: display_hex(mn),
        kind,
        weight,
        valid: true,
        key,
        current,
    };
    assert_eq!(
        names.voters,
        vec![
            voter(
                &A,
                MasternodeType::Regular,
                1,
                NameVotingKey::Keychain { index: 0 },
                vec![NameVoterState {
                    label: LABEL.into(),
                    choice: Some(towards(ID1)),
                    votes_used: 0,
                }],
            ),
            voter(
                &B,
                MasternodeType::Evonode,
                4,
                NameVotingKey::Keychain { index: 3 },
                vec![],
            ),
            voter(
                &C,
                MasternodeType::Regular,
                1,
                NameVotingKey::Attached,
                vec![],
            ),
        ]
    );
    assert_eq!(names.votes_allowed, 5);
    assert_eq!(n.keychain.len(), 20);
}

/// One transition per masternode, as Platform builds it: its proTxHash in
/// Platform's order, its voter identity, the next nonce and the choice on
/// DPNS's poll of the label; fenced as sent; counted in `votes_used`.
#[test]
fn vote_name_casts_one_transition_per_masternode() {
    let n = names();
    // A user's spelling is normalized as DPNS does.
    let cast = n
        .vote("C0NTEST", towards(ID2), &[A, B, C, D, F, H], n.grant())
        .unwrap();
    assert_eq!(
        reasons(&cast),
        vec![
            (display_hex(&A), None),
            (display_hex(&B), None),
            (display_hex(&C), None),
            (display_hex(&D), Some("no_voting_key")),
            (display_hex(&F), Some("unknown_masternode")),
            (display_hex(&H), Some("no_voting_key")),
        ]
    );
    assert!(cast[3..].iter().all(|c| c.transition.is_none()));
    let sent = n.net.sent();
    assert_eq!(sent.len(), 3);
    let keys = [n.keychain[0], n.keychain[3], key_id(ATTACHED)];
    let bytes = lock(&n.net.sent).clone();
    for (((v, mn), key), (c, b)) in sent
        .iter()
        .zip([A, B, C])
        .zip(keys)
        .zip(cast.iter().zip(&bytes))
    {
        let MasternodeVoteTransition::V0(v) = v;
        let platform = platform_order(&mn);
        assert_eq!(v.pro_tx_hash.to_buffer(), platform);
        assert_eq!(
            v.voter_identity_id,
            Identifier::create_voter_identifier(&platform, &key)
        );
        assert_eq!(v.nonce, 1);
        let Vote::ResourceVote(vote) = &v.vote;
        let VotePoll::ContestedDocumentResourceVotePoll(poll) = vote.vote_poll();
        assert_eq!(
            poll.index_values,
            vec![
                dpp::platform_value::Value::Text("dash".into()),
                dpp::platform_value::Value::Text(LABEL.into())
            ]
        );
        assert_eq!(
            vote.resource_vote_choice(),
            ResourceVoteChoice::TowardsIdentity(ID2.into())
        );
        let transition = c.transition.as_deref().unwrap();
        assert_eq!(
            transition,
            hex::encode(sha256::Hash::hash(b).to_byte_array())
        );
        assert_eq!(n.status(transition), Some(DispatchState::Sent));
    }
    assert_eq!(n.leases(), 0, "the lease ends with the call");

    let names =
        n.f.engine
            .block_on(n.gov.contested_names(n.f.wallet))
            .unwrap();
    let used: Vec<_> = (names.voters.iter())
        .map(|v| {
            v.current
                .iter()
                .map(|s| (s.label.as_str(), s.votes_used))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(used, vec![vec![(LABEL, 1)]; 3]);
    // The next votes take the next nonces.
    let again = n.vote(LABEL, NameChoice::Lock, &[A], n.grant()).unwrap();
    assert_eq!(again[0].rejected, None);
    let MasternodeVoteTransition::V0(v) = &n.net.sent()[3];
    assert_eq!(v.nonce, 2);
}

/// Whole-call errors come before the lease: the grant is not consumed and
/// votes afterwards.
#[test]
fn whole_call_errors_leave_the_grant_unused() {
    let n = names();
    let grant = n.grant();
    let code = |r: Result<Vec<NameVoteCast>, GovernanceError>| r.unwrap_err().code();
    let vote = |label: &str, choice: NameChoice, mns: &[[u8; 32]]| {
        code(n.vote(label, choice, mns, grant.clone()))
    };
    for label in ["ab", "a.b.c", "-abc", "0123456789abcdefghjk"] {
        assert_eq!(
            vote(label, NameChoice::Abstain, &[A]),
            "invalid_argument",
            "{label}"
        );
    }
    let bad_id = NameChoice::TowardsIdentity {
        identity: "not base58!".into(),
    };
    assert_eq!(vote(LABEL, bad_id, &[A]), "invalid_argument");
    for mns in [vec![], vec![A, A], vec![A, B, C, D, G, H, F]] {
        assert_eq!(vote(LABEL, NameChoice::Abstain, &mns), "invalid_argument");
    }
    assert_eq!(
        vote("zzzz", NameChoice::Abstain, &[A]),
        "governance.contest_not_found"
    );
    for closed in ["w0n", "ended"] {
        assert_eq!(
            vote(closed, NameChoice::Abstain, &[A]),
            "governance.contest_closed",
            "{closed}"
        );
    }
    let e = n.vote(LABEL, towards([0x33; 32]), &[A], grant.clone());
    assert!(matches!(
        e,
        Err(GovernanceError::NotAContender { ref identity }) if *identity == b58([0x33; 32])
    ));
    assert_eq!(code(e), "governance.not_a_contender");
    n.net.down.store(true, Ordering::SeqCst);
    assert_eq!(
        vote(LABEL, NameChoice::Abstain, &[A]),
        "platform.unavailable"
    );
    n.net.down.store(false, Ordering::SeqCst);
    let list = lock(&n.f.session.governance.name_list).take();
    assert_eq!(
        vote(LABEL, NameChoice::Abstain, &[A]),
        "governance.list_unavailable"
    );
    // A malformed or repeated proTxHash is refused before the list is read.
    for mns in [
        vec!["zz".to_owned()],
        vec![display_hex(&A), display_hex(&A)],
    ] {
        let e = n.f.engine.block_on(n.gov.vote_name(
            n.f.wallet,
            LABEL.into(),
            NameChoice::Abstain,
            mns,
            grant.clone(),
        ));
        assert_eq!(e.unwrap_err().code(), "invalid_argument");
    }
    let names = n.f.engine.block_on(n.gov.contested_names(n.f.wallet));
    assert_eq!(names.unwrap_err().code(), "governance.list_unavailable");
    *lock(&n.f.session.governance.name_list) = list;
    assert!(lock(&n.net.sent).is_empty());
    assert_eq!(n.leases(), 0);

    let cast = n
        .vote(LABEL, NameChoice::Abstain, &[A], grant.clone())
        .unwrap();
    assert_eq!(cast[0].rejected, None);
    // Then it is spent.
    assert_eq!(
        vote(LABEL, NameChoice::Abstain, &[B]),
        "platform.grant_invalid"
    );
    // Only a `Governance` grant redeems as the voting flow.
    let g =
        n.f.grant(GrantPurpose::Spend { max_duffs: 1 }, Credential::None);
    assert_eq!(
        code(n.vote(LABEL, NameChoice::Abstain, &[B], g)),
        "platform.grant_invalid"
    );
}

/// Per masternode: the local vote limit and a failed nonce read cast
/// nothing; Platform's refusals are reported by reason, a vote limit
/// records Platform's count, and a voting-key refusal is diagnosed.
#[test]
fn refusals_are_reported_per_masternode() {
    let n = names();
    let appdb = n.f.session.live().unwrap().appdb;
    set_votes_used(&appdb, &A, LABEL, 5).unwrap();
    lock(&n.net.no_nonce).insert(
        Identifier::create_voter_identifier(&platform_order(&B), &n.keychain[3]).to_buffer(),
    );
    let cast = n
        .vote(LABEL, NameChoice::Abstain, &[A, B], n.grant())
        .unwrap();
    assert_eq!(
        reasons(&cast),
        vec![
            (display_hex(&A), Some("vote_limit")),
            (display_hex(&B), Some("platform_unavailable")),
        ]
    );
    assert!(cast.iter().all(|c| c.transition.is_none()));
    assert!(lock(&n.net.sent).is_empty());
    lock(&n.net.no_nonce).clear();
    set_votes_used(&appdb, &A, LABEL, 0).unwrap();

    let refused = |refusal| Cast::Refused {
        refusal,
        executed: false,
    };
    let script = |casts: Vec<Cast>| *lock(&n.net.casts) = casts.into();
    script(vec![
        refused(Refusal::Reason("already_voted")),
        refused(Refusal::VoteLimit { used: 5 }),
        refused(Refusal::VotingKey),
    ]);
    *lock(&n.net.diagnosis) = Some("no_voter_identity");
    let cast = n
        .vote(LABEL, NameChoice::Abstain, &[A, B, C], n.grant())
        .unwrap();
    assert_eq!(
        reasons(&cast),
        vec![
            (display_hex(&A), Some("already_voted")),
            (display_hex(&B), Some("vote_limit")),
            (display_hex(&C), Some("no_voter_identity")),
        ]
    );
    for c in &cast {
        let t = c.transition.as_deref().expect("broadcast");
        assert_eq!(n.status(t), Some(DispatchState::NotSent));
    }
    let names =
        n.f.engine
            .block_on(n.gov.contested_names(n.f.wallet))
            .unwrap();
    let used = |i: usize| {
        names.voters[i]
            .current
            .iter()
            .map(|s| s.votes_used)
            .sum::<u32>()
    };
    assert_eq!((used(0), used(1), used(2)), (0, 5, 0));
    // B is at the limit now: refused here, nothing sent.
    let sent = lock(&n.net.sent).len();
    let cast = n.vote(LABEL, NameChoice::Lock, &[B], n.grant()).unwrap();
    assert_eq!(cast[0].rejected.as_deref(), Some("vote_limit"));
    assert_eq!(lock(&n.net.sent).len(), sent);

    // A voting-key refusal Platform's identity does not explain, a
    // transition that may have left, one that never did, and one executed
    // and refused.
    *lock(&n.net.diagnosis) = None;
    script(vec![
        refused(Refusal::VotingKey),
        Cast::Unknown,
        Cast::NotSent,
    ]);
    let cast = n
        .vote(LABEL, NameChoice::Lock, &[A, C, B], n.grant())
        .unwrap();
    // B is still at the limit.
    assert_eq!(
        reasons(&cast),
        vec![
            (display_hex(&A), Some("refused")),
            (display_hex(&C), Some("unconfirmed")),
            (display_hex(&B), Some("vote_limit")),
        ]
    );
    let c = cast[1].transition.as_deref().unwrap();
    assert_eq!(n.status(c), Some(DispatchState::MaybeSent));
    script(vec![
        Cast::NotSent,
        Cast::Refused {
            refusal: Refusal::Reason("contest_closed"),
            executed: true,
        },
    ]);
    let cast = n.vote(LABEL, NameChoice::Lock, &[A, C], n.grant()).unwrap();
    assert_eq!(
        reasons(&cast),
        vec![
            (display_hex(&A), Some("platform_unavailable")),
            (display_hex(&C), Some("contest_closed")),
        ]
    );
    let c = cast[1].transition.as_deref().unwrap();
    assert_eq!(n.status(c), Some(DispatchState::Sent));
    assert_eq!(n.leases(), 0);
}

/// A lock during the batch revokes the lease: the masternodes after it are
/// `cancelled`, and nothing more is broadcast.
#[test]
fn a_lock_mid_batch_cancels_the_rest() {
    let n = names();
    let session = Arc::clone(&n.f.session);
    *lock(&n.net.on_cast) = Some(Box::new(move || {
        session.lock_vault_sync().unwrap();
    }));
    let cast = n
        .vote(LABEL, NameChoice::Abstain, &[A, B, C], n.grant())
        .unwrap();
    assert_eq!(
        reasons(&cast),
        vec![
            (display_hex(&A), None),
            (display_hex(&B), Some("cancelled")),
            (display_hex(&C), Some("cancelled")),
        ]
    );
    assert_eq!(lock(&n.net.sent).len(), 1);
}

/// Review M1: a broadcast still running at its permit's deadline is cut
/// there, so nothing of it runs after a lock's drain: a lock during a slow
/// broadcast leaves the vote `unconfirmed` (MaybeSent), and the broadcast
/// never finishes.
#[test]
fn a_slow_broadcast_is_cut_at_the_permit_deadline() {
    let n = names();
    let ttl = crate::platform::lease::LeaseConfig::default().permit_ttl;
    *lock(&n.net.delay) = Some(ttl + Duration::from_secs(2));
    let session = Arc::clone(&n.f.session);
    *lock(&n.net.on_cast) = Some(Box::new(move || {
        session.lock_vault_sync().unwrap();
    }));
    let cast = n
        .vote(LABEL, NameChoice::Abstain, &[A, B], n.grant())
        .unwrap();
    assert_eq!(
        reasons(&cast),
        vec![
            (display_hex(&A), Some("unconfirmed")),
            (display_hex(&B), Some("cancelled")),
        ]
    );
    let t = cast[0].transition.as_deref().unwrap();
    assert_eq!(n.status(t), Some(DispatchState::MaybeSent));
    // Past where the dropped broadcast would have finished.
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(lock(&n.net.sent).len(), 1);
    assert_eq!(n.net.delivered.load(Ordering::SeqCst), 0);
    assert_eq!(n.leases(), 0);
}

/// Review r2: a broadcast DAPI took whose proof never comes back is
/// `unconfirmed` with its transition, `Sent` in the fence, and not counted.
#[test]
fn an_unproved_vote_is_sent_but_unconfirmed() {
    let n = names();
    n.net.unproved.store(true, Ordering::SeqCst);
    let cast = n.vote(LABEL, NameChoice::Abstain, &[B], n.grant()).unwrap();
    assert_eq!(reasons(&cast), vec![(display_hex(&B), Some("unconfirmed"))]);
    let t = cast[0].transition.as_deref().unwrap();
    assert_eq!(n.status(t), Some(DispatchState::Sent));
    let appdb = n.f.session.live().unwrap().appdb;
    assert_eq!(super::votes_used(&appdb, &B, LABEL).unwrap(), 0);
}

/// Review r3 m2, n4: a repeat is told by Platform's present choice, as
/// Drive tells it. At the local limit, the present choice again is
/// `already_voted` (Drive checks it before the count). "Already exists"
/// with the present choice again is `already_voted` at once, not a wait
/// for a proof that never comes; with another choice the proof is waited
/// for, and the vote counts.
#[test]
fn a_repeat_reads_platforms_present_choice() {
    let n = names();
    let appdb = n.f.session.live().unwrap().appdb;
    // A's present choice is towards ID1.
    set_votes_used(&appdb, &A, LABEL, 5).unwrap();
    let cast = n.vote(LABEL, towards(ID1), &[A], n.grant()).unwrap();
    assert_eq!(
        reasons(&cast),
        vec![(display_hex(&A), Some("already_voted"))]
    );
    assert!(cast[0].transition.is_none());
    assert!(lock(&n.net.sent).is_empty());
    set_votes_used(&appdb, &A, LABEL, 0).unwrap();

    n.net.duplicate.store(true, Ordering::SeqCst);
    // No proof for A's bytes: a wait would make it `unconfirmed`.
    n.net.unproved.store(true, Ordering::SeqCst);
    let cast = n.vote(LABEL, towards(ID1), &[A], n.grant()).unwrap();
    assert_eq!(
        reasons(&cast),
        vec![(display_hex(&A), Some("already_voted"))]
    );
    let t = cast[0].transition.as_deref().expect("broadcast");
    assert_eq!(n.status(t), Some(DispatchState::Sent));
    assert_eq!(n.net.waits.load(Ordering::SeqCst), 0);
    assert_eq!(super::votes_used(&appdb, &A, LABEL).unwrap(), 0);

    // B has no present vote: its duplicate may still execute.
    n.net.unproved.store(false, Ordering::SeqCst);
    let cast = n.vote(LABEL, NameChoice::Abstain, &[B], n.grant()).unwrap();
    assert_eq!(reasons(&cast), vec![(display_hex(&B), None)]);
    assert_eq!(n.net.waits.load(Ordering::SeqCst), 1);
    assert_eq!(super::votes_used(&appdb, &B, LABEL).unwrap(), 1);
    assert_eq!(n.leases(), 0);
}

/// Review m1, m4: the SDK's answers as the fence and the caller see them.
#[test]
fn broadcast_and_proof_answers_classify() {
    let present = ConsensusError::from(StateError::MasternodeVoteAlreadyPresentError(
        MasternodeVoteAlreadyPresentError::new(Identifier::from([5; 32]), poll()),
    ));
    let typed = || {
        dash_sdk::Error::StateTransitionBroadcastError(StateTransitionBroadcastError {
            code: present.code(),
            message: "present".into(),
            cause: Some(present.clone()),
        })
    };
    let untyped = || {
        dash_sdk::Error::StateTransitionBroadcastError(StateTransitionBroadcastError {
            code: 1,
            message: "no cause".into(),
            cause: None,
        })
    };
    let at_check_tx = || {
        dash_sdk::Error::Protocol(dpp::ProtocolError::ConsensusError(Box::new(
            present.clone(),
        )))
    };
    // A consensus error DAPI sent that this client cannot decode.
    let undecoded = || dash_sdk::Error::Protocol(dpp::ProtocolError::Generic("decode".into()));
    let timeout = || dash_sdk::Error::Generic("timeout".into());
    let already = Refusal::Reason("already_voted");

    assert_eq!(broadcast_outcome(Ok(())), Broadcast::Accepted);
    assert_eq!(
        broadcast_outcome(Err(dash_sdk::Error::AlreadyExists("same bytes".into()))),
        Broadcast::Duplicate
    );
    assert_eq!(
        broadcast_outcome(Err(at_check_tx())),
        Broadcast::Refused(already)
    );
    assert_eq!(
        broadcast_outcome(Err(dash_sdk::Error::NoAvailableAddressesToRetry(Box::new(
            at_check_tx()
        )))),
        Broadcast::Refused(already)
    );
    assert_eq!(
        broadcast_outcome(Err(undecoded())),
        Broadcast::Refused(Refusal::Reason("refused"))
    );
    assert_eq!(broadcast_outcome(Err(timeout())), Broadcast::Unknown);

    let executed = |refusal| Cast::Refused {
        refusal,
        executed: true,
    };
    assert_eq!(proof_outcome(Ok(())), Cast::Counted);
    assert_eq!(proof_outcome::<()>(Err(typed())), executed(already));
    assert_eq!(
        proof_outcome::<()>(Err(untyped())),
        executed(Refusal::Reason("refused"))
    );
    assert_eq!(proof_outcome::<()>(Err(timeout())), Cast::Unknown);
}

fn poll() -> VotePoll {
    VotePoll::ContestedDocumentResourceVotePoll(ContestedDocumentResourceVotePoll::default())
}

/// Every refusal of MG-07 §3 from the consensus error Drive sends, in each
/// shape the SDK hands it over.
#[test]
fn refusals_map_from_platforms_consensus_errors() {
    let mn = Identifier::from([5; 32]);
    let state = |e: StateError| ConsensusError::from(e);
    let cases: Vec<(ConsensusError, Refusal)> = vec![
        (
            state(StateError::MasternodeVoteAlreadyPresentError(
                MasternodeVoteAlreadyPresentError::new(mn, poll()),
            )),
            Refusal::Reason("already_voted"),
        ),
        (
            state(StateError::MasternodeVotedTooManyTimesError(
                MasternodeVotedTooManyTimesError::new(mn, 5, 5),
            )),
            Refusal::VoteLimit { used: 5 },
        ),
        (
            state(StateError::VotePollNotAvailableForVotingError(
                VotePollNotAvailableForVotingError::new(
                    poll(),
                    ContestedDocumentVotePollStatus::Locked,
                ),
            )),
            Refusal::Reason("contest_closed"),
        ),
        (
            state(StateError::VotePollNotFoundError(
                VotePollNotFoundError::new(poll()),
            )),
            Refusal::Reason("contest_not_found"),
        ),
        (
            state(StateError::MasternodeNotFoundError(
                MasternodeNotFoundError::new(mn),
            )),
            Refusal::Reason("masternode_not_found"),
        ),
        (
            state(StateError::MasternodeIncorrectVotingAddressError(
                MasternodeIncorrectVotingAddressError::new(mn, Bytes20([1; 20]), Bytes20([2; 20])),
            )),
            Refusal::Reason("wrong_voting_key"),
        ),
        (
            state(StateError::MasternodeIncorrectVoterIdentityIdError(
                MasternodeIncorrectVoterIdentityIdError::new(mn, mn, Identifier::from([6; 32])),
            )),
            Refusal::Reason("wrong_voting_key"),
        ),
        (
            state(StateError::VoteChoiceNotAllowedForVotePollError(
                VoteChoiceNotAllowedForVotePollError::new(poll(), ResourceVoteChoice::Lock),
            )),
            Refusal::Reason("choice_not_allowed"),
        ),
        (IdentityNotFoundError::new(mn).into(), Refusal::VotingKey),
        (MissingPublicKeyError::new(0).into(), Refusal::VotingKey),
        (PublicKeyIsDisabledError::new(0).into(), Refusal::VotingKey),
        (
            BasicECDSAError::new("bad".into()).into(),
            Refusal::Reason("refused"),
        ),
    ];
    for (e, expected) in cases {
        assert_eq!(refusal(&e), expected, "{e:?}");
        let code = e.code();
        let shapes = [
            dash_sdk::Error::StateTransitionBroadcastError(StateTransitionBroadcastError {
                code,
                message: e.to_string(),
                cause: Some(e.clone()),
            }),
            dash_sdk::Error::Protocol(dpp::ProtocolError::ConsensusError(Box::new(e.clone()))),
            dash_sdk::Error::NoAvailableAddressesToRetry(Box::new(dash_sdk::Error::Protocol(
                dpp::ProtocolError::ConsensusError(Box::new(e.clone())),
            ))),
        ];
        for shape in &shapes {
            assert_eq!(consensus(shape), Some(&e), "{shape:?}");
        }
    }
    assert_eq!(consensus(&dash_sdk::Error::Generic("x".into())), None);
    assert_eq!(
        consensus(&dash_sdk::Error::StateTransitionBroadcastError(
            StateTransitionBroadcastError {
                code: 1,
                message: "no cause".into(),
                cause: None,
            }
        )),
        None
    );
}
