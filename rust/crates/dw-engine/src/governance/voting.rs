//! Voting (mg-engine §2.4): the wallet's voting masternodes, relaying a
//! vote signed elsewhere, and casting votes under a `Governance` grant
//! (MG-05 §5).

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

use base64::Engine as _;
use dashcore::hashes::{Hash, hash160};
use dashcore::{OutPoint, Txid};
use dw_governance::object::{display_hex, parse_display_hex};
use dw_governance::store::{GovernanceStore, ObjectKind, VoteRejection};
use dw_governance::tally::CurrentVote;
use dw_governance::vote::GovernanceVote;
use dw_vault::{FundingOutcome, FundingVote, SignerError, VoteSigner, VotingKey};

use super::reads::{is_proposal, now, parse_hash};
use super::records::{
    MasternodeType, RawVote, VoteCast, VoteOutcome, VoteRelayed, VotingMasternode,
};
use super::sync::ListSnapshot;
use super::{Governance, GovernanceError};
use crate::masternode_keys::MasternodeKeyRole;
use crate::platform::lease::{
    AdmitRequest, ArtifactId, ArtifactKind, DispatchScope, Lease, Outcome, Verdict,
};
use crate::platform::{FlowKind, PlatformError};
use crate::{EngineError, WalletId};

/// The voting-key indexes the wallet looks at (the keychain view's
/// provider-key gap, MG-02 §4.3).
pub const VOTING_KEY_INDEXES: u32 = 20;

impl Governance {
    /// The masternodes of the tip list whose voting key the wallet derives
    /// (indexes 0–19), with each one's own vote seen on `proposal` and, for
    /// a shared key, the key's latest (D49).
    pub async fn voting_masternodes(
        &self,
        wallet: WalletId,
        proposal: String,
    ) -> Result<Vec<VotingMasternode>, GovernanceError> {
        let key = parse_hash(&proposal)?;
        self.runtime()?;
        let keys = self
            .session
            .masternode_keys(wallet, MasternodeKeyRole::Voting, 0, VOTING_KEY_INDEXES)
            .await?;
        let indexes: HashMap<[u8; 20], u32> = keys
            .iter()
            .filter_map(|k| {
                let pk = hex::decode(&k.public_key_hex).ok()?;
                Some((hash160::Hash::hash(&pk).to_byte_array(), k.index))
            })
            .collect();
        let rt = self.runtime()?;
        let list = rt.list();
        if list.entries.is_empty() {
            return Err(GovernanceError::ListUnavailable);
        }
        let store = rt.shared.store();
        let mut holders: HashMap<[u8; 20], u32> = HashMap::new();
        for e in &list.entries {
            *holders.entry(e.voting_key_id).or_default() += 1;
        }
        let mut out: Vec<VotingMasternode> = list
            .entries
            .iter()
            .filter_map(|e| {
                let key_index = *indexes.get(&e.voting_key_id)?;
                let key_holders = holders[&e.voting_key_id];
                let vote = own_vote(&store, &key, &e.pro_tx_hash, &e.voting_key_id, key_holders);
                let key_last = (key_holders > 1)
                    .then(|| latest_under(&store, &key, &e.voting_key_id))
                    .flatten();
                Some(VotingMasternode {
                    pro_tx_hash: display_hex(&e.pro_tx_hash),
                    kind: if e.is_evonode {
                        MasternodeType::Evonode
                    } else {
                        MasternodeType::Regular
                    },
                    weight: ListSnapshot::weight(e),
                    valid: e.is_valid,
                    key_index,
                    current_vote: vote.map(|v| VoteOutcome::from(v.outcome)),
                    vote_time: vote.map(|v| v.time),
                    key_holders,
                    key_last_vote: key_last.map(|v| VoteOutcome::from(v.outcome)),
                    key_last_vote_time: key_last.map(|v| v.time),
                })
            })
            .collect();
        out.sort_by(|a, b| {
            a.key_index
                .cmp(&b.key_index)
                .then(a.pro_tx_hash.cmp(&b.pro_tx_hash))
        });
        Ok(out)
    }

    /// Core's `voteraw`: a funding vote signed elsewhere, checked as a
    /// peer's vote is, stored, and announced to the peers once the run is
    /// synced (MG-02 D24). `governance.no_peers` when none is connected;
    /// the vote is kept and announced on the next connection.
    pub async fn relay_vote(&self, vote: RawVote) -> Result<VoteRelayed, GovernanceError> {
        // Its obsolete and hourly checks need every current vote (D40).
        let rt = self.require_relaying()?;
        let parent = parse_hash(&vote.proposal)?;
        let outpoint = parse_outpoint(&vote.collateral)?;
        // Core's `voteraw` order: the object, the base64, then the vote's
        // checks (`GovernanceStore::add_own_vote`). Votes on triggers are
        // refused where Core would check them (D31).
        let known = |store: &GovernanceStore| match store.object(&parent).map(|o| &o.kind) {
            None => Err(rejected("unknown_object")),
            Some(ObjectKind::Trigger(_)) => Err(rejected("not_a_proposal")),
            Some(ObjectKind::Proposal(_)) => Ok(()),
        };
        if rt.shared.store().object(&parent).is_none() {
            return Err(rejected("unknown_object"));
        }
        let signature = base64::engine::general_purpose::STANDARD
            .decode(vote.signature.as_bytes())
            .map_err(|_| EngineError::InvalidArgument("signature is not base64".into()))?;
        let mut v = GovernanceVote::funding(outpoint, parent, vote.outcome.into(), vote.time);
        v.signature = signature;
        let hash = v.hash();
        {
            let mut store = rt.shared.store();
            known(&store)?;
            store
                .add_own_vote(v.clone(), now())
                .map_err(|r| rejected(rejection_reason(r)))?;
        }
        (rt.shared.on_change)();
        let peers = rt.shared.relay_vote(&v);
        if peers == 0 {
            return Err(GovernanceError::NoPeers);
        }
        Ok(VoteRelayed {
            hash: display_hex(&hash),
            peers,
        })
    }

    /// Casts a funding vote with `outcome` on `proposal` for each masternode
    /// (proTxHash, display order) under one `Governance` grant, and relays
    /// each as [`Self::relay_vote`] does (MG-05 §5). The grant becomes a
    /// `voting` lease for the batch, ended when the call returns.
    ///
    /// The call fails as a whole, before the grant is redeemed, while sync
    /// is off or not synced, without a list, for a proposal it doesn't know,
    /// and for no masternodes, more than the list holds, or a malformed or
    /// repeated proTxHash (`invalid_argument`). The grant's refusals are
    /// `platform.*`. Otherwise each masternode gets a [`VoteCast`]: its
    /// vote's hash and peers, or why it was not cast (D6). The voting key is
    /// the wallet's keychain key at indexes 0–19, else the masternode's
    /// attached `voting` key, whichever has the list's key id. Each vote is
    /// stamped with the time it is signed.
    pub async fn vote(
        &self,
        wallet: WalletId,
        proposal: String,
        outcome: VoteOutcome,
        masternodes: Vec<String>,
        grant: String,
    ) -> Result<Vec<VoteCast>, GovernanceError> {
        self.vote_stamped(wallet, proposal, outcome, masternodes, grant, now)
            .await
    }

    /// [`Self::vote`] with `stamp` the clock each vote is stamped with
    /// (tests pin it).
    async fn vote_stamped(
        &self,
        wallet: WalletId,
        proposal: String,
        outcome: VoteOutcome,
        masternodes: Vec<String>,
        grant: String,
        stamp: impl Fn() -> i64 + Send + 'static,
    ) -> Result<Vec<VoteCast>, GovernanceError> {
        let rt = self.require_relaying()?;
        let parent = parse_hash(&proposal)?;
        let list = rt.list();
        if list.entries.is_empty() {
            return Err(GovernanceError::ListUnavailable);
        }
        if !is_proposal(&rt.shared.store(), &parent) {
            return Err(GovernanceError::ProposalNotFound { hash: proposal });
        }
        let targets = parse_masternodes(&masternodes, list.entries.len())?;
        let targets: Vec<Target> = {
            let store = rt.shared.store();
            targets
                .into_iter()
                .map(|pro_tx_hash| Target {
                    pro_tx_hash,
                    voting_key_id: (list.entries.iter())
                        .find(|e| e.pro_tx_hash == pro_tx_hash)
                        .map(|e| e.voting_key_id),
                    collateral: store.resolved_collateral(&pro_tx_hash),
                })
                .collect()
        };

        let _op = self.session.enter().await?;
        self.session.require_wallet(&wallet)?;
        let lease = (self.session)
            .begin_lease(wallet, FlowKind::Voting, &[grant])
            .await
            .map_err(PlatformError::from)?;
        let signer = lease.vote_signer().map_err(PlatformError::from)?;
        let signed = tokio::task::spawn_blocking(move || {
            let mut keychain = None;
            targets
                .into_iter()
                .map(|t| {
                    let vote = sign_one(&signer, &mut keychain, &t, parent, outcome, &stamp);
                    (t.pro_tx_hash, vote)
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| EngineError::Internal(format!("vote signing panicked: {e}")))?;

        let mut out = Vec::with_capacity(signed.len());
        for (pro_tx_hash, vote) in signed {
            let cast = match vote {
                Ok(vote) => self.hand_off(&lease, &parent, vote).await,
                Err(reason) => Err(reason),
            };
            let (hash, peers, rejected) = match cast {
                Ok((hash, peers)) => (Some(display_hex(&hash)), peers, None),
                Err(reason) => (None, 0, Some(reason.into())),
            };
            out.push(VoteCast {
                pro_tx_hash: display_hex(&pro_tx_hash),
                hash,
                peers,
                rejected,
            });
        }
        lease.end();
        Ok(out)
    }

    /// One signed vote through the dispatch fence (MG-05 §4, D5): admitted
    /// under the lease's scope, then handed off by entering the store,
    /// which announces it. The vote's hash and the peers it was announced
    /// to, or the reason it was refused.
    async fn hand_off(
        &self,
        lease: &Lease,
        parent: &[u8; 32],
        vote: GovernanceVote,
    ) -> Result<([u8; 32], u32), &'static str> {
        let hash = vote.hash();
        // Before the admission: a permit must be finished, not dropped
        // (dropped, it would read `MaybeSent`).
        let rt = self.runtime().map_err(|_| "cancelled")?;
        let wallet = lease.wallet();
        let table = Arc::clone(&lease.table);
        let verdict = lease
            .scope(async move {
                table
                    .admit(AdmitRequest {
                        wallet,
                        artifact: ArtifactId(hash),
                        kind: ArtifactKind::Vote,
                        tracked_row: false,
                        scope: DispatchScope::current(),
                    })
                    .await
            })
            .await;
        let store_it = || -> Result<u32, &'static str> {
            {
                let mut store = rt.shared.store();
                if !is_proposal(&store, parent) {
                    return Err("unknown_object");
                }
                store
                    .add_own_vote(vote.clone(), now())
                    .map_err(rejection_reason)?;
            }
            (rt.shared.on_change)();
            Ok(rt.shared.relay_vote(&vote))
        };
        let outcome = |r: &Result<u32, &str>| match r {
            Ok(_) => Outcome::Sent,
            Err(_) => Outcome::NotSent,
        };
        let peers = match verdict {
            Verdict::First(permit) => {
                let r = store_it();
                permit.finish(outcome(&r));
                r
            }
            Verdict::Resend(guard) | Verdict::FirstUnleased(guard) => {
                let r = store_it();
                guard.finish(outcome(&r));
                r
            }
            // The lease was revoked: a lock ended the batch.
            Verdict::Refused { .. } => Err("cancelled"),
            // The same vote (masternode, outcome and second) is in flight,
            // or its outcome is still being resolved (D13).
            Verdict::Deferred => Err("in_flight"),
        }?;
        Ok((hash, peers))
    }
}

/// One masternode of a `vote` batch: its list entry's voting key id and
/// its resolved collateral, when known.
struct Target {
    pro_tx_hash: [u8; 32],
    voting_key_id: Option<[u8; 20]>,
    collateral: Option<OutPoint>,
}

/// `vote`'s proTxHashes: at least one, at most `listed`, each 64 hex
/// characters and none twice (`invalid_argument`).
pub(super) fn parse_masternodes(
    texts: &[String],
    listed: usize,
) -> Result<Vec<[u8; 32]>, GovernanceError> {
    let bad = |m: String| GovernanceError::from(EngineError::InvalidArgument(m));
    if texts.is_empty() {
        return Err(bad("no masternodes to vote with".into()));
    }
    if texts.len() > listed {
        return Err(bad(format!(
            "{} masternodes, the list holds {listed}",
            texts.len()
        )));
    }
    let mut seen = HashSet::new();
    texts
        .iter()
        .map(|t| {
            let h = parse_display_hex(t)
                .ok_or_else(|| bad(format!("not a 64-hex proTxHash: {t:?}")))?;
            if !seen.insert(h) {
                return Err(bad(format!("proTxHash given twice: {t}")));
            }
            Ok(h)
        })
        .collect()
}

/// The wallet's keychain voting keys by key id, indexes 0–19.
pub(super) type Keychain = Result<HashMap<[u8; 20], u32>, SignerError>;

/// Signs `t`'s vote, stamped with `stamp`'s time, or says why not (MG-05
/// §5). `keychain` is filled on first use. On the blocking pool: each key
/// use is a vault operation.
fn sign_one(
    signer: &VoteSigner,
    keychain: &mut Option<Keychain>,
    t: &Target,
    parent: [u8; 32],
    outcome: VoteOutcome,
    stamp: &dyn Fn() -> i64,
) -> Result<GovernanceVote, &'static str> {
    let id = t.voting_key_id.ok_or("unknown_masternode")?;
    let collateral = t.collateral.ok_or("collateral_unresolved")?;
    let keys = keychain.get_or_insert_with(|| {
        (0..VOTING_KEY_INDEXES)
            .map(|index| Ok((signer.key_id(&VotingKey::Keychain { index })?, index)))
            .collect()
    });
    let key = choose_key(keys, id, t.pro_tx_hash, |k| signer.key_id(k))?;
    let time = stamp();
    let fields = FundingVote {
        collateral,
        proposal: parent,
        outcome: funding_outcome(outcome),
        time,
    };
    let signature = signer.sign_vote(&key, &fields).map_err(refused)?;
    let mut vote = GovernanceVote::funding(collateral, parent, outcome.into(), time);
    vote.signature = signature.to_vec();
    Ok(vote)
}

/// The key with the list's key id `id`: the keychain's, else the
/// masternode's attached key (`attached_id` reads its id), or why there
/// is none. A keychain the vault cannot read (other than locked, or a
/// wallet without a seed) leaves the attached key to try (D20); without
/// it the vote is `signer_unavailable`, not `no_voting_key`, since the
/// keychain may hold the key (D23).
pub(super) fn choose_key(
    keychain: &Keychain,
    id: [u8; 20],
    pro_tx_hash: [u8; 32],
    attached_id: impl FnOnce(&VotingKey) -> Result<[u8; 20], SignerError>,
) -> Result<VotingKey, &'static str> {
    let unread = match keychain {
        Ok(keys) => match keys.get(&id) {
            Some(&index) => return Ok(VotingKey::Keychain { index }),
            None => false,
        },
        Err(SignerError::Locked) => return Err("cancelled"),
        Err(SignerError::NoSecret) => false,
        Err(_) => true,
    };
    let attached = VotingKey::Attached { pro_tx_hash };
    match attached_id(&attached) {
        Ok(a) if a == id => Ok(attached),
        Ok(_) | Err(SignerError::NoSecret) if unread => Err("signer_unavailable"),
        Ok(_) | Err(SignerError::NoSecret) => Err("no_voting_key"),
        Err(e) => Err(refused(e)),
    }
}

/// The reason of a vault refusal while signing.
pub(super) fn refused(e: SignerError) -> &'static str {
    match e {
        SignerError::Locked => "cancelled",
        _ => "signer_unavailable",
    }
}

/// The vault's name of a funding vote's outcome.
fn funding_outcome(o: VoteOutcome) -> FundingOutcome {
    match o {
        VoteOutcome::Yes => FundingOutcome::Yes,
        VoteOutcome::No => FundingOutcome::No,
        VoteOutcome::Abstain => FundingOutcome::Abstain,
    }
}

/// The current vote on `proposal` of the masternode `pro_tx_hash`, whose
/// voting key `key` `holders` entries of the tip list hold: one under the
/// key whose outpoint is resolved to it, or, when no other entry holds the
/// key, one whose outpoint is not resolved to another masternode (D49).
fn own_vote<'a>(
    store: &'a GovernanceStore,
    proposal: &[u8; 32],
    pro_tx_hash: &[u8; 32],
    key: &[u8; 20],
    holders: u32,
) -> Option<&'a CurrentVote> {
    store
        .votes_on(proposal)
        .filter(|v| &v.key_id == key)
        .filter(|v| match store.resolved(&v.outpoint) {
            Some(r) => &r.pro_tx_hash == pro_tx_hash && r.voting_key_id == v.key_id,
            None => holders == 1,
        })
        .max_by_key(|v| v.time)
}

/// The latest current vote on `proposal` under `key`, for any outpoint.
fn latest_under<'a>(
    store: &'a GovernanceStore,
    proposal: &[u8; 32],
    key: &[u8; 20],
) -> Option<&'a CurrentVote> {
    store
        .votes_on(proposal)
        .filter(|v| &v.key_id == key)
        .max_by_key(|v| v.time)
}

fn rejected(reason: &str) -> GovernanceError {
    GovernanceError::VoteRejected {
        reason: reason.into(),
    }
}

/// The `vote_rejected` reason of a store rejection (mg-engine §4). The
/// parent is a known proposal by then, so an ignored vote is one on an
/// expired proposal.
fn rejection_reason(r: VoteRejection) -> &'static str {
    match r {
        VoteRejection::Duplicate => "duplicate",
        VoteRejection::Ignored => "expired",
        // `Parked` is an operator vote's (D89): the facade relays only
        // funding votes.
        VoteRejection::Unverifiable | VoteRejection::Orphan | VoteRejection::Parked => {
            "unknown_object"
        }
        VoteRejection::Malformed => "malformed",
        VoteRejection::BadSignature => "bad_signature",
        VoteRejection::UnknownVotingKey => "unknown_voting_key",
        VoteRejection::WrongVotingKey => "wrong_voting_key",
        VoteRejection::TooNew => "too_far_ahead",
        VoteRejection::Obsolete => "obsolete",
        VoteRejection::SameTimestamp => "same_timestamp",
        VoteRejection::TooOften => "too_often",
        VoteRejection::Banned => "masternode_not_found",
        VoteRejection::KeyLimit => "key_limit",
    }
}

/// `"<txid>-<n>"` (Core `ToStringShort`).
pub(super) fn parse_outpoint(text: &str) -> Result<OutPoint, GovernanceError> {
    let bad = || EngineError::InvalidArgument(format!("not an outpoint \"<txid>-<n>\": {text:?}"));
    let (txid, n) = text.rsplit_once('-').ok_or_else(bad)?;
    let txid = Txid::from_str(txid).map_err(|_| bad())?;
    let vout = n.parse::<u32>().map_err(|_| bad())?;
    Ok(OutPoint::new(txid, vout))
}

#[cfg(test)]
#[path = "voting_tests.rs"]
mod flow_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use dashcore::Network;
    use dashcore::secp256k1::{Message, PublicKey, SecretKey};
    use dw_governance::object::GovernanceObject;
    use dw_governance::store::{ListEntry, ResolvedMasternode};
    use dw_governance::vote::{VoteOutcome as Outcome, to_recoverable_compact};

    const NOW: i64 = 1_000_000;

    fn key_id(n: u8) -> [u8; 20] {
        let sk = SecretKey::from_secret_bytes([n; 32]).unwrap();
        hash160::Hash::hash(&PublicKey::from_secret_key(&sk).serialize()).to_byte_array()
    }

    /// A funding vote by voting key `key` for outpoint `(op, 0)`.
    fn signed(parent: [u8; 32], key: u8, op: u8, outcome: Outcome, time: i64) -> GovernanceVote {
        let sk = SecretKey::from_secret_bytes([key; 32]).unwrap();
        let pk = PublicKey::from_secret_key(&sk);
        let outpoint = OutPoint::new(Txid::from_byte_array([op; 32]), 0);
        let mut v = GovernanceVote::funding(outpoint, parent, outcome, time);
        let d = v.signing_digest(Network::Regtest);
        let s = dashcore::secp256k1::ecdsa::sign(Message::from_digest(d), &sk);
        v.signature = to_recoverable_compact(&s.serialize_compact(), &pk, &d)
            .unwrap()
            .to_vec();
        v
    }

    /// A store holding one proposal, with a list in which `shared` entries
    /// hold voting key 1 and one entry holds key 2.
    fn store_with(shared: usize) -> (GovernanceStore, [u8; 32]) {
        let mut store = GovernanceStore::new(Network::Regtest);
        store.set_confirmations(1);
        let data = format!(
            r#"{{"end_epoch":{},"name":"p","payment_address":"yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf","payment_amount":5,"start_epoch":1,"type":1,"url":"http://x.y"}}"#,
            NOW + 10_000
        );
        let p = GovernanceObject::new_proposal(1, NOW, data.into_bytes());
        let hash = p.hash();
        store.add_object(p, None, NOW).unwrap();
        let mut list = vec![
            ListEntry {
                voting_key_id: key_id(1),
                weight: Some(1),
                valid: true,
            };
            shared
        ];
        list.push(ListEntry {
            voting_key_id: key_id(2),
            weight: Some(1),
            valid: true,
        });
        store.set_masternode_list(&list);
        (store, hash)
    }

    /// Sol r1 Major 5: two masternodes share voting key 1. A vote under the
    /// key is not reported as either one's own vote, whether both of them
    /// voted (Yes, then No later) or only one did; a key only one
    /// masternode holds votes for that masternode.
    #[test]
    fn a_shared_keys_vote_is_not_reported_as_each_holders_own() {
        let (a, b) = ([0xa; 32], [0xb; 32]);
        let (mut store, h) = store_with(2);
        store
            .add_vote(signed(h, 1, 1, Outcome::Yes, NOW - 200), None, NOW)
            .unwrap();
        store
            .add_vote(signed(h, 1, 2, Outcome::No, NOW - 100), None, NOW)
            .unwrap();
        for mn in [&a, &b] {
            assert_eq!(own_vote(&store, &h, mn, &key_id(1), 2), None);
        }
        let last = latest_under(&store, &h, &key_id(1)).unwrap();
        assert_eq!((last.outcome, last.time), (Outcome::No, NOW - 100));
        // Resolved, each outpoint's vote is its masternode's own.
        let resolved = |pro_tx_hash, op: u8| {
            (
                OutPoint::new(Txid::from_byte_array([op; 32]), 0),
                ResolvedMasternode {
                    pro_tx_hash,
                    voting_key_id: key_id(1),
                    operator_key: [0; 48],
                    weight: 1,
                },
            )
        };
        store.set_resolved(HashMap::from([resolved(a, 1), resolved(b, 2)]));
        let own = |mn| own_vote(&store, &h, mn, &key_id(1), 2).map(|v| v.outcome);
        assert_eq!((own(&a), own(&b)), (Some(Outcome::Yes), Some(Outcome::No)));

        let (mut store, h) = store_with(2);
        store
            .add_vote(signed(h, 1, 1, Outcome::Yes, NOW - 200), None, NOW)
            .unwrap();
        for mn in [&a, &b] {
            assert_eq!(own_vote(&store, &h, mn, &key_id(1), 2), None);
        }
        let last = latest_under(&store, &h, &key_id(1)).unwrap();
        assert_eq!(last.outcome, Outcome::Yes);

        store
            .add_vote(signed(h, 2, 3, Outcome::No, NOW - 100), None, NOW)
            .unwrap();
        let own = own_vote(&store, &h, &[0xc; 32], &key_id(2), 1).unwrap();
        assert_eq!((own.outcome, own.time), (Outcome::No, NOW - 100));
    }

    #[test]
    fn outpoints_parse_as_core_prints_them() {
        let txid = "aa".repeat(32);
        let o = parse_outpoint(&format!("{txid}-7")).unwrap();
        assert_eq!(o.vout, 7);
        assert_eq!(
            dw_governance::object::outpoint_short(&o),
            format!("{txid}-7")
        );
        for bad in [
            "",
            "x-1",
            &txid.to_string(),
            &format!("{txid}--1"),
            &format!("{txid}-a"),
        ] {
            assert!(parse_outpoint(bad).is_err(), "{bad}");
        }
    }

    /// MG-05 §7: the vault's vote digest (which this crate's governance
    /// code cannot share, D1) equals `GovernanceVote::signing_digest` on
    /// every network, for every outcome.
    #[test]
    fn the_vaults_vote_digest_is_the_governance_digest() {
        let mut txid = [0u8; 32];
        let mut proposal = [0u8; 32];
        for i in 0..32u8 {
            txid[i as usize] = i.wrapping_mul(37) ^ 0x5c;
            proposal[i as usize] = i.wrapping_mul(11) ^ 0xa3;
        }
        let collateral = OutPoint::new(Txid::from_byte_array(txid), 0x0102_0304);
        for (outcome, ours) in [
            (Outcome::Yes, FundingOutcome::Yes),
            (Outcome::No, FundingOutcome::No),
            (Outcome::Abstain, FundingOutcome::Abstain),
        ] {
            for time in [0, NOW, -1, i64::MAX] {
                let theirs = GovernanceVote::funding(collateral, proposal, outcome, time);
                let vote = FundingVote {
                    collateral,
                    proposal,
                    outcome: ours,
                    time,
                };
                for net in [
                    Network::Mainnet,
                    Network::Testnet,
                    Network::Devnet,
                    Network::Regtest,
                ] {
                    assert_eq!(
                        vote.digest(net),
                        theirs.signing_digest(net),
                        "{net:?} {ours:?} {time}"
                    );
                }
            }
        }
    }

    /// Which key a vote uses, or why none (D20, D23; review r1 F3 E9/E10,
    /// F4).
    #[test]
    fn a_vote_uses_the_keychain_key_else_the_attached_key_with_the_lists_id() {
        let (id, other, mn) = (key_id(1), key_id(2), [5; 32]);
        let attached = VotingKey::Attached { pro_tx_hash: mn };
        let has = |k: [u8; 20]| move |_: &VotingKey| Ok(k);
        let none = |_: &VotingKey| Err(SignerError::NoSecret);
        let fails = |e: fn() -> SignerError| move |_: &VotingKey| Err(e());
        let derivation = || SignerError::Derivation("unreadable".into());

        let keys: Keychain = Ok(HashMap::from([(id, 3)]));
        assert_eq!(
            choose_key(&keys, id, mn, has(id)),
            Ok(VotingKey::Keychain { index: 3 })
        );
        // Not in the keychain: the attached key when it has the list's id.
        let empty: Keychain = Ok(HashMap::new());
        assert_eq!(choose_key(&empty, id, mn, has(id)), Ok(attached));
        assert_eq!(choose_key(&empty, id, mn, has(other)), Err("no_voting_key"));
        assert_eq!(choose_key(&empty, id, mn, none), Err("no_voting_key"));
        assert_eq!(
            choose_key(&empty, id, mn, fails(derivation)),
            Err("signer_unavailable")
        );
        assert_eq!(
            choose_key(&empty, id, mn, fails(|| SignerError::Locked)),
            Err("cancelled")
        );
        // An unreadable keychain still lets the attached key vote (D20);
        // without it the key may be the keychain's (D23).
        let unread: Keychain = Err(derivation());
        assert_eq!(choose_key(&unread, id, mn, has(id)), Ok(attached));
        assert_eq!(choose_key(&unread, id, mn, none), Err("signer_unavailable"));
        assert_eq!(
            choose_key(&unread, id, mn, has(other)),
            Err("signer_unavailable")
        );
        // A wallet without a seed has no keychain keys; a locked vault
        // ends the batch.
        let seedless: Keychain = Err(SignerError::NoSecret);
        assert_eq!(choose_key(&seedless, id, mn, none), Err("no_voting_key"));
        assert_eq!(choose_key(&seedless, id, mn, has(id)), Ok(attached));
        let locked: Keychain = Err(SignerError::Locked);
        assert_eq!(choose_key(&locked, id, mn, has(id)), Err("cancelled"));
    }
}
