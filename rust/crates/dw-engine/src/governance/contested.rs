//! Votes on contested DPNS names (mg-engine §2.6, MG-07): the open contests
//! with the wallet's masternodes that can vote on them, and votes cast with
//! their voting keys under a `Governance` grant. They need the SPV list and
//! Platform, not governance sync (MG-07 D2).

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use dash_sdk::RequestSettings;
use dash_sdk::drive::query::contested_resource_votes_given_by_identity_query::ContestedResourceVotesGivenByIdentityQuery;
use dash_sdk::platform::FetchMany;
use dash_sdk::platform::transition::broadcast::BroadcastStateTransition;
use dash_sdk::platform::transition::masternode_vote_keys::{
    VotingKeyProblem, diagnose_voting_key_failure,
};
use dash_sdk::platform::transition::put_settings::PutSettings;
use dash_sdk::query_types::ResourceVotesByIdentity;
use dpp::consensus::ConsensusError;
use dpp::consensus::signature::SignatureError;
use dpp::consensus::state::state_error::StateError;
use dpp::dashcore::ProTxHash;
use dpp::dashcore::hashes::Hash as _;
use dpp::identifier::MasternodeIdentifiers;
use dpp::platform_value::Value;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::state_transition::StateTransition;
use dpp::state_transition::proof_result::StateTransitionProofResult;
use dpp::system_data_contracts::SystemDataContract;
use dpp::util::strings::convert_to_homograph_safe_chars;
use dpp::voting::vote_choices::resource_vote_choice::ResourceVoteChoice;
use dpp::voting::vote_polls::VotePoll;
use dpp::voting::votes::resource_vote::ResourceVote;
use dpp::voting::votes::resource_vote::accessors::v0::ResourceVoteGettersV0;
use dw_appdb::{AppDb, GLOBAL_SCOPE};
use dw_governance::object::display_hex;
use dw_vault::{NameVote, SignerError, VoteSigner, VotingKey, is_contested_label};
use futures_util::StreamExt as _;
use platform_wallet::masternode::MasternodeListSummary;

use super::records::{
    ContestedName, ContestedNames, MasternodeType, NameChoice, NameContender, NameVoteCast,
    NameVoter, NameVoterState, NameVotingKey,
};
use super::sync::ListSnapshot;
use super::voting::{Keychain, VOTING_KEY_INDEXES, choose_key, parse_masternodes, refused};
use super::{Governance, GovernanceError};
use crate::events::unix_now;
use crate::masternode_keys::{MasternodeKeyInfo, MasternodeKeyRole};
use crate::masternodes::AttachedKeyRole;
use crate::masternodes::follow::{Notes, load_notes, read_list};
use crate::platform::contest_watch::{ContestNet, ENDS_PAGE, POLL_PAGE, Poll, SdkContests};
use crate::platform::lease::fence::HandOff;
use crate::platform::lease::{
    AdmitRequest, ArtifactId, ArtifactKind, DispatchScope, Lease, Outcome, Verdict,
};
use crate::platform::{FlowKind, PlatformError};
use crate::{EngineError, NetworkSession, WalletId};

/// `votes_used:<proTxHash>:<label>` in app.sqlite's global scope (D6).
const VOTES_USED: &str = "governance.name_votes:";
/// Polls read at once for `contested_names`.
const POLLS_AT_ONCE: usize = 8;
/// How long a vote's wait for Platform's proof may take before it is
/// `unconfirmed` (D7).
const WAIT_FOR_PROOF: Duration = Duration::from_secs(60);

/// What a name vote asks of Platform beyond DP1-04's contest reads, behind
/// a seam: [`SdkNames`] asks the SDK; the tests script it.
pub(crate) trait NameVoteNet: ContestNet {
    /// Platform's `votes_allowed_per_masternode` (5 at the pin).
    fn votes_allowed(&self) -> u32;
    /// The proved present choices of masternode `pro_tx_hash` (Platform
    /// order) on DPNS polls, by normalized label (D4).
    fn votes_of(
        &self,
        pro_tx_hash: [u8; 32],
    ) -> impl Future<Output = Result<BTreeMap<String, NameChoice>, PlatformError>> + Send;
    /// The voter identity's next nonce.
    fn nonce(&self, voter: [u8; 32]) -> impl Future<Output = Result<u64, PlatformError>> + Send;
    /// Broadcasts a signed transition: one attempt, no retry on another
    /// node (D12). A First runs under its permit's deadline (D16).
    fn broadcast(&self, bytes: Vec<u8>) -> impl Future<Output = Broadcast> + Send;
    /// Waits for the proof of a transition [`Broadcast::Accepted`] took
    /// (D12), after the permit ended.
    fn wait(&self, bytes: Vec<u8>) -> impl Future<Output = Cast> + Send;
    /// Why Platform refused the voting key: one identity fetch, only
    /// after such a refusal (D10). `None` when the fetch says nothing.
    fn diagnose(
        &self,
        pro_tx_hash: [u8; 32],
        voting_address: [u8; 20],
    ) -> impl Future<Output = Option<&'static str>> + Send;
}

/// What the one broadcast attempt of a transition came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Broadcast {
    /// DAPI took it, or already held the same bytes: its proof is waited
    /// for.
    Accepted,
    /// Platform's CheckTx refused it.
    Refused(Refusal),
    /// The request never left (a local error).
    NotSent,
    /// It may have reached a node; no answer came back.
    Unknown,
}

/// What became of one broadcast transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cast {
    /// Platform's proof of the executed vote came back.
    Counted,
    /// Platform refused it: at the broadcast (`executed` false), or in the
    /// block that executed it.
    Refused { refusal: Refusal, executed: bool },
    /// The request never left (a local error).
    NotSent,
    /// It may have left, and no answer or no proof came back (D7).
    Unknown,
}

/// Platform's refusal of a vote, from its typed consensus error (D10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// A per-masternode reason of mg-engine §4.
    Reason(&'static str),
    /// Over the limit, with the votes Platform counted.
    VoteLimit { used: u16 },
    /// A signature error about key 0, to be told apart by `diagnose`.
    VotingKey,
}

/// The refusal a consensus error means for a vote (MG-07 §3).
pub(crate) fn refusal(e: &ConsensusError) -> Refusal {
    use Refusal::Reason;
    match e {
        ConsensusError::StateError(s) => match s {
            StateError::MasternodeVoteAlreadyPresentError(_) => Reason("already_voted"),
            StateError::MasternodeVotedTooManyTimesError(e) => Refusal::VoteLimit {
                used: e.times_already_voted(),
            },
            StateError::VotePollNotAvailableForVotingError(_) => Reason("contest_closed"),
            StateError::VotePollNotFoundError(_) => Reason("contest_not_found"),
            StateError::MasternodeNotFoundError(_) => Reason("masternode_not_found"),
            StateError::MasternodeIncorrectVotingAddressError(_)
            | StateError::MasternodeIncorrectVoterIdentityIdError(_) => Reason("wrong_voting_key"),
            StateError::VoteChoiceNotAllowedForVotePollError(_) => Reason("choice_not_allowed"),
            _ => Reason("refused"),
        },
        // rs-sdk's `is_voting_key_failure`.
        ConsensusError::SignatureError(
            SignatureError::IdentityNotFoundError(_)
            | SignatureError::MissingPublicKeyError(_)
            | SignatureError::PublicKeyIsDisabledError(_),
        ) => Refusal::VotingKey,
        _ => Reason("refused"),
    }
}

/// The consensus error an SDK error carries: the broadcast's or the wait
/// stream's refusal, or a local one, under any retry envelopes.
fn consensus(e: &dash_sdk::Error) -> Option<&ConsensusError> {
    match e {
        dash_sdk::Error::StateTransitionBroadcastError(e) => e.cause.as_ref(),
        dash_sdk::Error::Protocol(dpp::ProtocolError::ConsensusError(e)) => Some(e),
        dash_sdk::Error::NoAvailableAddressesToRetry(last) => consensus(last),
        _ => None,
    }
}

/// The broadcast's answer as [`Broadcast`]. `AlreadyExists` (the same
/// bytes in the mempool) goes on to the wait, as rs-sdk's identity put
/// does. A `Protocol` error with no consensus error is DAPI's refusal in
/// a form this client cannot decode (rs-sdk `Error::from(DapiClientError)`):
/// refused, not unsent.
fn broadcast_outcome(answer: Result<(), dash_sdk::Error>) -> Broadcast {
    let Err(e) = answer else {
        return Broadcast::Accepted;
    };
    match (&e, consensus(&e)) {
        (dash_sdk::Error::AlreadyExists(_), _) => Broadcast::Accepted,
        (_, Some(c)) => Broadcast::Refused(refusal(c)),
        (dash_sdk::Error::Protocol(_), None) => Broadcast::Refused(Refusal::Reason("refused")),
        (_, None) => {
            tracing::debug!(error = %e, "name vote broadcast");
            Broadcast::Unknown
        }
    }
}

/// The proof wait's answer as [`Cast`]: a refusal in the block that
/// executed the transition, typed or not, or no proof.
fn proof_outcome<T>(answer: Result<T, dash_sdk::Error>) -> Cast {
    let Err(e) = answer else {
        return Cast::Counted;
    };
    let refusal = match (&e, consensus(&e)) {
        (_, Some(c)) => refusal(c),
        (dash_sdk::Error::StateTransitionBroadcastError(_), None) => Refusal::Reason("refused"),
        (_, None) => {
            tracing::debug!(error = %e, "name vote proof");
            return Cast::Unknown;
        }
    };
    Cast::Refused {
        refusal,
        executed: true,
    }
}

/// The SDK behind [`NameVoteNet`].
pub(crate) struct SdkNames(pub(crate) Arc<dash_sdk::Sdk>);

impl SdkNames {
    /// The vault's transition, decoded as Drive decodes it.
    fn decode(&self, bytes: &[u8]) -> Option<StateTransition> {
        StateTransition::deserialize_from_bytes_untrusted_exact_in_version(bytes, self.0.version())
            .ok()
    }
}

impl ContestNet for SdkNames {
    async fn page(&self, normalized: &str, limit: u16) -> Result<Poll, PlatformError> {
        SdkContests(Arc::clone(&self.0))
            .page(normalized, limit)
            .await
    }

    async fn end_page(
        &self,
        from: u64,
        limit: u16,
    ) -> Result<BTreeMap<String, u64>, PlatformError> {
        SdkContests(Arc::clone(&self.0)).end_page(from, limit).await
    }
}

impl NameVoteNet for SdkNames {
    fn votes_allowed(&self) -> u32 {
        let voting = &self.0.version().dpp.validation.voting;
        u32::from(voting.votes_allowed_per_masternode)
    }

    async fn votes_of(
        &self,
        pro_tx_hash: [u8; 32],
    ) -> Result<BTreeMap<String, NameChoice>, PlatformError> {
        // Drive keys the votes a masternode gave by its proTxHash.
        let query = ContestedResourceVotesGivenByIdentityQuery {
            identity_id: Identifier::from(pro_tx_hash),
            offset: None,
            limit: Some(POLL_PAGE),
            start_at: None,
            order_ascending: true,
        };
        let votes: ResourceVotesByIdentity = ResourceVote::fetch_many(&self.0, query).await?;
        Ok(votes
            .into_values()
            .flatten()
            .filter_map(dpns_choice)
            .collect())
    }

    async fn nonce(&self, voter: [u8; 32]) -> Result<u64, PlatformError> {
        Ok(self
            .0
            .get_identity_nonce(Identifier::from(voter), true, None)
            .await?)
    }

    async fn broadcast(&self, bytes: Vec<u8>) -> Broadcast {
        let Some(st) = self.decode(&bytes) else {
            return Broadcast::NotSent;
        };
        // One node only: a refusal by a second node would not prove that
        // the first did not take it (D12).
        let settings = PutSettings {
            request_settings: RequestSettings {
                retries: Some(0),
                ..Default::default()
            },
            ..Default::default()
        };
        broadcast_outcome(st.broadcast(&self.0, Some(settings)).await)
    }

    async fn wait(&self, bytes: Vec<u8>) -> Cast {
        let Some(st) = self.decode(&bytes) else {
            return Cast::Unknown;
        };
        let settings = PutSettings {
            wait_timeout: Some(WAIT_FOR_PROOF),
            ..Default::default()
        };
        proof_outcome(
            st.wait_for_response::<StateTransitionProofResult>(&self.0, Some(settings))
                .await,
        )
    }

    async fn diagnose(
        &self,
        pro_tx_hash: [u8; 32],
        voting_address: [u8; 20],
    ) -> Option<&'static str> {
        let hash = ProTxHash::from_byte_array(pro_tx_hash);
        diagnose_voting_key_failure(&self.0, hash, &voting_address)
            .await
            .map(|p| match p {
                VotingKeyProblem::NoVoterIdentity { .. } => "no_voter_identity",
                VotingKeyProblem::NoUsableVotingKey { .. } => "voting_key_disabled",
            })
    }
}

/// A DPNS `domain` vote as `(label, choice)`; other polls are not names.
fn dpns_choice(vote: ResourceVote) -> Option<(String, NameChoice)> {
    let VotePoll::ContestedDocumentResourceVotePoll(poll) = vote.vote_poll();
    if poll.contract_id != SystemDataContract::DPNS.id() || poll.document_type_name != "domain" {
        return None;
    }
    let Some(Value::Text(label)) = poll.index_values.get(1) else {
        return None;
    };
    let choice = match vote.resource_vote_choice() {
        ResourceVoteChoice::TowardsIdentity(id) => NameChoice::TowardsIdentity {
            identity: id.to_string(Encoding::Base58),
        },
        ResourceVoteChoice::Abstain => NameChoice::Abstain,
        ResourceVoteChoice::Lock => NameChoice::Lock,
    };
    Some((label.clone(), choice))
}

/// The voter identity of a masternode (display order) and its voting key
/// id (Drive `create_voter_identity_v0`).
fn voter_identity(pro_tx_hash: &[u8; 32], key_id: &[u8; 20]) -> [u8; 32] {
    Identifier::create_voter_identifier(pro_tx_hash, key_id).to_buffer()
}

/// The display (Platform) order of a list entry's wire-order proTxHash.
fn platform_order(wire: &[u8; 32]) -> [u8; 32] {
    let mut h = *wire;
    h.reverse();
    h
}

/// The key of `pro_tx_hash`'s (wire order) count on `label`.
fn votes_used_key(pro_tx_hash: &[u8; 32], label: &str) -> String {
    format!("{VOTES_USED}{}:{label}", display_hex(pro_tx_hash))
}

/// The votes this app saw Platform take from `pro_tx_hash` (wire order)
/// on `label` (D6).
fn votes_used(db: &AppDb, pro_tx_hash: &[u8; 32], label: &str) -> Result<u32, EngineError> {
    let v = db.setting(GLOBAL_SCOPE, &votes_used_key(pro_tx_hash, label))?;
    Ok(v.and_then(|v| v.parse().ok()).unwrap_or(0))
}

fn set_votes_used(
    db: &AppDb,
    pro_tx_hash: &[u8; 32],
    label: &str,
    used: u32,
) -> Result<(), EngineError> {
    let used = used.to_string();
    db.set_setting(
        GLOBAL_SCOPE,
        &votes_used_key(pro_tx_hash, label),
        Some(&used),
    )?;
    Ok(())
}

/// A user's label as DPNS normalizes it, if it can be contested.
fn contested_label(label: &str) -> Result<String, GovernanceError> {
    let normalized = convert_to_homograph_safe_chars(label);
    if !is_contested_label(&normalized) {
        return Err(EngineError::InvalidArgument(format!(
            "{label:?} is not a contested DPNS label (3–19 of a-z, 0, 1 and -)"
        ))
        .into());
    }
    Ok(normalized)
}

/// The vault's choice of a record's, checking the identity id.
fn vault_choice(choice: &NameChoice) -> Result<dw_vault::NameChoice, GovernanceError> {
    Ok(match choice {
        NameChoice::TowardsIdentity { identity } => {
            let id = Identifier::from_string(identity, Encoding::Base58).map_err(|_| {
                EngineError::InvalidArgument(format!("not a base58 identity id: {identity:?}"))
            })?;
            dw_vault::NameChoice::TowardsIdentity(id.to_buffer())
        }
        NameChoice::Abstain => dw_vault::NameChoice::Abstain,
        NameChoice::Lock => dw_vault::NameChoice::Lock,
    })
}

/// The listed masternodes the wallet votes for, in list order, with the key
/// each votes with: a keychain voting key, else the attached one the list's
/// key id matches (D3).
fn name_voters<'a>(
    list: &'a [MasternodeListSummary],
    keys: &[MasternodeKeyInfo],
    notes: &Notes,
) -> Vec<(&'a MasternodeListSummary, NameVotingKey)> {
    let keychain: HashMap<[u8; 20], u32> = keys
        .iter()
        .filter_map(|k| {
            let pk = hex::decode(&k.public_key_hex).ok()?;
            Some((
                dashcore::hashes::hash160::Hash::hash(&pk).to_byte_array(),
                k.index,
            ))
        })
        .collect();
    list.iter()
        .filter_map(|e| {
            if let Some(&index) = keychain.get(&e.voting_key_id) {
                return Some((e, NameVotingKey::Keychain { index }));
            }
            let shown = display_hex(&e.pro_tx_hash);
            let attached = notes.key_ids.get(&(shown, AttachedKeyRole::Voting));
            (attached == Some(&e.voting_key_id)).then_some((e, NameVotingKey::Attached))
        })
        .collect()
}

/// The DPNS contests still open, soonest ending first, with their first
/// page of contenders.
async fn open_contests(net: &impl NameVoteNet) -> Result<Vec<ContestedName>, GovernanceError> {
    let now = unix_now();
    let ends = net.end_page(now, ENDS_PAGE).await?;
    let mut open: Vec<(String, u64)> = ends.into_iter().filter(|(_, end)| *end > now).collect();
    open.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    let polls: Vec<Result<Poll, PlatformError>> = futures_util::stream::iter(&open)
        .map(|(label, _)| net.page(label, POLL_PAGE))
        .buffered(POLLS_AT_ONCE)
        .collect()
        .await;
    let mut contests = Vec::with_capacity(open.len());
    for ((label, ends_at), poll) in open.into_iter().zip(polls) {
        let poll = poll?;
        // Resolved after the end-date page was read.
        if poll.winner.is_some() {
            continue;
        }
        contests.push(ContestedName {
            contenders_complete: poll.contenders.len() < usize::from(POLL_PAGE),
            contenders: (poll.contenders.iter())
                .map(|(id, votes)| NameContender {
                    identity: id.to_string(Encoding::Base58),
                    votes: *votes,
                })
                .collect(),
            label,
            ends_at,
            lock_votes: poll.lock_votes,
            abstain_votes: poll.abstain_votes,
        });
    }
    Ok(contests)
}

/// `e`'s standing in the listed contests: its present choices, proved, and
/// the votes this app saw it use.
async fn name_voter(
    net: &impl NameVoteNet,
    appdb: &Arc<AppDb>,
    labels: &[String],
    e: &MasternodeListSummary,
    key: NameVotingKey,
) -> Result<NameVoter, GovernanceError> {
    let (wire, platform) = (e.pro_tx_hash, platform_order(&e.pro_tx_hash));
    let mut choices = net.votes_of(platform).await?;
    let (db, labels) = (Arc::clone(appdb), labels.to_vec());
    let used = tokio::task::spawn_blocking(move || {
        (labels.into_iter())
            .map(|l| Ok((votes_used(&db, &wire, &l)?, l)))
            .collect::<Result<Vec<_>, EngineError>>()
    })
    .await
    .map_err(EngineError::from)??;
    let current = used
        .into_iter()
        .filter_map(|(votes_used, label)| {
            let choice = choices.remove(&label);
            (choice.is_some() || votes_used > 0).then_some(NameVoterState {
                label,
                choice,
                votes_used,
            })
        })
        .collect();
    Ok(NameVoter {
        pro_tx_hash: display_hex(&e.pro_tx_hash),
        kind: if e.is_evonode {
            MasternodeType::Evonode
        } else {
            MasternodeType::Regular
        },
        weight: ListSnapshot::weight(e),
        valid: e.is_valid,
        key,
        current,
    })
}

/// `label`'s poll can take a vote for `choice` (`vault`, checked): it exists,
/// is not resolved or past its end, and lists the identity voted for (D8,
/// D9).
async fn check_open_poll(
    net: &impl NameVoteNet,
    label: &str,
    choice: &NameChoice,
    vault: dw_vault::NameChoice,
) -> Result<(), GovernanceError> {
    let now = unix_now();
    // From an hour back, as the watch reads it: a poll stays in the
    // index until the block that resolves it (D8).
    let ends = net.end_page(now.saturating_sub(3600), ENDS_PAGE).await?;
    let poll = net.page(label, POLL_PAGE).await?;
    let closed = || GovernanceError::ContestClosed {
        label: label.to_owned(),
    };
    if poll.winner.is_some() {
        return Err(closed());
    }
    if poll.contenders.is_empty() {
        return Err(GovernanceError::ContestNotFound {
            label: label.to_owned(),
        });
    }
    if ends.get(label).is_some_and(|end| *end <= now) {
        return Err(closed());
    }
    if let NameChoice::TowardsIdentity { identity } = choice
        && let dw_vault::NameChoice::TowardsIdentity(id) = vault
        && !poll.contenders.iter().any(|(c, _)| c.to_buffer() == id)
    {
        return Err(GovernanceError::NotAContender {
            identity: identity.clone(),
        });
    }
    Ok(())
}

impl Governance {
    /// The open DPNS contests, soonest ending first, and the wallet's
    /// masternodes that can vote on them, with their present choices and
    /// the votes this app saw them use (MG-07 §1).
    pub async fn contested_names(
        &self,
        wallet: WalletId,
    ) -> Result<ContestedNames, GovernanceError> {
        #[cfg(test)]
        if let Some(fake) = self.fake_names() {
            return self.contested_names_on(wallet, Ok(&*fake)).await;
        }
        let net = sdk_names(&self.session);
        self.contested_names_on(wallet, net.as_ref().map_err(Clone::clone))
            .await
    }

    async fn contested_names_on(
        &self,
        wallet: WalletId,
        net: Result<&impl NameVoteNet, PlatformError>,
    ) -> Result<ContestedNames, GovernanceError> {
        let list = self.name_list().await?;
        self.session.require_wallet(&wallet)?;
        let keys = self
            .session
            .masternode_keys(wallet, MasternodeKeyRole::Voting, 0, VOTING_KEY_INDEXES)
            .await?;
        let appdb = self.session.live()?.appdb;
        let notes = {
            let appdb = Arc::clone(&appdb);
            tokio::task::spawn_blocking(move || load_notes(&appdb))
                .await
                .map_err(EngineError::from)?
                .map_err(EngineError::from)?
        };
        let voters = name_voters(&list, &keys, &notes);

        let net = net?;
        let contests = open_contests(net).await?;
        let labels: Vec<String> = contests.iter().map(|c| c.label.clone()).collect();
        let mut out = Vec::with_capacity(voters.len());
        for (e, key) in voters {
            out.push(name_voter(net, &appdb, &labels, e, key).await?);
        }
        Ok(ContestedNames {
            contests,
            voters: out,
            votes_allowed: net.votes_allowed(),
        })
    }

    /// Casts `choice` on the DPNS poll of `label` for each masternode
    /// (proTxHash, display order) under one `Governance` grant, a `voting`
    /// lease ended when the call returns (MG-07 §1–§2). Each vote is signed
    /// in the vault, admitted through the fence, broadcast, and waited for.
    ///
    /// The call fails as a whole, before the grant is redeemed, for a label
    /// that cannot be contested, a malformed choice or proTxHash list
    /// (`invalid_argument`), without an SPV list, without Platform, and for
    /// a poll that does not exist, is closed, or does not list the
    /// identity voted for (mg-engine §4). Otherwise each masternode gets a
    /// [`NameVoteCast`]: the transition's hash, or why the vote does not
    /// count. The key is chosen as `vote` chooses it.
    pub async fn vote_name(
        &self,
        wallet: WalletId,
        label: String,
        choice: NameChoice,
        masternodes: Vec<String>,
        grant: String,
    ) -> Result<Vec<NameVoteCast>, GovernanceError> {
        #[cfg(test)]
        if let Some(fake) = self.fake_names() {
            return (self.vote_name_on(wallet, label, choice, masternodes, grant, Ok(&*fake)))
                .await;
        }
        let net = sdk_names(&self.session);
        let net = net.as_ref().map_err(Clone::clone);
        self.vote_name_on(wallet, label, choice, masternodes, grant, net)
            .await
    }

    async fn vote_name_on(
        &self,
        wallet: WalletId,
        label: String,
        choice: NameChoice,
        masternodes: Vec<String>,
        grant: String,
        net: Result<&impl NameVoteNet, PlatformError>,
    ) -> Result<Vec<NameVoteCast>, GovernanceError> {
        let label = contested_label(&label)?;
        let vault_choice = vault_choice(&choice)?;
        // Format and repeats first; the count needs the list.
        let targets = parse_masternodes(&masternodes, usize::MAX)?;
        let list = self.name_list().await?;
        parse_masternodes(&masternodes, list.len())?;
        let net = net?;
        check_open_poll(net, &label, &choice, vault_choice).await?;
        let allowed = net.votes_allowed();

        let _op = self.session.enter().await?;
        self.session.require_wallet(&wallet)?;
        let appdb = self.session.live()?.appdb;
        let lease = (self.session)
            .begin_lease(wallet, FlowKind::Voting, &[grant])
            .await
            .map_err(PlatformError::from)?;
        let signer = Arc::new(lease.vote_signer().map_err(PlatformError::from)?);
        let keychain: Arc<Keychain> = {
            let signer = Arc::clone(&signer);
            Arc::new(
                tokio::task::spawn_blocking(move || {
                    (0..VOTING_KEY_INDEXES)
                        .map(|index| Ok((signer.key_id(&VotingKey::Keychain { index })?, index)))
                        .collect()
                })
                .await
                .map_err(EngineError::from)?,
            )
        };
        let batch = Batch {
            net,
            lease: &lease,
            signer,
            keychain,
            appdb,
            allowed,
            label: &label,
            choice: vault_choice,
        };
        let mut out = Vec::with_capacity(targets.len());
        for wire in targets {
            let target = Target {
                wire,
                voting_key_id: list
                    .iter()
                    .find(|e| e.pro_tx_hash == wire)
                    .map(|e| e.voting_key_id),
            };
            let (transition, rejected) = match batch.cast_one(&target).await {
                Ok(hash) => (Some(hex::encode(hash)), None),
                Err((hash, reason)) => (hash.map(hex::encode), Some(reason.into())),
            };
            out.push(NameVoteCast {
                pro_tx_hash: display_hex(&wire),
                transition,
                rejected,
            });
        }
        lease.end();
        Ok(out)
    }

    /// The synced SPV list's entries, or `governance.list_unavailable`.
    async fn name_list(&self) -> Result<Vec<MasternodeListSummary>, GovernanceError> {
        #[cfg(test)]
        if let Some(entries) = (self.session.governance.name_list.lock())
            .unwrap_or_else(|p| p.into_inner())
            .clone()
        {
            return Ok(entries);
        }
        let manager = self.session.manager()?;
        let list = read_list(&self.session, &manager).await;
        if list.entries.is_empty() {
            return Err(GovernanceError::ListUnavailable);
        }
        Ok(list.entries)
    }

    #[cfg(test)]
    fn fake_names(&self) -> Option<Arc<tests::FakeNames>> {
        (self.session.governance.name_net.lock())
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

/// What the calls ask of Platform: the SDK, or `platform.unavailable` for
/// a session without Platform.
fn sdk_names(session: &NetworkSession) -> Result<SdkNames, PlatformError> {
    if !session.platform.enabled() {
        return Err(PlatformError::Unavailable);
    }
    let manager = session.manager().map_err(|_| PlatformError::Unavailable)?;
    Ok(SdkNames(manager.sdk_arc()))
}

/// One masternode of a `vote_name` batch: its proTxHash (wire order, as
/// the list and the vault keep it) and its list entry's voting key id.
struct Target {
    wire: [u8; 32],
    voting_key_id: Option<[u8; 20]>,
}

/// A failed vote: the transition's hash once it was broadcast, and why it
/// does not count.
type Rejection = (Option<[u8; 32]>, &'static str);

fn no_hash(reason: &'static str) -> Rejection {
    (None, reason)
}

/// A blocking task or a store read that did not run to its end.
fn unavailable<E>(_: E) -> Rejection {
    no_hash("signer_unavailable")
}

/// What the masternodes of one `vote_name` call share.
struct Batch<'a, N> {
    net: &'a N,
    lease: &'a Lease,
    signer: Arc<VoteSigner>,
    keychain: Arc<Keychain>,
    appdb: Arc<AppDb>,
    /// Platform's votes per masternode and poll.
    allowed: u32,
    label: &'a str,
    choice: dw_vault::NameChoice,
}

impl<N: NameVoteNet> Batch<'_, N> {
    /// One masternode's vote: the key, the nonce, the vault's signature,
    /// then the fence and the broadcast (MG-07 §2). The transition's hash,
    /// or why the vote does not count with the hash when it was broadcast.
    async fn cast_one(&self, t: &Target) -> Result<[u8; 32], Rejection> {
        let id = t.voting_key_id.ok_or(no_hash("unknown_masternode"))?;
        let used = {
            let (db, mn, label) = (Arc::clone(&self.appdb), t.wire, self.label.to_owned());
            tokio::task::spawn_blocking(move || votes_used(&db, &mn, &label))
                .await
                .map_err(unavailable)?
                .map_err(unavailable)?
        };
        if used >= self.allowed {
            return Err(no_hash("vote_limit"));
        }
        let key = {
            let (signer, keychain, mn) =
                (Arc::clone(&self.signer), Arc::clone(&self.keychain), t.wire);
            tokio::task::spawn_blocking(move || choose_key(&keychain, id, mn, |k| signer.key_id(k)))
                .await
                .map_err(unavailable)?
                .map_err(no_hash)?
        };
        let platform = platform_order(&t.wire);
        let nonce = (self.net)
            .nonce(voter_identity(&platform, &id))
            .await
            .map_err(|_| no_hash("platform_unavailable"))?;
        let fields = NameVote {
            pro_tx_hash: platform,
            label: self.label.to_owned(),
            choice: self.choice,
            nonce,
        };
        let signed = {
            let signer = Arc::clone(&self.signer);
            tokio::task::spawn_blocking(move || signer.sign_name_vote(&key, &fields))
                .await
                .map_err(unavailable)?
                .map_err(|e: SignerError| no_hash(refused(e)))?
        };
        let hash = signed.hash;
        let outcome = self.hand_off(signed.bytes, hash).await?;
        let rejected = match outcome {
            Cast::Counted => {
                self.record_used(t.wire, used + 1).await;
                return Ok(hash);
            }
            Cast::Refused {
                refusal: Refusal::VoteLimit { used: counted },
                ..
            } => {
                self.record_used(t.wire, u32::from(counted).max(used)).await;
                "vote_limit"
            }
            Cast::Refused {
                refusal: Refusal::VotingKey,
                ..
            } => self.net.diagnose(platform, id).await.unwrap_or("refused"),
            Cast::Refused {
                refusal: Refusal::Reason(r),
                ..
            } => r,
            // It never left: no transition to name.
            Cast::NotSent => return Err(no_hash("platform_unavailable")),
            Cast::Unknown => "unconfirmed",
        };
        Err((Some(hash), rejected))
    }

    /// One signed transition through the dispatch fence (MG-05 §4, MG-07
    /// §2): admitted under the lease's scope, broadcast under a First's
    /// permit deadline (so nothing leaves after a lock's drain), then
    /// waited for after the permit ended. A refusal of the fence, or a
    /// deadline that came first, is `Err` with no hash.
    async fn hand_off(&self, bytes: Vec<u8>, hash: [u8; 32]) -> Result<Cast, Rejection> {
        let wallet = self.lease.wallet();
        let table = Arc::clone(&self.lease.table);
        let verdict = self
            .lease
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
        // A single attempt refused at CheckTx never entered a block.
        let outcome = |b: &Broadcast| match b {
            Broadcast::Accepted => Outcome::Sent,
            Broadcast::Refused(_) | Broadcast::NotSent => Outcome::NotSent,
            Broadcast::Unknown => Outcome::MaybeSent,
        };
        let sent = match verdict {
            Verdict::First(permit) => {
                let ready = async { Ok::<_, std::convert::Infallible>(()) };
                let handed = (self.lease.table)
                    .hand_off(Some(permit.deadline()), ready, |()| {
                        self.net.broadcast(bytes.clone())
                    })
                    .await;
                match handed {
                    Ok(HandOff::Done(b)) => {
                        permit.finish(outcome(&b));
                        b
                    }
                    Ok(HandOff::NotEntered) => {
                        permit.finish(Outcome::NotSent);
                        return Err(no_hash("cancelled"));
                    }
                    Ok(HandOff::Cut) => {
                        permit.finish(Outcome::MaybeSent);
                        return Ok(Cast::Unknown);
                    }
                    Err(never) => match never {},
                }
            }
            Verdict::Resend(guard) | Verdict::FirstUnleased(guard) => {
                let b = self.net.broadcast(bytes.clone()).await;
                guard.finish(outcome(&b));
                b
            }
            // The lease was revoked: a lock ended the batch.
            Verdict::Refused { .. } => return Err(no_hash("cancelled")),
            // The same transition is in flight, or its outcome is still
            // being resolved.
            Verdict::Deferred => return Err(no_hash("in_flight")),
        };
        Ok(match sent {
            Broadcast::Accepted => self.net.wait(bytes).await,
            Broadcast::Refused(refusal) => Cast::Refused {
                refusal,
                executed: false,
            },
            Broadcast::NotSent => Cast::NotSent,
            Broadcast::Unknown => Cast::Unknown,
        })
    }

    /// Records the votes Platform took; a failed write only lowers the
    /// lower bound (D6).
    async fn record_used(&self, pro_tx_hash: [u8; 32], used: u32) {
        let (db, label) = (Arc::clone(&self.appdb), self.label.to_owned());
        let written =
            tokio::task::spawn_blocking(move || set_votes_used(&db, &pro_tx_hash, &label, used))
                .await;
        if !matches!(written, Ok(Ok(()))) {
            tracing::warn!("a name vote's count was not recorded");
        }
    }
}

#[cfg(test)]
#[path = "contested_tests.rs"]
pub(super) mod tests;
