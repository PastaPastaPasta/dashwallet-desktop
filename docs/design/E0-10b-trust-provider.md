# E0-10b: the layered quorum provider (`platform/trust.rs`)

Status: design notes for ROADMAP row E0-10b (DASHPAY §2.2 rules 1–4 and 6). It ships in the degraded mode of §2.2
(DEC-168): E0-10a failed R7 at platform's pin, and full enforcement (DP3-04 / H-09) waits for chain anchoring of
quorum lists (dashpay/rust-dashcore#1117).

Counterpart (ROADMAP §0, second-agent review): iOS builds its SDK with `TrustedHttpContextProvider`
(`rs-sdk-ffi/src/sdk.rs`, `dash_sdk_create_trusted`) or a host callback provider (`rs-sdk-ffi/src/context_provider.rs`,
`CallbackContextProvider`). Neither consults SPV. The SPV lookup is platform-wallet's
`SpvRuntime::get_quorum_public_key_with_status` (platform#5366, on the fork pin), which E0-10b is the first to call.

## 1. Where it sits

The SDK calls `ContextProvider::get_quorum_public_key(type, hash, core_chain_locked_height)` synchronously from a
tokio worker, once per proof it verifies, for reads and for the result proofs of writes. An `Err` makes the proof
fail verification (`drive_proof_verifier::Error::ContextProviderError`). The fetch is never accepted unproven.

```
SdkBuilder::with_context_provider(SharedContext)
  SharedContext ── quorum keys ──▶ TrustProvider (trust.rs)
                │                    ├─ SpvQuorums: SpvRuntime::get_quorum_public_key_with_status (+ cache)
                │                    ├─ TrustedQuorums: LazyTrustedContext (context.rs, unchanged)
                │                    ├─ Provenance (E0-06): fallback latch, dp_trust_verified
                │                    └─ masternode sync flag (SessionHub progress)
                └─ contracts, token configs, activation height ──▶ LazyTrustedContext (rule 6)
```

The SDK is built before the wallet manager, so `SpvRuntime` and `Provenance` are attached right after the manager
exists (`TrustProvider::attach`). A lookup before that serves no fallback: the latch cannot be set yet, so it fails
closed.

## 2. Modes

| Mode | When | Quorum key source | `QuorumSource` |
|---|---|---|---|
| `Degraded` | **default** (DEC-168) | the trusted service; when SPV holds the quorum, its key is compared, and a mismatch refuses. A `Verified` SPV key is served when the service is unreachable | `Trusted` |
| `Enforcing` | R7 full mode (H-09 makes it the default once #1117 lands); selectable in tests | SPV `Verified` only; the trusted service before masternode sync, for reads | `Spv`, or `TrustedFallback` before masternode sync |
| trusted-only toggle | developer setting `trust.trusted_only` (global, app.sqlite; read when a session opens, applied at once to the session that sets it) | the trusted service only; SPV is not consulted | `Trusted` |

The toggle wins over the mode. It exists to compare behaviour with iOS and to work around a broken SPV state during
development. It is not offered as a user setting.

## 3. One lookup

`SpvQuorums::lookup` answers one of: `Verified(key)`, `Held(key, status)` (any other status), `Absent` (not found,
or SPV not started). The status mapping, from the E0-10c doc (§5 step 4, §5.1):

| dash-spv status | Treated as | Why |
|---|---|---|
| `Verified` | `Verified` | the only status where dash-spv checked the commitment against the members it derives (§5 step 4) |
| `Skipped(MissedList)` | `Held` | transient during initial sync; #1150 makes the first quorum after a start `Verified` on arrival |
| `Skipped(NotMarkedForVerification)` | `Held` | a quorum that rotated out before the client's start height (§5.1). Errs strict: never `Verified` without verification |
| `Unknown` | `Held` | not verified yet |
| `Invalid` | refusal | the lookup never returns it (platform-wallet skips invalid entries); if it ever does, SPV has judged the quorum invalid |

A `Held` key is compared with the trusted key (rule 4 does not depend on the status), but it never counts as
SPV-verified. So a lying peer can make Platform reads refuse (availability), and can never make data count as
verified.

**Degraded** (default):

1. `t` = trusted key (may fail: offline, DNS).
2. `Verified(k)`: if `t` is known and `t != k`, refuse (mismatch). Otherwise serve `k`, SPV-verified.
3. `Held(k, _)`: `t` is required; `k != t` refuses; otherwise serve `t` behind the latch, unverified.
4. `Absent`: `t` is required; serve `t` behind the latch, unverified.

**Enforcing**:

1. `Verified(k)`: serve `k`. The trusted service is not consulted (rule 2).
2. Before masternode sync: `t` is required; a `Held(k)` with `k != t` refuses; serve `t` behind the latch, unverified.
3. After masternode sync (rule 3): never the trusted service. Wait up to `unknown_quorum_wait` (default 2 minutes),
   polling SPV once a second for `Verified`; serve it if it comes, otherwise fail closed. A quorum that failed closed
   fails at once for `FAILED_CLOSED_FOR` (30 s), so the SDK's retries on other DAPI nodes (which cite the same quorum)
   do not each wait another 2 minutes. Each waiting lookup polls on its own; a poll is a local storage read. "Rebuild
   the lookup from storage" is not offered by platform-wallet's API at the pin; a follow-up if the wait proves too
   short. If masternode sync completes while a lookup waits for the trusted service, the
   lookup re-checks and applies rule 3 instead of the fallback.

Every SPV lookup is bounded (5 s, or what is left of the rule-3 wait); one that takes longer counts as `Absent`.

**Trusted-only**: serve `t` behind the latch, unverified.

"Masternode sync" is the headers and masternode phases of `SyncTracker` both done (`Synced`, or `WaitForEvents` with
the target reached), fed by `SessionHub::on_progress`. Headers too: after time offline the masternode phase can report
done against a stale tip. It is sticky for the session: once SPV has synced, a later restart of SPV
does not reopen the fallback (rule 3: "once SPV is synced, an unknown quorum never falls back").

**Behind the latch** means `Provenance::fallback_in_use()` returned `Ok` first: the latch is committed and a full WAL
checkpoint made it power-loss durable (DEC-135, DEC-139, DEC-143). On `Err` (a writer holds app.sqlite, a reader
keeps the checkpoint from completing, no provenance attached yet) the lookup fails: the trusted key is not served.
The latch is blocking SQLite work, done under `block_in_place` on the multi-thread engine runtime.

**Cache (rule 1).** `Verified` SPV keys are cached by `(type, hash)`; the key of a mined quorum commitment does not
change, and a cached key is not re-checked within the session. Misses drive the async SPV lookup from the synchronous
`ContextProvider` call (`block_in_place` on the engine's multi-thread runtime, a thread of its own otherwise). The
cache holds at most 256 entries and is cleared when full. Nothing else is cached, except the failed-closed window
above.

**Lifetimes.** The provider holds SPV and provenance weakly: SPV's event handlers reach it through the session hub,
so strong handles would form a cycle that keeps a closed session's storage claimed. Once the session closed, lookups
see SPV as `Absent` and the latch as unavailable, so nothing unverified is served.

**Mismatch (rule 4).** The lookup fails with `ContextProviderError::InvalidQuorum` carrying a fixed marker.
`PlatformError::from(dash_sdk::Error)` maps it to `platform.trust_mismatch` (directly, or wrapped in
`Error::Proof(ContextProviderError)`), and the engine emits `Notice{PlatformTrustMismatch}` once per quorum hash per
session (an engine-side notice code: the frozen bindings do not carry it yet). Other quorum-key failures inside a
proof map to `platform.context_unavailable`, not `proof_invalid`: the proof was not judged.

## 4. Positive provenance: `record_verified`

A fetch counts as SPV-verified when every quorum key its proofs used was `Verified` by SPV (and matched the trusted
key where both were consulted), and it used at least one. The provider cannot see which entity a proof is for, so the
engine runs a fetch it wants to record inside `trust::scoped(fut)`, a tokio task-local that each lookup updates.
A lookup outside a scope (platform-wallet's own sync loops), or in a task the SDK spawned, updates nothing, so a scope
can only ever under-count verification: it fails closed. A nested scope's lookups also count in the enclosing scope.

Errors platform-wallet turns into strings lose the mismatch code: such a refusal surfaces as the library's error,
still a refusal. The mismatch marker is a fixed prefix of `InvalidQuorum` text that only this provider produces.

**Content binding (DEC-183).** A record verifies the content it was written for. `dp_trust_verified.digest` (dw-appdb
migration `trust_digest`) holds `v1:` and the hex SHA-256 of the entity's canonical bytes (`reverify::Canon`: a domain
tag, the encoding version and the kind, then each field; byte strings length-prefixed, integers u64 LE, options
tagged; identity keys in their platform serialization). Readers (`recovery::summaries` through each identity's
`entities`, DP2-01's contacts model) compute the digest of what the library holds now, and `Provenance::is_unverified`
counts the record only when the digests are equal. So an entity the library replaced after it verified, from any
source, reads as unverified, and a replacement served only through the fallback never reads as verified; the
re-fetch verifies new content and its record replaces the old one. Rows written before the column existed have no
digest and verify nothing. Nothing clears records on change: the comparison is the whole mechanism. A change to the
encoding takes the next version, and records of the old one read as unverified until re-verified. Identity keys are
encoded by dpp (bincode), so a dpp release that changes that layout has the same effect.

What a record means: this exact content verified at some time, not that it is current. Content that verified once
and comes back later (a rollback served through the fallback) reads as verified again; DEC-183 accepts that. The
identity's balance is outside provenance (`identity` covers the identity and its keys). Known limit: platform-wallet
stores a sent request without the `autoAcceptProof` its document carries, and does not re-ingest it, so a request
sent from a QR with auto-accept never verifies (fails closed); an upstream fix belongs to platform-wallet.

## 5. Re-fetching unverified entities after sync

Once the latch is set and masternode sync completes, a session task re-fetches what the read models show as
unverified (the kinds with consumers today):

| Kind | Fetch | Recorded when |
|---|---|---|
| `identity` | `Identity::fetch` | it verified and its public keys equal the library's |
| `profile` | the DashPay `profile` document of the identity | it verified and the fields shown equal the library's |
| `dpns_label` | the DPNS `domain` document of the label | it verified and resolves to the identity holding it |
| `contact_request` | the DashPay `contactRequest` documents from sender to recipient (`ownerIdUserId` index), the one with the held `$createdAt` | it verified and every field the library holds is equal (as `parse_contact_request_doc` reads it) |

A verified fetch that differs from what the library holds records nothing: the library's own sync replaces the data,
and the next run records it. The task runs at the sync transition and then every 10 minutes while the session is open
(a run with nothing unverified is a read of the library). It enters the session only to read the library and to
record, not across the network fetches, and stops when the session closes. Records are written under the wallet
manager's read lock, and only for entities the library still holds unchanged and unverified at that point.

The DPNS re-fetch queries the `domain` document (`normalizedParentDomainName = "dash"`, `normalizedLabel = label`) and
takes `records.identity`, as the SDK's `resolve_dpns_name` does, without re-normalizing the already normalized label.

A `contact_request` record is keyed `sender:recipient:$createdAt` (Base58 ids, milliseconds;
`provenance::contact_request_key`, m4 §5), the key DP2-01's contacts read model checks. The task covers the sent,
received and established requests the library holds. `payment` records are left to DP3-04's gate, which re-fetches
through the same `scoped`.

In the degraded mode this works the same way: with #1150 the Platform quorums are `Verified` on first ask, so most
re-fetches record.

## 6. Writes (rule 2)

In the degraded mode writes proceed (rule 5 is suspended). In `Enforcing`, writes must wait for masternode sync.
`TrustProvider::writes_wait()` answers that; wiring it into the write flows (registration, top-up, profile and name
writes) belongs to H-09, when `Enforcing` becomes reachable. A gate at the provider would come too late: the state
transition is already broadcast when its result proof is verified.

## 7. Status

`DashPay::sync_status` is filled from what exists: the bring-up status, the contact-crypto queue, the loops, and
`quorum_source` (§2). `last_pass` stays `None` until a sync-pass report exists. `NetworkSession::quorum_source()` reads
the same value without a wallet.

## 8. Tests (T0, with fake SPV and trusted sources over a real app.sqlite)

| Rule | Test |
|---|---|
| 1 | a `Verified` SPV key is served and cached (one SPV call for two lookups) |
| 2 | Enforcing, before sync: the trusted key is served, the latch is on file, the source is `TrustedFallback` |
| 2 | the fallback is used only before sync: after sync the trusted service is never called |
| 3 | after sync an unknown quorum waits, then fails closed; a retry within the window fails at once; a key that turns `Verified` during the wait is served; the same on a runtime worker |
| 4 | a mismatch refuses in both modes, maps to `platform.trust_mismatch`, and emits one notice per quorum |
| 4 | a forged trusted key with SPV holding the real one refuses every lookup, so every DashPay read refuses |
| latch | a held writer, and a checkpoint a reader blocks, refuse the fallback (nothing served) |
| status map | `Skipped(*)` and `Unknown` count as held (compared, never verified); `Invalid` refuses |
| degraded | SPV `Verified` + equal: served and verified; trusted unreachable + SPV `Verified`: served |
| toggle | trusted-only never consults SPV and reports `Trusted` |
| §4 | a scope records only when every lookup was SPV-verified; no lookup records nothing; nested scopes add up |
| §5 | the post-sync re-fetch records equal, verified entities and skips differing ones |
| §5 | a record needs the entity unchanged at write time; profile documents parse as the library caches them |
| DEC-183 | a replaced entity reads as unverified; a fallback-sourced replacement never verifies; the re-fetch re-verifies new content; every shown field is in the digest; pre-digest rows (migration test) verify nothing |
| §5 | a verified, equal contact request records under DP2-01's key; a differing or absent one records nothing; `contactRequest` documents parse as the library reads them |
| lifetimes | after close nothing unverified is served; a closed session reopens (`session_lifecycle`) |
| §7 | `sync_status().quorum_source` per mode |
