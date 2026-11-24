//! A registration's identity create from the wallet's Platform addresses
//! (C-funding F8–F10, F22), under the move lock: one proved read, the plan
//! written into the row, a `Register` record written and read back, and
//! only then the signed create at the plan's nonces. Settled by evidence
//! alone: the SDK's proved result, or the nonces taken together with a
//! proved read of the identity or of the exit's withdrawal document.

use std::future::Future;

use crate::platform::flows::BudgetPurpose;
use crate::platform::keys_policy::registration_specs;
use crate::platform::lease::Lease;
use crate::platform::registration_platform::{self, register_fee, register_plan};
use crate::platform::registration_rows::PlatformPlan;

use super::*;

/// What one funding pass of a Platform-funded registration ended with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RegisterOutcome {
    /// The planned identity exists, proved (base58).
    Registered(String),
    /// It may still execute and no evidence says it did: the row waits.
    Unknown,
    /// Its record failed by height and may still execute: the row waits
    /// for "Move to Dash wallet" or another try (F22).
    Held,
    /// The exit at its inputs and nonces executed: the create never can.
    Exited,
    /// Its nonces are taken, the identity is absent and no exit executed:
    /// a create that failed inside a block, paid (F10).
    PaidFailure,
    /// Its nonces are taken by a create no record of this draft made and
    /// its identity is proved present: the row's only if it is the seed's
    /// identity at the row's index, else the row plans again.
    Adopt,
    /// It can never execute and nothing of it was taken: the plan is
    /// cleared and the row may plan again; with the refusal that ended a
    /// call which released no signature.
    Replan(Option<PlatformError>),
}

/// One funding pass of a row: the numbers its quote showed, and the plan
/// it stored, if any.
#[derive(Debug, Clone)]
pub(crate) struct RegisterAsk {
    pub(crate) draft: i64,
    pub(crate) index: u32,
    pub(crate) funding_credits: u64,
    pub(crate) fee_reserve_credits: u64,
    pub(crate) plan: Option<PlatformPlan>,
    /// A record failed by height is signed again (once per run).
    pub(crate) resign: bool,
}

fn record_id(draft: i64, identity: &str) -> String {
    format!("register-{draft}-{identity}")
}

/// `PlatformError::Internal` with `detail`.
fn internal(detail: impl Into<String>) -> PlatformError {
    PlatformError::Internal {
        detail: detail.into(),
    }
}

/// A plan's durable write into its row.
pub(crate) type PlanWrite<'a> =
    std::pin::Pin<Box<dyn Future<Output = Result<(), PlatformError>> + Send + 'a>>;

/// What writes a new plan into its row.
pub(crate) type PlanWriter<'a> = Box<dyn FnOnce(PlatformPlan) -> PlanWrite<'a> + Send + 'a>;

/// One funding pass, as the registration flow awaits it.
pub(crate) type PlanFund<'a> =
    std::pin::Pin<Box<dyn Future<Output = Result<RegisterOutcome, CreditsError>> + Send + 'a>>;

/// The create of `ask`'s row under the move lock (C-funding F9, F10).
/// `write_plan` writes a new plan into the row, durably, before its record
/// is written and before anything is signed; a refusal before it writes
/// nothing. Settling reads only; a plan is made or signed only under a
/// `lease` that carries the addresses' signer (`platform.needs_grant`).
pub(crate) async fn register_fund<'w>(
    net: &impl MoveNet,
    session: &NetworkSession,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
    (ask, lease): (RegisterAsk, Option<&Lease>),
    write_plan: impl FnOnce(PlatformPlan) -> PlanWrite<'w> + Send,
) -> Result<RegisterOutcome, CreditsError> {
    registration_platform::refuse_on_mainnet(session.network())?;
    let version = net.sends_version().await;
    sends_active(version)?;
    let lock = wallet_lock(wallet_id);
    let _one = lock.lock().await;
    let network = session.vault.network();
    let store = Store::of(session, wallet_id)?;
    let mut moves = store.moves().await?;
    let pool = pool(wallet).await?;
    let mut addresses = covered(&pool, &moves);
    if let Some(plan) = &ask.plan {
        addresses.extend(plan.inputs.iter().filter_map(|(a, _, _)| parse(a)));
    }
    // Rule 5 and the one read every decision below rests on.
    let seen = read(session, net, addresses, true).await?;
    let mut funds = store.funds().await?;
    let held = fund_holds(session, wallet_id);
    let evidence = exits(session, net, &moves, &seen).await;
    let docs = documents(session, net, &moves, &seen).await;
    if observe(
        &store,
        (&mut moves, &mut funds),
        held,
        (&seen, &evidence, &docs),
    )
    .await?
    {
        session
            .hub
            .pump
            .mark_platform(wallet_id, [PlatformChange::PlatformAddresses]);
    }
    super::contact_payout::settled(session, wallet_id, &moves).await;
    let plan = match &ask.plan {
        Some(plan) => plan.clone(),
        None => {
            signing(lease)?;
            let plan = new_plan(&moves, &seen, (&pool, network), &ask, version)?;
            write_plan(plan.clone()).await?;
            plan
        }
    };
    let inputs: BTreeMap<PlatformAddress, (u32, u64)> = (plan.inputs.iter())
        .map(|(a, n, c)| Some((parse(a)?, (*n, *c))))
        .collect::<Option<_>>()
        .ok_or_else(|| internal("a stored plan names an address that does not parse"))?;
    // A plan signs only as it was made: distinct addresses, each at a used
    // nonce's successor and claiming something.
    if inputs.len() != plan.inputs.len() || inputs.values().any(|&(n, c)| n == 0 || c == 0) {
        return Err(internal("a stored plan repeats an address or claims nothing").into());
    }
    let identity_id = Identifier::from_string(&plan.identity, Encoding::Base58)
        .map_err(|_| internal("a stored plan names no identity"))?;
    let id = record_id(ask.draft, &plan.identity);
    let mine = moves.iter().position(|m| m.id == id);
    let nonces: Vec<Option<u32>> = (plan.inputs.iter())
        .map(|(a, _, _)| seen.nonce(a))
        .collect();
    if nonces.iter().any(Option::is_none) {
        return Ok(RegisterOutcome::Unknown);
    }
    let taken = (plan.inputs.iter().zip(&nonces))
        .filter(|((_, signed, _), now)| now.is_some_and(|n| n >= *signed))
        .count();
    if taken == plan.inputs.len() {
        let outcome = match mine {
            Some(at) => settled(session, net, &moves, at, (identity_id, &seen)).await,
            None => unsigned(session, net, &moves, &id, (identity_id, &seen)).await,
        };
        return Ok(outcome);
    }
    if taken > 0 {
        // Another transition took one input's nonce: this one never can.
        if let Some(at) = mine {
            let record = &mut moves[at];
            if record.holds() {
                record.state = PlatformActivityState::Failed;
                record.can_land = false;
                store.commit(vec![move_write(record)?]).await?;
            }
        }
        return Ok(RegisterOutcome::Replan(None));
    }
    // Every nonce unused: the plan may still execute.
    let beside = (moves.iter().enumerate()).find(|(i, m)| {
        Some(*i) != mine && (m.holds() || m.state == PlatformActivityState::Pending)
    });
    let record = mine.map(|at| &moves[at]);
    // A record that never executes: a stored plan is cleared; a new plan
    // at the same nonces (the same identity) signs in its place.
    let dead = record.is_some_and(|m| !m.can_land);
    match record {
        Some(m)
            if m.state == PlatformActivityState::Pending
                || m.state == PlatformActivityState::Done =>
        {
            return Ok(RegisterOutcome::Unknown);
        }
        Some(_) if dead && ask.plan.is_some() => return Ok(RegisterOutcome::Replan(None)),
        Some(_) if !dead && (!ask.resign || beside.is_some()) => {
            return Ok(RegisterOutcome::Held);
        }
        _ => {
            if let Some((_, h)) = beside {
                return Err(CreditsError::MovePending { id: h.id.clone() });
            }
        }
    }
    // A held create is signed again only on its own terms.
    if let Some(m) = record.filter(|m| m.holds()) {
        let mut recorded: Vec<_> = (m.inputs.iter())
            .map(|i| (i.address.as_str(), i.nonce, i.credits))
            .collect();
        let mut planned: Vec<_> = (plan.inputs.iter())
            .map(|(a, n, c)| (a.as_str(), *n, *c))
            .collect();
        recorded.sort_unstable();
        planned.sort_unstable();
        if recorded != planned {
            return Err(internal("a held create's record differs from its plan").into());
        }
    }
    let lease = signing(lease)?;
    sign(
        net,
        session,
        (&store, &mut moves, mine),
        (&pool, &seen, network, version),
        (&ask, lease, &plan, inputs),
    )
    .await
}

/// A Platform-funded row's create, as its stored record reads with no
/// network: failed by height and able to land (C-funding F22).
pub(crate) async fn register_held(
    session: &NetworkSession,
    wallet_id: WalletId,
    draft: i64,
) -> Result<bool, PlatformError> {
    let prefix = record_id(draft, "");
    let moves = Store::of(session, wallet_id)?.moves().await?;
    Ok((moves.iter().rev())
        .find(|m| m.kind == MoveKind::Register && m.id.starts_with(&prefix))
        .is_some_and(|m| m.holds() && m.state == PlatformActivityState::Failed))
}

/// The lease a plan is made or signed under: one carrying the addresses'
/// signer, else `platform.needs_grant` with nothing written.
fn signing(lease: Option<&Lease>) -> Result<&Lease, PlatformError> {
    let lease = lease.ok_or(PlatformError::NeedsGrant {
        purpose: BudgetPurpose::Credits,
    })?;
    lease.call_platform_address_signer()?;
    Ok(lease)
}

/// A new plan for `ask`'s numbers over `seen`: refused while any move may
/// still execute or the read is behind one already settled, when it no
/// longer gives the quoted numbers, or when its inputs could not be moved
/// to Core afterwards.
fn new_plan(
    moves: &[MoveRecord],
    seen: &Seen,
    (pool, network): (&BTreeMap<PlatformAddress, DerivationPath>, Network),
    ask: &RegisterAsk,
    version: &PlatformVersion,
) -> Result<PlatformPlan, CreditsError> {
    if let Some(h) = (moves.iter()).find(|m| m.holds() || m.state == PlatformActivityState::Pending)
    {
        return Err(CreditsError::MovePending { id: h.id.clone() });
    }
    behind(moves, seen)?;
    let balances = proved_balances(pool, seen, network);
    let nonces = (pool.keys())
        .filter_map(|a| Some((*a, seen.nonce(&bech32m(a, network))?)))
        .collect();
    let keys = registration_specs().len();
    let planned = register_plan((&balances, &nonces), ask.funding_credits, keys, version)?;
    if (planned.funding_credits, planned.fee_reserve_credits)
        != (ask.funding_credits, ask.fee_reserve_credits)
    {
        return Err(CreditsError::QuoteChanged);
    }
    let (fee_source, _) = (planned.inputs.first_key_value()).ok_or(PlatformError::Unavailable)?;
    let as_move = Plan {
        inputs: (planned.inputs.iter())
            .map(|(a, (_, c))| (*a, *c))
            .collect(),
        fee_source: *fee_source,
        fee: planned.fee_credits,
        reserve: planned.fee_reserve_credits,
        fee_from_output: false,
    };
    // The way out of a held create is a move of its inputs to Core.
    exit_possible(&as_move, &balances, version)?;
    let identity = dpp::state_transition::identity_id_from_input_addresses(&planned.inputs)
        .map_err(|e| internal(e.to_string()))?;
    Ok(PlatformPlan {
        inputs: (planned.inputs.iter())
            .map(|(a, (n, c))| (bech32m(a, network), *n, *c))
            .collect(),
        identity: identity.to_string(Encoding::Base58),
    })
}

/// Whether identity `id` exists, proved at or above `seen`'s height under
/// rule 5; `None` when the read fails, is behind or is unverified.
async fn identity(
    session: &NetworkSession,
    net: &impl MoveNet,
    id: Identifier,
    seen: &Seen,
) -> Option<bool> {
    let (read, tally) = trust::scoped(net.identity_exists(id)).await;
    let gated = session.trust.rule_5() == Rule5::Enforce
        && session.live().is_ok_and(|l| l.provenance.fallback_used());
    match read {
        Ok(_) if gated && !tally.verified() => {
            tracing::info!(%id, "a registration's identity read was unverified");
            None
        }
        Ok((present, height)) if height >= seen.height => Some(present),
        Ok(_) => {
            tracing::info!(%id, "a registration's identity read was behind");
            None
        }
        Err(e) => {
            tracing::info!(%id, "reading a registration's identity: {e}");
            None
        }
    }
}

/// The plan's nonces are taken and `moves[at]` is its record: the exit's
/// document present says the exit executed; else the identity, proved at or
/// above the nonces' proof, says whether the create made it (F10).
async fn settled(
    session: &NetworkSession,
    net: &impl MoveNet,
    moves: &[MoveRecord],
    at: usize,
    (id, seen): (Identifier, &Seen),
) -> RegisterOutcome {
    let key = moves[at].key();
    let exits: Vec<&MoveRecord> = (moves.iter())
        .filter(|m| m.kind == MoveKind::ToCore && m.key() == key)
        .collect();
    if exits.iter().any(|m| m.state == PlatformActivityState::Done) {
        return RegisterOutcome::Exited;
    }
    // An exit beside it whose document no read decided yet.
    if exits.iter().any(|m| m.holds()) {
        return RegisterOutcome::Unknown;
    }
    // A record that could never execute did not take the nonces.
    if !moves[at].can_land {
        return unsigned(session, net, moves, &moves[at].id, (id, seen)).await;
    }
    match identity(session, net, id, seen).await {
        Some(true) => RegisterOutcome::Registered(moves[at].to.clone()),
        Some(false) => RegisterOutcome::PaidFailure,
        None => RegisterOutcome::Unknown,
    }
}

/// The plan's nonces are taken and no create of this draft that could
/// execute took them: another draft's create of the same identity did (plan
/// again), or another install's, whose identity is this row's only if it is
/// the seed's at the row's index.
async fn unsigned(
    session: &NetworkSession,
    net: &impl MoveNet,
    moves: &[MoveRecord],
    own: &str,
    (id, seen): (Identifier, &Seen),
) -> RegisterOutcome {
    let planned = id.to_string(Encoding::Base58);
    let others = (moves.iter())
        .any(|m| m.kind == MoveKind::Register && m.id != own && m.to == planned && m.can_land);
    if others {
        return RegisterOutcome::Replan(None);
    }
    // A stored plan may have been signed by another install of the wallet
    // (a restored row): nonces taken with the identity absent are paid.
    match identity(session, net, id, seen).await {
        Some(true) => RegisterOutcome::Adopt,
        Some(false) => RegisterOutcome::PaidFailure,
        None => RegisterOutcome::Unknown,
    }
}

/// Records the plan's create, signs it and sends it, then settles what can
/// be settled: the SDK's proved result, or no signature released.
async fn sign(
    net: &impl MoveNet,
    session: &NetworkSession,
    (store, moves, mine): (&Store, &mut Vec<MoveRecord>, Option<usize>),
    (pool, seen, network, version): (
        &BTreeMap<PlatformAddress, DerivationPath>,
        &Seen,
        Network,
        &'static PlatformVersion,
    ),
    (ask, lease, plan, inputs): (
        &RegisterAsk,
        &Lease,
        &PlatformPlan,
        BTreeMap<PlatformAddress, (u32, u64)>,
    ),
) -> Result<RegisterOutcome, CreditsError> {
    let wallet_id = lease.wallet();
    let keys = registration_specs();
    let claims: u64 = inputs.values().map(|(_, c)| *c).sum();
    let debit = claims.saturating_add(ask.fee_reserve_credits);
    let first = inputs.keys().next().copied();
    let mut record = MoveRecord {
        id: record_id(ask.draft, &plan.identity),
        kind: MoveKind::Register,
        inputs: (inputs.iter())
            .map(|(a, (nonce, credits))| {
                let address = bech32m(a, network);
                MoveInput {
                    before: seen.credits(&address).unwrap_or(0),
                    address,
                    nonce: *nonce,
                    credits: *credits,
                    fee_source: Some(*a) == first,
                    unseen: 0,
                }
            })
            .collect(),
        credits: debit,
        fee_credits: register_fee(version, inputs.len(), keys.len()),
        expected_duffs: 0,
        to: plan.identity.clone(),
        fee_from_output: false,
        at: unix_now(),
        state: PlatformActivityState::Pending,
        signed: false,
        answered: false,
        // Until a definitive refusal says otherwise, it may be signed and
        // may execute: so it is recorded, and so a restart reads it.
        can_land: true,
        seen_at: None,
        fee_taken: None,
        group: None,
        version: Some(version.protocol_version),
    };
    let paths: BTreeMap<[u8; 20], DerivationPath> = (inputs.keys())
        .map(|a| match (a, pool.get(a)) {
            (PlatformAddress::P2pkh(hash), Some(path)) => Ok((*hash, path.clone())),
            _ => Err(PlatformError::Unavailable),
        })
        .collect::<Result<_, _>>()?;
    let permit = write_permit(lease, "platform_registration", debit)?;
    let signers = match (
        permit.platform_address_signer(paths),
        permit.identity_signer(&[ask.index]),
    ) {
        (Ok(address), Ok(identity)) => (address, identity),
        (Err(e), _) | (_, Err(e)) => {
            permit.finish(false);
            return Err(PlatformError::from(e).into());
        }
    };
    let built = match signers.1.derive_public_keys(ask.index, &keys).await {
        Ok(keys) => Identity::new_with_input_addresses_and_keys(&inputs, keys, version)
            .map_err(|e| internal(e.to_string())),
        Err(e) => Err(PlatformError::from(e)),
    };
    let identity = match built {
        Ok(identity) if identity.id().to_string(Encoding::Base58) == plan.identity => identity,
        Ok(_) => {
            permit.finish(false);
            return Err(internal("the plan's inputs give another identity").into());
        }
        Err(e) => {
            permit.finish(false);
            return Err(e.into());
        }
    };
    // Recorded and read back before anything is signed.
    if let Err(e) = store.commit(vec![move_write(&record)?]).await {
        drop(signers);
        permit.finish(false);
        return Err(e.into());
    }
    session
        .hub
        .pump
        .mark_platform(wallet_id, [PlatformChange::PlatformAddresses]);
    let call = leased(
        lease,
        net.register(&identity, inputs, ask.index, (&signers.1, &signers.0)),
    )
    .await;
    let released = permit.signatures().released();
    drop(signers);
    permit.finish(call.is_ok());
    record.signed = released > 0;
    record.answered = call.is_ok();
    // In the process that made the call: no signature left it, or the
    // structure check refused it before any block (C-funding F10).
    // A released signature may have reached a node whatever came back: only
    // a create that released none can never execute.
    let cannot_land = released == 0;
    if let Err(e) = &call {
        tracing::info!(id = %record.id, cannot_land, "a registration's create failed: {e}");
    }
    // A shortfall in this signing says nothing about the held one it
    // replaced: that one still holds, and the row waits on it (F22).
    let keep_prior = cannot_land && mine.is_some_and(|at| moves[at].holds());
    // A stored plan this install holds no record of (a restored row) may
    // have been signed elsewhere: its record holds as signed may.
    let keep_new = cannot_land && mine.is_none() && ask.plan.is_some();
    let at = match mine {
        Some(at) => {
            if !keep_prior {
                moves[at] = record;
            }
            at
        }
        None => {
            moves.push(record);
            moves.len() - 1
        }
    };
    if cannot_land {
        if !keep_prior && !keep_new {
            moves[at].state = PlatformActivityState::Failed;
            moves[at].can_land = false;
        }
    } else {
        // Done only on its proved nonces, whatever the call answered.
        let signed = moves[at].inputs.iter().filter_map(|i| parse(&i.address));
        match read(session, net, signed.collect(), false).await {
            Ok(after) => {
                let evidence = exits(session, net, moves, &after).await;
                let docs = documents(session, net, moves, &after).await;
                settle_with(moves, &after, &evidence);
                settle_payouts(moves, &after, &docs);
            }
            Err(e) => tracing::info!(id = %moves[at].id, "reading a registration's nonces: {e}"),
        }
    }
    let writes: Vec<_> = moves.iter().map(move_write).collect::<Result<_, _>>()?;
    if let Err(e) = commit_outcome(store, writes).await {
        // The record stored before signing stands, `pending` and able to
        // land: the next read settles it by its nonces.
        tracing::warn!(id = %moves[at].id, "storing a registration's outcome: {e}");
    }
    super::contact_payout::settled(session, wallet_id, moves).await;
    session
        .hub
        .pump
        .mark_platform(wallet_id, [PlatformChange::PlatformAddresses]);
    Ok(match call {
        Ok(created) if created.to_string(Encoding::Base58) == plan.identity => {
            RegisterOutcome::Registered(plan.identity.clone())
        }
        Ok(created) => {
            tracing::error!(%created, planned = %plan.identity, "the create answered another identity");
            return Err(internal("the create answered another identity").into());
        }
        Err(_) if keep_prior => RegisterOutcome::Held,
        Err(_) if keep_new => RegisterOutcome::Unknown,
        Err(e) if cannot_land => RegisterOutcome::Replan(Some(PlatformError::from(e))),
        Err(_) => RegisterOutcome::Unknown,
    })
}
