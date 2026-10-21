# MG-04b: v24 shared masternodes

ROADMAP MG-04b (release 1.1, size 2 × L, engine, tier T1 (v24), engine review Y; wave 14 lane E1 part 1, wave 15 part
2). It covers the session protocol for creating a shared masternode (2–8 shares), changing a share's reward address,
rotating the operator or voting key with every owner's approval, dissolving now (alone) or together, and the standby
dissolution, on top of `dw-protx`'s payload codecs. Parity rows QT-126 (creation) and QT-127 (maintenance).

Acceptance:
- a three-party shared masternode is created, its reward address changed, and it is dissolved on regtest;
- every payload, digest and session artefact equals dashd's (dashd v24.0.0-rc.3 is the oracle);
- the wallet never signs as operator (`dw-protx/tests/no_operator_signing.rs` stays green).

Inputs:
- MG-04's owner flows, operations table, fence hand-off and D-rows ([`MG-04-protx-owner.md`](MG-04-protx-owner.md),
  cited "MG-04 D…"), and the contract [`mn-engine.md`](../contracts/mn-engine.md) (Contract-Version 5);
- MG-01's `dw-protx` (its `shared` module holds the type 10–12 codecs, written from rc.2) and the no-operator-signing
  guard; MG-03's `shared_unsupported` and BL-187; MG-05's `MasternodeOwner` scope, `MasternodeOp{max_duffs}` and the
  "Shared masternode" and "Dissolution" flows it reserves for this row (MG-05 D8);
- MG-00's hand-over row for MG-04b and its D31–D36, D41, D46; UX-SPEC §4.25.8;
- MG-07's lesson: a resend or resume uses the first send's fence and reservations, and a row whose transaction may be
  out is never deleted;
- Dash Core **v24.0.0-rc.3** (`0364ed62`). Every "rc.3" citation is `git show v24.0.0-rc.3:<path>` at that tag. The
  three research notes behind this document, with their own citations, are in `/work/scratch/dw-mg-04b/research/`
  (`session-protocol.md`, `consensus.md`, `rpc.md`).

D-numbers here are MG-04b's (D1 and up). Other tasks' rows are cited as "MG-04 D5" and so on.

## 0. Blocker: the dashcore pin (BL-187)

**This row cannot hand a shared-masternode transaction to the network, nor keep syncing once one is mined, at the
desktop's dashcore pin** (`8fe0a381`, patched to PastaPastaPasta/rust-dashcore-dashpay `d3d50520a`, D1):
- a shared registration is a **version-3** ProRegTx with a share table (rc.3 `src/evo/providertx.h:162-227`); the
  pin encodes and decodes ProRegTx at version ≤ 2 only (MG-04 D1);
- ProDisTx, ProUpShareTx and ProUpSharedRegTx are special types **10, 11 and 12** (rc.3
  `src/primitives/transaction.h:39-41`); the pin's `TransactionType::try_from` refuses them
  (`UnknownSpecialTransactionType`), and `TransactionPayload` has no raw variant;
- every hand-off goes through `PreparedTx::assembled(tx: dashcore::Transaction, …)` and dash-spv broadcasts a typed
  `Transaction`, so none of these can be sent;
- a block holding one of them can't be decoded, and the wallet's filters match every one of them (its inputs fund the
  registration, its refund script is paid by a dissolution, its share's ProTx hash is in every update). dash-spv then
  re-requests the block forever and the wallet stops scanning at that height, as MG-04 D48 saw for a version-3
  ProUpServTx.

Upstream `dashpay/rust-dashcore` `dev` has all of it: unknown special types decode as raw payloads (#1136,
3ae5b48e4), ProUpServTx/ProUpRegTx version 3 (#1138, 0d215f3f4), ProRegTx version 3 with `SharedCollateral` (#1152,
d775b1bf8) and types 10–12 (#1156, 2102cacf2). BL-187 already asks for that pin move (#1138/#1152/#1156); this row
adds that **#1136 is a prerequisite**: cherry-picking #1138 onto `d3d50520a` conflicts in
`special_transaction/mod.rs` on #1136's `UnknownPayloadType` and the `len()`→`size()` rename (probe, 10-21, scratch
worktree removed). The backport is four `feat(dash)!` changes to dashcore's public types, which platform's crates
compile against too, so it is a graph-wide `[patch]` under DEC-09, the desktop pin owner's (manager to staff).

Everything below the hand-off can be built and proven now: the codecs, the session protocol and its envelopes, the
consent and maintenance digests, the vault signers, the engine's sessions, persistence and refusals, and oracle
equality with dashd's RPCs for every payload, digest and signature (dashd builds them; the wallet's bytes are
compared, not broadcast). The regtest acceptance and the hand-off wait for BL-187 (§13, D1).

| Part | Where |
|---|---|
| Codecs: ProRegTx v3 shared, `ExtNetInfo`, the consent digest, the share rules, the reward and dissolution split, canonical signatures | `rust/crates/dw-protx/src/shared.rs` (extended), `shared/netinfo.rs`, `shared/rules.rs` |
| The session protocol: envelopes, JSON fidelity, fingerprint, merge rules, the funding template, `payloadMatchesEnvelope` | `rust/crates/dw-protx/src/shared/{json,envelope,session}.rs` (pure, no I/O) |
| Owner-key signers (the vault hashes) | `rust/crates/dw-vault/src/owner.rs` |
| Facade calls, records, errors | `rust/crates/dw-engine/src/masternodes/{shared,shared_records}.rs` |
| Sessions, maintenance builders, hand-off | `rust/crates/dw-engine/src/masternodes/shared/{session,build,maint,ops}.rs` |
| Persistence | `rust/crates/dw-appdb/migrations/V2026102201__mg_shared_sessions.sql`, `dw-appdb/src/shared_sessions.rs` |
| dwcli | `rust/bin/dwcli/src/masternodes.rs`: `mn shared …` |
| Oracle vectors | `testdata/oracle/dashd/gen_shared_vectors.py`, `testdata/shared_*.json`, `testdata/oracle/univalue/` (D7) |
| Regtest | `regtest/functional/dwd_shared.py` (on `dwd_protx.py`'s harness) |

## 1. What Core does, and what the wallet does

Core has two layers: RPCs over `evo::provider` (rc.3 `src/evo/providertx_service.cpp:1000-1371`, `src/rpc/evo.cpp`)
and dash-qt's wizard and maintenance dialogs, which exchange text messages by clipboard or file (rc.3
`src/qt/mnsharesession.{h,cpp}`, `src/qt/sharedmncreatedialog.cpp`, `src/qt/sharedmndialogs.cpp`,
`src/qt/sharedmnwidgets.cpp`). The wallet speaks dash-qt's messages byte for byte, so a desktop party and a dash-qt
party can be in one session, and builds every transaction as Core's service layer does.

| Step | Core (rc.3) | Wallet |
|---|---|---|
| Activation | v24 gates everything; a shared registration is payload version 3 (`evo/types.h:23-35`, `specialtxman.cpp:1105-1121`). Shared evonodes also need `evo_shares` (`specialtxman.cpp:1163-1170`), which no masternode signs at rc.3 (`llmq/ehf_signals.cpp:53-55`); mainnet has both `NEVER_ACTIVE` (`chainparams.cpp:213,222`) | the engine reports v24 (D3); regular masternodes only (D4) |
| Shares | 2–8; each ≥ 100 DASH and ≤ the collateral; they sum to exactly 1,000 DASH; owner keys unique and non-null; refund scripts unique; refund and reward P2PKH or P2SH, never paying the voting key or any owner key (`providertx.cpp:63-130`) | the same rules, with Core's reject reasons (`dw-protx::shared::rules`), before anything is signed |
| Exit terms | `nEarlyPeriodBlocks` ≤ 420,480; `0 ≤ nEarlyPenalty <` the smallest share; penalty 0 without a period (`providertx.cpp:63-130`) | the same; the zero-penalty warning text is Core's (`providertx_service.cpp:618-619`) |
| Funding | the coordinator's wizard builds a funding template from every party's inputs and change (`mnsharesession.cpp:129-158,802-813`); `shared_register_prepare` appends the 1,000 DASH template output last (`providertx_service.cpp:1011-1088`) | the same template and assembly, byte for byte (§2.4) |
| Fee | paid by the coordinator: its contribution is its share plus the fee it types (default 100,000 duffs); a participant contributes exactly its share (`sharedmncreatedialog.cpp:81,3010-3015`) | the same (D9) |
| Consent | each share owner signs `MakeSharedRegConsentHash` (`providertx.cpp:335-373`): every prevout and sequence, the lock time, every output, the share table, the terms | the same digest, computed by the vault from the transaction it is shown (D11) |
| Signing order | join signatures first, then each party signs its own inputs `SIGHASH_ALL` (`providertx_service.cpp:1195-1199`) | the same |
| Broadcast | only the coordinator's wizard offers it (`sharedmncreatedialog.cpp:2670-2691,2908-2912`) | the same: the coordinator sends (D13) |
| Reward change | `shared_update_share`: one owner, a ProUpShareTx signed over `SerializeHash(payload)`, fee inputs from the wallet (`providertx_service.cpp:1298-1334`) | `update_share` (§7), MG-04's operation path |
| Key rotation | `shared_update_registrar_prepare` → `shared_sign` by every owner → `shared_combine` on the funding wallet (`providertx_service.cpp:1336-1371,1209-1237`) | the same roles, over dash-qt's `dash-shared-mn-sigs` envelope (§2.5) |
| Dissolve now | `shared_dissolve`: one input (the collateral, empty scriptSig), non-actors refunded in share order with their pro-rata part of the penalty, the actor last (omitted at 0), fee ≤ 1,000,000 duffs from the actor's share, signed `MakeSignHash(tx, 1)` (`providertx_service.cpp:639-681,1242-1279`) | the same builder and digest (§5.4) |
| Dissolve together | `shared_dissolve_prepare` (penalty 0) → `shared_sign` by every owner → `shared_combine` (`providertx_service.cpp:1281-1296`) | the same, over the sigs envelope |
| Standby | two signed unilateral dissolutions in a text file, penalty-free and immediate (`sharedmndialogs.cpp:1634-1741`) | the same pair, stored and exported in Core's file format (§9.4) |
| Time locks | `shared_sign` refuses a registration or dissolution with an unsatisfied lock time or a relative lock (`providertx_service.cpp:683-705,1100-1109`) | refused always (D16) |

The wallet never signs as operator. A new operator key in a key rotation is a BLS **public** key, given or the
keychain's (MG-04 D14). No step reads an operator secret, and the guard keeps scanning the workspace.

## 2. The session protocol (dash-qt's, mirrored)

### 2.1 Three message kinds

| Kind | Wire form | Producer | Purpose |
|---|---|---|---|
| Session envelope | JSON object, `"type": "dash-shared-mn-session"`, `"version": 1` (`mnsharesession.cpp:35-36,276-349`) | the creation session, both roles | the shared registration |
| Sigs envelope | JSON object, `"type": "dash-shared-mn-sigs"`, `"version": 1`, `"kind": "dissolve"` or `"registrar"` (`sharedmndialogs.cpp:64-65,653-665`) | dissolve together, key rotation | collecting every owner's signature |
| Standby | bare hex of one signed ProDisTx; the standby file holds two (`sharedmndialogs.cpp:1652-1741`) | the standby | a unilateral exit kept for later |

One import call routes all three, as `SharedMnImport::Detect` does (`sharedmnwidgets.cpp:354-399`): trimmed text that
parses as a JSON object with one of the two `type`s is an envelope; any other JSON object is refused; otherwise text
that is hex of a type-10 transaction is a standby; anything else is refused. Input over 2 MiB (UTF-8 bytes) is
refused before parsing (`sharedmnwidgets.h:158`).

### 2.2 JSON fidelity and the message code

Both envelopes are written by UniValue's `write(2)`: keys in insertion order, two-space indent, `"key": value`, an
empty array as `[`, newline, indent, `]`, no trailing newline, UniValue's escape table (control bytes as `\u00XX`
except `\b \t \n \f \r`, `"` and `\`, `0x7f` as `\u007f`, everything else raw), integers in plain decimal
(`univalue/lib/univalue_write.cpp:12-113`, `univalue_escapes.h`).

The **message code** shown to users (UX-SPEC's "check code") is `EnvelopeFingerprint`: single SHA-256 of that
rendering of the object without its `fingerprint` key, first four bytes as upper-case hex, `XXXX-XXXX`; the
producer appends it as the last key (`mnsharesession.cpp:46-57,225-237`). A receiver recomputes it over its own parse
of the received text, re-rendered: unknown keys in received order and every number as its received token. A mismatch
only warns ("This message was edited after it was copied."); a missing code is silent (`mnsharesession.cpp:239-249`).

The **session code** is the first six characters of `sessionId` (64 lower-case hex, `GetRandHash().ToString()`),
upper-cased (`mnsharesession.h:163-166`, `mnsharesession.cpp:251-255`). UX-SPEC's 6-hex session code and
`XXXX-XXXX` check code are these two.

`dw-protx::shared::json` is a small JSON reader and writer for this (D7): an ordered object model that keeps
duplicate keys (lookup takes the first, as UniValue's `find_value`), keeps each number's token text, reads integers
as UniValue's `getInt` does (`std::from_chars`: optional `-`, digits only), and writes UniValue's rendering exactly.

### 2.3 The session envelope, field by field

Key order is the serializer's (`mnsharesession.cpp:276-349`); every message is the whole session, not a delta.

| Key | Type | Value | Reader rule (`fromJson`, `mnsharesession.cpp:371-664`) |
|---|---|---|---|
| `type` | string | `dash-shared-mn-session` | exact |
| `version` | int | `1` | `== 1` |
| `network` | string | Core's `NetworkIDString`: `main`, `test`, `devnet`, `regtest` | equals the wallet's network (UX: "This invitation is for Mainnet…", MG-00 D36) |
| `sessionId` | string | 64 lower-case hex | non-empty; the wallet's own are 64 hex, a foreign one is kept as given |
| `revision` | int | starts at 1 | ≥ 1 |
| `stage` | string | `draft`, `frozen`, `signing`, `combined`, `fundingSigned`, `broadcast` | one of these |
| `fundingTx` | string | hex of the funding template (§2.4), `""` before a contribution | not trusted: rebuilt from `contributions` (D8) |
| `shares[]` | objects | `{label, amount, ownerAddress, refundAddress, rewardAddress}` in share order; `""` for a field not yet given | ≤ 8; `amount` in money range; owner and refund required (may be `""`) |
| `terms` | object | `{coreP2PAddrs, operatorPubKey, votingAddress, operatorReward, earlyPeriodBlocks, earlyPenalty}`; `coreP2PAddrs` is `ip:port` entries joined by `,`; `operatorReward` in basis points | members optional; compared as exact strings and integers (`sameTerms`, `mnsharesession.cpp:1098-1106`) |
| `contributions[]` | objects | `{label, inputs[{txid, vout, sequence}], change{address, amount}?, changeIndex}`, sorted by share index | ≤ 8 entries, ≤ 64 inputs each; a contribution belongs to the share whose `label` it carries |
| `protx` | string | `""` (draft) or the shared ProRegTx hex | at `frozen`+: decodes, its consent digest equals `consentHash`, and `payloadMatchesEnvelope` holds (§2.4) |
| `consentHash` | string | `""` or the digest in display order (byte-reversed hex) | 64 hex or empty |
| `collateralIndex` | int | `-1` or the collateral output | equals the number of change outputs |
| `sigs[]` | objects | `[{shareIndex, signature}]`, ascending; base64 (padded) of 65 bytes | at `combined`+ every share has one and each verifies |
| `prepareWallet` | string | the coordinator's wallet **name** in dash-qt | the wallet writes `""` (D14) |
| `operatorSecretHolder` | string | free text "node run by" | carried |
| `coordinatorLabel` | string | the coordinator's share label | carried |
| `fingerprint` | string | §2.2 | warns on mismatch |

Unknown keys are dropped by dash-qt and change its displayed code (H-12 in the research note); the wallet writes none.

Message names by stage (`sharedmncreatedialog.cpp:2693-2709`), which MG-00 D31's user-facing stages map onto:

| `stage` | Coordinator sends | Participant sends | MG-00 D31 stage |
|---|---|---|---|
| `draft` | Invitation | Details | Invite, Collect details |
| `frozen`, `signing` | Locked Terms | Approval | Lock terms, Approvals |
| `combined` | Signing Request | Signed Contribution | Signatures |
| `fundingSigned` | Signed Contribution | Signed Contribution | Signatures (complete) |
| `broadcast` | Final Record | Final Record | Send |

### 2.4 The funding template and the registration

`fundingTx` (`mnsharesession.cpp:129-158,802-813`): `nVersion 2`, type 0, lock time 0; inputs are every
contribution's inputs, contributions in share order and each in its own order; outputs are each contribution's change
(when it has one), in the same order. No collateral output.

The shared ProRegTx (`providertx_service.cpp:1011-1088`): the funding template with `nVersion 3`, `nType 1`, the
1,000 DASH output to `04445348437551` appended **last** (`collateralOutpoint = (null, vout.len() − 1)`), and the
version-3 payload:

```
u16 nVersion = 3 | u16 nType = 0 | u16 nMode = 0 | [32] null | u32 collateralIndex | ExtNetInfo
| [20] keyIDOwner = 0 | [48] pubKeyOperator (basic) | [20] keyIDVoting | u16 nOperatorReward
| u8 payouts = 0 | u8 n | n × CCollateralShare | n × [65] join signatures (no count byte)
| u32 nEarlyPeriodBlocks | i64 nEarlyPenalty | [32] inputsHash | cs vchSig = 0x00
```

(`providertx.h:162-227`; `CCollateralShare` = `i64 amount | script refund | script reward | [20] keyIDOwner`,
`:74-77`, an empty reward meaning "the refund"; `ExtNetInfo` = `u8 1 | cs purposes | per purpose u8 id, cs n, n
entries`, a service entry `0x01 | ADDRv2 | u16 big-endian port`, `netinfo.h:194-263,411-435`). Join signatures are
65 zero bytes until combined.

`payloadMatchesEnvelope` (`mnsharesession.cpp:982-1086`) is the check every party runs at `frozen` and after: the
payload's shares equal the envelope's rows (owner key id, amount, refund script, reward script or empty); the terms
equal (voting key id, operator key case-insensitively, reward, period, penalty, `coreP2PAddrs` parsed into the same
net info); `collateralIndex` equals the number of change outputs and the payload's index; the type is regular; the
inputs (prevout, sequence) and outputs (value, script) are exactly the template's plus the collateral; lock time 0.
The wallet runs it and adds what dash-qt leaves to consensus (D12): `tx.nVersion 3`, payload version 3, `inputsHash`,
and, at `combined`, every join signature verified against the consent digest.

### 2.5 The sigs envelope

```
{ "type": "dash-shared-mn-sigs", "version": 1, "network": …, "kind": "dissolve" | "registrar",
  "proTxHash": …, "tx": <hex>, "signatures": [{shareIndex, signature}] (ascending), "fingerprint": … }
```

(`sharedmndialogs.cpp:653-665`.) A reader checks the network, the kind, the ProTx hash (case-insensitive), adopts a
non-empty `tx` only if it is the one it holds (or it holds none), and verifies each signature against the digest it
recomputes (`sharedmndialogs.cpp:253-309,559-620,683-730`): for `dissolve`, `MakeSignHash(tx, n)` of a template that
returns every non-actor exactly its principal and pays at most the actor's refund (`SharedMnReadDissolution`,
`sharedmnwidgets.cpp:272-352`); for `registrar`, `SerializeHash(payload)`. dash-qt also takes a bare JSON array as
signatures for the transaction it holds; the wallet accepts that too.

## 3. State machines

### 3.1 Creation: coordinator

```
start ─▶ inviting(draft) ─▶ collecting(draft) ─lock+consent─▶ approving(signing) ─all approvals: combine─▶
signing(combined) ─all funding sigs: sign own + save─▶ ready(fundingSigned) ─send─▶ sent(broadcast) ─▶ confirmed
                                    ◀─unlock (discards approvals)─┘
any state before `ready` ──discard──▶ (deleted, coins unlocked)
```

1. `start_shared` (§7): checks the request (§5), picks this wallet's keys (D10), reserves its coins (share + fee,
   D9) with persistent locks, writes the session (`draft`, revision 1, the coordinator's row filled, its contribution
   added), and returns the Invitation.
2. `import_shared_message` with a Details reply: absorbed as `absorbDraftReply` does (`mnsharesession.cpp:1204-1284`):
   same session, same terms (exact), same labels and amounts; empty row fields filled, new contributions added and
   sorted by share index, `revision` = ours + 1. Stricter (D15): a reply fills at most one row, the row its one new
   contribution names.
3. `lock_shared_terms` (grant): every row has owner and refund and a contribution; the fee is at least the relay
   minimum for the final size and positive (`sharedmncreatedialog.cpp:3294-3324`); builds the ProRegTx (§2.4), runs
   §5's rules and `payloadMatchesEnvelope`, then signs this wallet's consent (§4) and moves to `signing` with
   `revision` unchanged (`freeze` keeps it, `mnsharesession.cpp:885-923`).
4. Approvals, in any order (`mergeEnvelope`, `mnsharesession.cpp:1353-1495`): same session and consent digest, each
   signature verified as it is added. The wallet refuses an import whose `revision` is higher while its own stage is
   past `draft` (D15; dash-qt would replace its locked session with it, H-1). When every share has signed, the
   wallet combines (join signatures into the payload) and moves to `combined`: the Signing Request.
5. Signed contributions, in any order: per input, the non-empty scriptSig is taken; two different ones conflict; the
   rest of the transaction must be identical with scriptSigs cleared (`mnsharesession.cpp:1424-1463`). When only the
   coordinator's inputs are unsigned, `send_shared_registration` (grant) signs them, saves the bytes durably, then
   hands them to the fence (§6.3) and moves to `broadcast`: the Final Record.
6. The follower marks the session `confirmed` once the ProRegTx is in a block of the wallet's history, and
   `conflicted` once another transaction in a block spends one of its inputs.

`unlock_shared_terms` returns a `frozen`/`signing` session to `draft` (revision + 1, approvals discarded,
`mnsharesession.cpp:925-935`). Not past `signing`: once combined, participants may have signed inputs.

### 3.2 Creation: participant

```
import invitation ─▶ invited(draft) ─join: reserve─▶ joined(draft) ─import locked terms─▶ to_approve(signing)
─approve─▶ approved(signing) ─import signing request─▶ to_sign(combined) ─sign funding─▶ signed(combined/fundingSigned)
─import final record or see the ProRegTx─▶ confirmed
invited / joined / to_approve / approved ──discard──▶ (deleted, coins unlocked)
```

1. The first import of an Invitation creates a session in `invited` (role participant; no coin is touched). The wallet
   infers its share as dash-qt does (`inferMyShare`, `sharedmncreatedialog.cpp:3326-3348`): the row whose owner key
   it holds, else the single empty row, else the request must name one.
2. `join_shared`: fills its row (owner key, refund, reward: D10), reserves exactly its share (D9), writes the session,
   and returns the Details: the invitation with its row and contribution, `revision` + 1 (as `addContribution`
   does).
3. Import of Locked Terms (`adoptLockedTerms`, `mnsharesession.cpp:1286-1351`): same session, terms, labels and
   amounts; its own row and contribution exactly as it sent them; then the session is replaced by the locked copy.
4. `approve_shared` (grant): §4's checks, then a consent signature for each share whose owner key this wallet holds;
   returns the Approval (the locked copy plus its signature, `revision` unchanged: H-1).
5. Import of the Signing Request (`combined`): every join signature verified (D12); `sign_shared_funding` (grant)
   signs this wallet's inputs (SIGHASH_ALL), checks that only they changed (`signedOnlyOwnInputs`,
   `sharedmncreatedialog.cpp:2448-2466`), **saves the signed copy durably and marks it may-be-out before returning
   it** (D17), and returns the Signed Contribution.
6. From then on anyone holding a complete copy can broadcast it (H-9): the session is never deleted and its coins stay
   locked until it reads `confirmed` or `conflicted` (D17).

### 3.3 Maintenance

| Flow | Parties | States |
|---|---|---|
| Change my reward address (ProUpShareTx) | this wallet | MG-04's operation states: `signed`, `not_sent`, `sent`, `confirmed`, `conflicted` (kind `update_share`) |
| Dissolve now (unilateral ProDisTx) | this wallet | the same (kind `dissolve`), no fee inputs |
| Standby (two unilateral ProDisTx, not sent) | this wallet | `stored` → (sent through `send_standby` as a `dissolve` operation) |
| Dissolve together (unanimous ProDisTx) | every owner; any can send | `collecting` → `complete` → `sent` → `confirmed` / `conflicted` |
| Change keys together (ProUpSharedRegTx) | every owner; the funding wallet sends | `collecting` → `complete` → `sent` → `confirmed` / `conflicted` |

The two multi-party flows are maintenance sessions keyed by their transaction: a session is created by the wallet
that prepares it (`start_dissolve_together`, `start_key_change`) or by the first import of a sigs envelope. Each
import adds verified signatures; `approve_maintenance` (grant) adds this wallet's; `send_maintenance` hands off a
complete one: a dissolution from any wallet (`CombineShared` needs none, `providertx_service.cpp:1209-1220`), a key
change only from the wallet whose inputs fund it, which re-signs them after the payload changed
(`providertx_service.cpp:1221-1237`). A session whose input another transaction spent in a block is dead
(`sharedmndialogs.cpp:947-971`). Both flows need the masternode one block deep (`sharedmndialogs.cpp:1112-1122`) and
in the synced list.

## 4. Consent: what each party signs, and what it checks first

Every share-owner signature is Core's `SignCompact` of a raw 32-byte digest, no message prefix, verified by
`VerifyHashCanonical`: 65 bytes, header 27–34, low S, the recovered key's id equal to the share's `keyIDOwner`
(`messagesigner.cpp:48-107`). The vault computes each digest itself from the transaction or payload it is handed
(D11), and the engine checks the signature it gets back (canonical, recovers to the share's key) before using it.

| Signature | Digest (rc.3) | Signed by | The wallet checks first |
|---|---|---|---|
| Registration consent (join signature) | `MakeSharedRegConsentHash(tx)`: `cs "DashSharedMNReg"`, payload version, tx version, tx type, lock time, `CalcTxInputsHash`, every input's sequence, `CalcTxOutputsHash`, MN type, mode, net info, (evonode: platform node id), voting key id, operator key, operator reward, `u8 n`, the shares, period, penalty (`providertx.cpp:335-373`) | each share owner whose key this wallet holds | the transaction decodes and is the registration of §2.4; `payloadMatchesEnvelope` and D12; its own row (amount, owner, refund, reward) and contribution exactly as it gave them; no other input is its coin (`foreignWalletInputs`, `sharedmncreatedialog.cpp:2433-2446`); its net debit (own inputs minus outputs paying it) is at most its share plus, for the coordinator, the fee it set (`ownContributionError`, `:2475-2513`); §5's share rules; lock time 0 and every sequence final (D16); the digest recomputed equals `consentHash` |
| Funding inputs | `SIGHASH_ALL` over the combined transaction | each party, its own inputs | the consent checks again on the combined transaction; every join signature present and valid (D12); only its own inputs gain scriptSigs |
| ProUpShareTx | `SerializeHash(payload)` without `vchSig`: version, ProTx hash, share index, reward script, `inputsHash` (`providertx.h:497-503`) | the share's owner | the share index is this wallet's share (its owner key); the reward is P2PKH or P2SH, not the voting key's or any owner key's address (`bad-proupshare-payee-reuse`, `specialtxman.cpp:1563-1617`); `inputsHash` is the transaction's |
| ProDisTx unilateral | `MakeSignHash(tx, 1)`: `cs "DashSharedMNDissolve"`, payload version, tx version, tx type, lock time, prevouts, sequences, outputs, ProTx hash, actor index, `u8 1` (`providertx.cpp:506-533`) | the actor's owner | the transaction is Core's builder output (§5.4) for this masternode, actor, penalty and fee, recomputed by the wallet from the share table and compared byte for byte; fee in (0, 1,000,000] |
| ProDisTx unanimous | `MakeSignHash(tx, n)` | every owner | `SharedMnReadDissolution`'s rule: one input (the collateral), every non-actor gets exactly its principal at its refund, at most one more output, to the actor's refund, fee in (0, 1,000,000] (`sharedmnwidgets.cpp:272-352`); lock time 0, sequence final (D16) |
| ProUpSharedRegTx | `SerializeHash(payload)` without the count and signatures: version, ProTx hash, operator key (basic), voting key id, `inputsHash` (`providertx.h:531-545`) | every owner | the masternode is listed and shared; the new voting key's address is no share's refund or reward (`bad-proupsharedreg-payee-reuse`); the new operator key is in no other listed masternode (`bad-protx-dup-key`); the user saw the operator and voting keys it sets (the preview, MG-04 D41) |

What consent does **not** give: a consent signature moves no coin. A party's coins move only with its own funding
signature, which it makes only on the combined transaction it checked. A unanimous dissolution or key change needs
every owner; a unilateral dissolution moves at most the actor's own share to the others (Core caps the bonuses at the
penalty and the fee at 1,000,000 duffs, `specialtxman.cpp:1758-1767`).

## 5. Collateral and share accounting

- **Shares:** 2–8 rows; amounts in duffs, each ≥ 100 DASH and ≤ 1,000 DASH, summing to exactly 1,000 DASH (regular
  only, D4). A coordinator sets labels and amounts; each party fills its own owner, refund and reward.
- **Rules** (`dw-protx::shared::rules`, Core's order and reasons, `providertx.cpp:63-130,265-333`):
  `bad-protx-shares-count`, `-early-period`, `-amount`, `-key-null`, `-dup-key`, `-dup-refund`, `-payee`,
  `-payee-dest`, `-payee-reuse`, `-amount-sum`, `-penalty`; and `bad-protx-shares-external`, `-owner-key`,
  `-payouts`, `bad-protx-key-null`, `bad-protx-operator-reward`, `bad-protx-netinfo-*`. Against the synced list and
  the wallet's operations: the service, the operator key under either scheme (`bad-protx-dup-key`, MG-04 D30), and
  the wallet's own owner keys (MG-04 D13). Owner keys of other parties are checked by Core only (SPV has no owner keys,
  MG-04 D5).
- **Early period and penalty:** blocks, 0..420,480 (UX presets 0, 52,560, 105,120, 210,240, 420,480,
  `sharedmncreatedialog.cpp:628-631`); `0 ≤ penalty <` the smallest share, and 0 without a period. A zero penalty
  carries Core's warning text.
- **Reward split** (for display; Core pays it): the masternode reward less the operator's share once its payout
  address is set, then `SplitAmountByShares`: each share `floor(total × amount / 1,000 DASH)` with 128-bit
  intermediates, the remainder to the last; a zero output omitted (`payments.cpp:162-196`,
  `providertx_util.cpp:88-115`). MG-00 D46's two figures (now and once the operator payout is set) come from this.
- **Funding:** the coordinator contributes its share plus the network fee (default 100,000 duffs, editable, D9);
  each participant contributes exactly its share. Each party selects largest-first from its own spendable,
  unlocked, confirmed coins until it covers the target with change 0 or above dust; change below dust goes to the fee
  (`sharedmncreatedialog.cpp:3017-3072`); change goes to a fresh internal address of the wallet (D10).
- **Dissolution** (`BuildDissolution`, `providertx_service.cpp:639-681`): one input, the collateral, empty scriptSig,
  sequence `0xffffffff`, lock time 0, `nVersion 3`, type 10. Outputs: each non-actor in share order, `amount + bonus`
  to its refund script, where the bonuses are `SplitAmountByShares(penalty, non-actors)`; then the actor's `amount −
  penalty − fee` to its refund script, omitted when 0. `penalty` is `nEarlyPenalty` when the dissolution is
  unilateral and early (`height + 1 − registered < period`), else 0; a standby's variants fix it either way. A
  together dissolution has penalty 0 and the chosen actor pays the fee.
- **Validity over time:** a ProDisTx valid at some height is valid at every later one (`specialtxman.cpp:1755-1758`;
  release notes), so a penalty-free standby becomes valid at `registered + period` and stays valid; it is spent only
  if the masternode is still there.

## 6. Keys, grants and leases

### 6.1 Keys per party (D10)

- **Share owner key:** a keychain owner key `m/9'/coin'/3'/2'/i`, the lowest index 0–19 free by MG-04 D13 (shared
  sessions count as in use); P2PKH. One share per wallet in a creation session it starts or joins; a wallet that holds
  several shares of a masternode (another device of the same seed, or a dash-qt wallet's import) signs for each.
- **Refund address:** a fresh BIP44 receive address of the wallet: a funds account, so a refund lands in the
  balance (MG-04 D55), never an owner or voting key's address.
- **Reward address:** none by default (the refund receives rewards, Core's empty `scriptReward`), or a fresh receive
  address, or a given P2PKH/P2SH address.
- **Voting key** (coordinator, MG-00 D34): the keychain voting key by MG-04 D13, or a given address; never an owner
  key's address (MG-04 D52).
- **Operator key:** given public key or the keychain's operator public key (MG-04 D14).

### 6.2 Vault signers (D11)

New `VaultSigner` methods, each reachable only under `SignerScope::MasternodeOwner` with `KeyUse::OwnerPayload` at a
keychain owner path (the attached-owner-key path stays ProUpRegTx-only):
- `sign_shared_consent(index, tx_bytes)`: decodes the shared ProRegTx and signs `MakeSharedRegConsentHash`;
- `sign_update_share(index, payload)`: `SerializeHash(payload)` of a ProUpShareTx;
- `sign_update_shared_registrar(index, payload)`: `SerializeHash(payload)` of a ProUpSharedRegTx;
- `sign_dissolution(index, tx_bytes, sig_count)`: `MakeSignHash(tx, sig_count)` of a ProDisTx, `sig_count` 1 or the
  share count of a payload it decodes, nothing else.

None takes a digest. An owner key still signs no transaction input and no message (MG-04 D50). The scope's
`max_duffs` charge stays the engine's (MG-05 §2).

### 6.3 Grants, leases, charges

Every signing call takes one `MasternodeOp{max_duffs}` grant for the acting wallet and begins its own lease
(MG-05 §4): `FlowKind::SharedMasternode` ("Shared masternode") for creation, `update_share` and the key change;
`FlowKind::Dissolution` ("Dissolution") for dissolve now, standby and together (MG-05 D8, MG-00 D35). The lease's
`masternode` budget is charged before anything is signed (D18):

| Call | Charged |
|---|---|
| `lock_shared_terms`, `approve_shared` (consent only) | 0: no coin of the wallet can move without its funding signature |
| `sign_shared_funding`, `send_shared_registration` | the wallet's net debit of the registration: its inputs minus outputs paying it (its share; plus the fee for the coordinator) |
| `update_share`, `send_maintenance` of a key change, a resend | the transaction's outflow (MG-05 D4): the fee |
| `dissolve`, `create_standby`, `send_standby`, `approve_maintenance` of a dissolution | what the wallet's shares give up: penalty + fee when it is the actor, else 0 |

The fence (E0-04 §5, MG-04 §2): the coordinator's registration send, `update_share`, `dissolve`, `send_standby` and a
complete maintenance send are row-less `CoreTx` artifacts through `PreparedTx::assembled` and `TxDraft::broadcast`,
the row saved durably first and `sent_at` set before each hand-off (MG-04 D6, D7). A resend reserves the same inputs
again and runs under its own lease (MG-07). Inputs that aren't the wallet's coins (other parties' contributions, the
template collateral) are reserved under the same owner too: harmless, and it keeps one path.

## 7. Facade

All on `Masternodes`. Kind as in mn-engine §2.

| Call | Kind | Grant | Does |
|---|---|---|---|
| `shared_capability(wallet)` | async | — | `SharedCapability`: v24 `active` / `inactive` / `unknown`, evonode shares always `inactive` (D3, D4), and whether this build can send them (`false` until BL-187, D1). |
| `shared_sessions(wallet)` | async | — | Every `SharedSession` of the wallet (creation and maintenance), newest first, state derived on read. |
| `shared_session(wallet, session)` | async | — | One `SharedSession`. |
| `start_shared(wallet, request)` | async | — | Coordinator: checks `StartSharedRequest`, picks keys, reserves share + fee, saves the draft. Returns the session and its Invitation (`SharedMessage`). |
| `import_shared_message(wallet, text)` | async | — | Detects and applies a pasted or opened message (§2.1): creates or advances a creation session, a maintenance session, or describes a standby. Returns `SharedImport` (the session or standby, what changed, the received code and any warning). Signs nothing. |
| `join_shared(wallet, session, request)` | async | — | Participant: fills its row, reserves its share, returns the Details. |
| `shared_message(wallet, session)` | async | — | The message to copy or save now: text, code, Core's file name (`shared-mn-session-<CODE>.json`). |
| `lock_shared_terms(wallet, session, grant)` | async | `MasternodeOp` | Coordinator: builds the registration, signs its consent, returns Locked Terms. |
| `unlock_shared_terms(wallet, session)` | async | — | Coordinator: back to draft before `combined`. |
| `approve_shared(wallet, session, grant)` | async | `MasternodeOp` | Participant: §4's checks, consent signatures, returns the Approval. |
| `sign_shared_funding(wallet, session, grant)` | async | `MasternodeOp` | Participant: signs its inputs on the combined transaction, saves it as may-be-out, returns the Signed Contribution. |
| `send_shared_registration(wallet, session, grant)` | async | `MasternodeOp` | Coordinator: signs its inputs once every other input is signed, saves, hands off; `resend` the same way. |
| `discard_shared_session(wallet, session)` | async | — | Deletes a session nothing of which may be out (D17) and unlocks its coins. |
| `preview_update_share(wallet, request)` / `update_share(wallet, request, grant)` | async | `MasternodeOp` | ProUpShareTx for one of this wallet's shares; a `ProTxOperation` of kind `update_share`. |
| `preview_dissolve(wallet, request)` / `dissolve(wallet, request, grant)` | async | `MasternodeOp` | Unilateral ProDisTx now: `DissolvePreview` (each share's return, penalty, fee, early, valid from); the operation (kind `dissolve`). |
| `create_standby(wallet, pro_tx_hash, fee, grant)` | async | `MasternodeOp` | Signs both standby variants for each of this wallet's shares and stores them. |
| `standby(wallet, pro_tx_hash)` / `export_standby(wallet, pro_tx_hash, share)` | async | — | The stored standby; Core's file text and name (§9.4). |
| `send_standby(wallet, pro_tx_hash, share, variant, grant)` | async | `MasternodeOp` | Hands a stored variant to the fence as a `dissolve` operation. |
| `start_dissolve_together(wallet, request, grant)` | async | `MasternodeOp` | Builds the unanimous ProDisTx, signs this wallet's shares, returns the session and its sigs envelope. |
| `start_key_change(wallet, request, grant)` | async | `MasternodeOp` | Builds and funds the ProUpSharedRegTx (operator and voting keys kept or set), signs this wallet's shares, returns the session and envelope. |
| `approve_maintenance(wallet, session, grant)` | async | `MasternodeOp` | Signs this wallet's shares of an imported maintenance session. |
| `send_maintenance(wallet, session, grant)` | async | `MasternodeOp` | Inserts every signature in share order and hands off (a key change re-signs its fee inputs). |
| `operations`, `resend_operation`, `discard_operation` | | | MG-04's, now also over `update_share` and `dissolve` rows |

Refusals, in order, as MG-04 §2.3: arguments (`invalid_argument`; a session id is 64 lower-case hex for a creation
session or 32 for a maintenance one), `wallet_not_found`, `masternodes.unsupported{feature: "shared_masternodes"}`
(v24 not active, or not sendable at this pin for a hand-off), the list (`list_unavailable`), the session
(`shared_session_not_found`, `in_flight`, `shared_stage{stage}`), the message (`shared_message{reason}`), Core's rules
(`protx_refused{reason}`), the consent checks (`shared_funding_mismatch{reason}`), then the lease (`Platform`) and the
send (`send.*`).

## 8. Records

```rust
pub enum Activation { Active, Inactive, Unknown }
pub struct SharedCapability { pub v24: Activation, pub evonode_shares: Activation, pub can_send: bool }

pub struct ShareTerms {                   // the coordinator's request, and every session's terms
    pub service: String,                  // "a.b.c.d:port[,…]" (D5); "" accepted from a dash-qt coordinator
    pub operator: OperatorChoice,         // MG-04's; a session stores the resolved public key
    pub operator_reward: u16,             // basis points
    pub voting: VotingChoice,             // MG-04's (D10)
    pub early_period_blocks: u32,
    pub early_penalty: u64,               // duffs
}
pub struct ShareSlot { pub label: String, pub amount: u64 }
pub struct StartSharedRequest {
    pub shares: Vec<ShareSlot>,           // 2..=8; row 0 is the coordinator's
    pub terms: ShareTerms,
    pub fee: u64,                         // the registration's network fee, paid by the coordinator (D9)
    pub reward: RewardChoice,             // the coordinator's own
    pub node_run_by: Option<String>,      // `operatorSecretHolder`
}
pub enum RewardChoice { Refund, NewAddress, Address { address: String } }
pub struct JoinSharedRequest { pub share_index: Option<u16>, pub reward: RewardChoice }

pub enum SharedRole { Coordinator, Participant }
pub enum SharedKind { Registration, DissolveTogether, KeyChange }
pub enum SharedStage {                    // the wallet's view; §3
    Invited, Draft, Signing, Combined, FundingSigned,
    Sent, Confirmed { height: u32 }, Conflicted, Dead,  // Dead: a reserved or held input was spent elsewhere
}
pub struct ShareRow {
    pub index: u16, pub label: String, pub amount: u64,
    pub owner_address: Option<String>, pub refund_address: Option<String>, pub reward_address: Option<String>,
    pub mine: bool, pub contributed: bool, pub approved: bool, pub funding_signed: bool,
}
pub struct SharedSession {
    pub id: String, pub code: String,     // session code (6 upper-case hex)
    pub kind: SharedKind, pub role: SharedRole, pub stage: SharedStage, pub revision: u32,
    pub rows: Vec<ShareRow>, pub terms: ShareTermsView, pub fee: Option<u64>,
    pub pro_tx_hash: Option<String>, pub txid: Option<String>,
    pub reserved: Vec<String>,            // this wallet's locked outpoints
    pub next: SharedNext,                 // what this wallet does next (copy, wait, approve, sign, send, nothing)
    pub warning: Option<String>,          // Core's zero-penalty text
    pub created_at: u64, pub updated_at: u64, pub last_error: Option<String>,
}
pub struct SharedMessage { pub text: String, pub code: String, pub name: String, pub file_name: String }
pub enum SharedImport {
    Session { session: SharedSession, changed: bool, received_code: String, warning: Option<String> },
    Standby { description: StandbyDescription },
}
pub struct DissolveRequest { pub pro_tx_hash: String, pub share_index: u16, pub fee: u64 }
pub struct DissolvePreview {
    pub early: bool, pub penalty: u64, pub fee: u64, pub valid_from_height: u32,
    pub returns: Vec<ShareReturn>,        // per share: amount back, to which refund address, mine
}
pub struct Standby { pub pro_tx_hash: String, pub share_index: u16, pub fee: u64,
                     pub full_valid_from: u32, pub created_at: u64 }   // the signed bytes stay inside
pub struct UpdateShareRequest { pub pro_tx_hash: String, pub share_index: u16, pub reward: RewardChoice }
pub struct KeyChangeRequest { pub pro_tx_hash: String,
                              pub operator: KeepOr<OperatorChoice>, pub voting: KeepOr<VotingChoice> }
```

`ProTxOperationKind` gains `UpdateShare` and `Dissolve`. Records follow mn-engine §1. No record holds a secret; a
session holds public data only (addresses, outpoints, the public operator key, signatures), as dash-qt's file does
(`sharedmncreatedialog.cpp:2758-2781`).

## 9. Persistence, crash and restart, timeouts, standby

### 9.1 Tables

app.sqlite, migration `V2026102201__mg_shared_sessions.sql`, scoped by wallet and network, deleted with the wallet,
not in `.dwbackup` (D19):

`shared_mn_sessions`: `id`, `wallet_id`, `network`, `kind`, `role`, `envelope` (the latest canonical message JSON,
public data only), `my_shares` (JSON: index → keychain owner index), `reserved` (the wallet's locked outpoints),
`signed_tx` (this wallet's signed copy, written before it is returned or handed off), `txid`, `out_at` (set when
bytes that may be broadcast leave this wallet: a Signed Contribution returned, or a hand-off begun), `created_at`,
`updated_at`, `last_error`; `CHECK (out_at IS NULL OR signed_tx IS NOT NULL)`.

`shared_mn_standby`: `wallet_id`, `pro_tx_hash`, `share_index`, `fee`, `full_tx`, `immediate_tx`, `full_valid_from`,
`created_at`.

`update_share` and `dissolve` operations are rows of MG-04's `masternode_ops` (new `kind` values), with every MG-04
rule unchanged.

### 9.2 Rules

- **The state is derived on read** from the row, the wallet's history and the list: `confirmed` once the txid is in a
  block of the history; `conflicted` once another transaction in a block spends one of its inputs; `dead` once a
  reserved input of a session that is not out is spent.
- **Save before anything can leave.** Every message that adds this wallet's signature, and every hand-off, is
  preceded by a durable write (`synchronous=FULL`) of the session with the signed bytes and, when the bytes can be
  broadcast by others or the network, `out_at`.
- **A row whose bytes may be out is never deleted:** `DELETE … WHERE out_at IS NULL` (MG-04 D6). Discard refuses
  otherwise. Such a session ends `confirmed` or `conflicted`; to end it the user spends one of its inputs (MG-04 D49's
  abandon path).
- **One call at a time per session** (an in-flight set as MG-04's).
- **Coins:** reserved coins are persistent locks (`lock_manual`) written in the same transaction as the session
  (MG-04 D53); released on discard, on `conflicted`, and on `confirmed`; never released while `out_at` is set and the
  session is open. **A resumed session never reuses a spent reservation:** every call that signs re-reads the wallet's
  reserved inputs and refuses (`shared_message{reason: "inputs_spent"}`, session `dead`) if one is no longer an
  unspent wallet coin locked by this session; nothing re-selects coins after `join_shared`/`start_shared` (D17).
- **Restart:** sessions, locks and standbys are reloaded as stored; an in-memory reservation is re-made only by a
  hand-off, from the saved inputs (MG-04 D7).

### 9.3 Timeouts and abandonment

Core has none: no expiry, no timestamps in the envelope; a session is dead only when a contributed coin is spent
(`sharedmncreatedialog.cpp:3226-3246`, `sharedmndialogs.cpp:947-971`). The wallet adds none (D20). A participant can
discard any time before `out_at` (its coins come back, the others start over); a coordinator likewise. After
`out_at`, the exit is the spend of an input.

### 9.4 Standby

After the registration confirms, every `shared_masternodes` read offers `create_standby` for each share without one
(MG-00 D32's "prepares and stores", gated by a grant, D21). It builds Core's two variants with the same actor and fee
(`createStandby`, `sharedmndialogs.cpp:1634-1668`): **full principal** (penalty 0; valid from `registered + period`)
and **immediate** (penalty paid; valid now). `export_standby` writes Core's file
`standby-dissolution-<first 8 of the ProTx hash>-share<k>.txt` with Core's lines and the two hex lines
(`sharedmndialogs.cpp:1695-1741`); the header lines are English (Core translates them; only the hex lines are read,
D22). Pasting one hex line imports it (`SharedImport::Standby`), described as `masternodelist.cpp:300-373` does.

## 10. Errors

New `MasternodesError` variants and §4 codes:

| Variant | Code | When |
|---|---|---|
| `SharedSessionNotFound` | `masternodes.shared_session_not_found` | |
| `SharedStage { stage }` | `masternodes.shared_stage` | the call doesn't apply at the session's stage or role |
| `SharedMessage { reason }` | `masternodes.shared_message` | `not_shared` ("This isn't a shared-masternode message."), `too_large`, `parse`, `network` (the message's network, never echoing it), `other_session`, `terms_changed`, `older`, `conflict`, `bad_signature{share}`, `inputs_spent{share}`, `not_participant` |
| `SharedFundingMismatch { reason }` | `masternodes.shared_funding_mismatch` | a consent or funding check of §4 fails: `foreign_input`, `overdraw`, `row_changed`, `contribution_changed`, `time_lock`, `template`, `join_signature`, `not_own_inputs` |
| `ShareNotMine` | `masternodes.share_not_mine` | a maintenance call for a share whose owner key this wallet lacks |

`masternodes.unsupported{feature: "shared_masternodes"}` now also covers v24 inactive and a hand-off at this pin
(D1, D3); `protx_refused{reason}` carries Core's shared reasons (§5). `invalid_argument`, `send.*`, `Platform` as
MG-04. No message or code repeats pasted text: a refused paste names only the reason (MG-00 D36; mn-engine D23).
New §4 codes need Swift `engineCodes` and the Swift T0 suite (dw-common).

## 11. Events

`EngineEvent::Masternodes { network }` after every session or standby change, as MG-04's operations. The follower
pass derives `confirmed`/`conflicted`/`dead` and releases settled sessions' locks.

## 12. Tests

**T0, `dw-protx`:**
- the v3 shared ProRegTx, `ExtNetInfo` (IPv4, IPv6), consent digest, `SplitAmountByShares`, the dissolution builder,
  each rule and reject reason, canonical signatures (high S, header aliases);
- the JSON reader and writer against UniValue's own output (D7), the fingerprint, envelope round trips, every
  `fromJson` refusal, `absorbDraftReply`, `adoptLockedTerms` and `mergeEnvelope` cases (the dash-qt test cases of
  `mnsharesessiontests.cpp`), the H-1 and H-8 refusals;
- `tests/oracle_shared.rs` over dashd vectors (`gen_shared_vectors.py`, rc.3, regtest with v24 by `-vbparams`): for a
  three-party registration built by `shared_register_prepare` from a funding template the wallet's code made, the
  payload, consent digest and each `shared_sign` signature equal the wallet's; `shared_combine`'s bytes equal the
  wallet's combine; `shared_update_share`, `shared_update_registrar_prepare`, `shared_dissolve` (both `payPenalty`)
  and `shared_dissolve_prepare` payloads, digests and signatures equal the wallet's on dashd's own transactions
  (RFC 6979 makes the signatures comparable, as MG-04's oracle does);
- the rules module passes dashd's payloads and returns dashd's reason for each mutated payload.

**T0, `dw-vault`:** each new signer signs only under `MasternodeOwner` at an owner path, refuses `Full` and every
other scope, hashes what it is given (no digest entry point).

**T0, `dw-engine`:** every call's refusals in order; a three-wallet session end to end in one process (coordinator
and two participants, messages passed as text) up to the signed registration; a dash-qt participant simulated by
dashd's `shared_sign` output; restarts at each stage (locks kept, nothing re-selected, `out_at` rows not deletable);
the charges; the in-flight mark; the contract test.

**T1, `regtest/functional/dwd_shared.py`** (after BL-187): three wallets (dwcli) create a shared masternode from a
v24 regtest (dashd `-vbparams=v24:0:999999999999:0:10:8:6:5:0`, `rpc.md` §11), one changes its reward address,
`protx info` shows the share table, the operator key rotated together and revived by dashd's `update_service`; then
dissolved alone in the early period (penalty paid), and in a second run together; a standby sent once valid. A
mixed session: dashd's wallet as one participant through `shared_register_prepare` / `shared_sign`, interoperating
with the wallet's envelope.

**Mutants** (brief §4; a scratch script applies each and runs the suites; each must fail a test): a share below the
minimum accepted; a party's payout (refund or reward) swapped; a signature over the wrong hash (consent without the
sequences; ProUpShareTx with the signature; ProDisTx with the wrong `sig_count`); a dissolution combined with fewer
signatures than shares; operator signing attempted (a BLS secret or an operator-signed builder: the guard); a resumed
session reusing a spent reservation; plus the funding checks (`foreign_input`, `overdraw`), H-1 (a newer draft
replacing a locked session) and D16 (a time lock signed).

## 13. Slices

| Part | Slice | Content | Needs | Checkpoint |
|---|---|---|---|---|
| — | 0 | This document and the contract delta (§14) | — | now |
| 1 | 1a | `dw-protx`: codecs, rules, JSON and envelopes, session merge logic; vault signers; the oracle vectors and `oracle_shared.rs`; engine: capability, sessions, persistence, `start_shared`, `import_shared_message`, `join_shared`, `shared_message`, `lock/unlock_shared_terms`, `approve_shared`, `sign_shared_funding`, `discard_shared_session`, `preview_update_share`; dwcli `mn shared start|import|join|message|lock|approve|sign|discard|sessions`; the in-process three-wallet test | — (the pin isn't needed below the hand-off) | report |
| 1 | 1b | `send_shared_registration`, `update_share`, resend; the follower's shared sessions; regtest creation and reward change | **BL-187 (+ #1136)** | READY part 1 |
| 2 | 2 | `preview_dissolve`, `dissolve`, standby (create, export, import, send), dissolve together, key change; regtest create → reward change → dissolve; mutants | BL-187 | READY part 2 |

If BL-187 can't land in wave 14, slice 1a is reviewable and mergeable on its own: the engine reports
`can_send: false`, the hand-off calls aren't on the facade yet, and nothing reaches a user (MG-10b's UI waits for
part 1).

## 14. Contract delta (mn-engine.md)

With slice 1a's code (the contract test fails an orphan §2 row):
- `Contract-Version` 5 → 6 (1a), → 7 with 1b, → 8 with part 2;
- §0: `shared.rs`, `shared_records.rs` (facade); `shared/` (not facade);
- §1 Keys: §2.4's calls sign with keychain owner keys only, through the vault's shared signers (D11), under a
  `MasternodeOp` grant; the attached owner key stays ProUpRegTx-only; nothing signs as operator;
- §2.4 *Shared masternodes (MG-04b)*: §7's rows, the refusals in order;
- §3 regenerated; §4: §10's codes; §5: `SharedStage`, `ProTxOperationKind::{UpdateShare, Dissolve}`, and
  `shared_unsupported` narrowed: a shared masternode the wallet holds a session or a share key for reads as shared
  (`you hold k of n`) once BL-187 lands (until then MG-03 D25 stands);
- §6: the event after a session or standby change;
- §7: `shared_mn_sessions`, `shared_mn_standby`, their locks.

MG-05's FlowKind gains `SharedMasternode` and `Dissolution` (MG-05 D8). `events.ts` and `docs/parity.md` (QT-126,
QT-127 to `partial` with 1a, `done` with part 2) are touched minimally (siblings in flight).

## D-rows

| D | Decision | Why |
|---|---|---|
| D1 | **The hand-off and the regtest acceptance wait for BL-187**, extended by #1136: a graph-wide `[patch]` to a rust-dashcore with #1136, #1138, #1152 and #1156 on top of `d3d50520a` (or platform's next pin). Until then `shared_capability().can_send` is `false`, the hand-off calls (`send_shared_registration`, `update_share`, `dissolve`, `send_standby`, `send_maintenance`) are not on the facade, and the row is ready only as slice 1a. | §0: the pin can neither represent nor decode a version-3 ProRegTx or types 10–12, every hand-off needs a typed `dashcore::Transaction` (`send/mod.rs` `PreparedTx::assembled`), and the wallet would stall on the first block holding one (MG-04 D48, MG-03 D25, BL-187). The probe showed #1138 needs #1136. Hand-rolled broadcasting of raw bytes would still leave the sync stalled. DEC-09 makes the pin move the manager's. **DEC-310 (10-21):** none of the four is in the desktop pin nor in platform v5.0-dev's (d631aaca, v0.45.0), so the bump is a platform-fork branch, a separate pin task the manager staffs; part 1 ends at slice 1a until it lands. |
| D2 | Codecs stay `dw-protx`'s own (raw `Encodable` structs and byte builders), as the type 10–12 ones already are, also after BL-187; the engine converts to dashcore's types only at the hand-off. | The oracle compares bytes the wallet writes itself, independent of the dashcore revision; after the pin moves, a unit test compares `dw-protx`'s bytes with dashcore's encoding of the same transaction. |
| D3 | v24 is `inactive` on mainnet (rc.3 `chainparams.cpp:213`, `NEVER_ACTIVE`); elsewhere `active` once the synced list holds a version-3 entry or a version-3 provider transaction is known (MG-03's `any_shared_seen`, D29–D30), else `unknown`. Creation needs `active`. The regtest harness lets dashd register one version-3 masternode first. | SPV sees no deployment state; a version-3 entry is valid only after v24 (`specialtxman.cpp:1105-1121`), so it proves activation. A registration sent before activation is refused silently (MG-04 D5) and holds coins until abandoned, so `unknown` doesn't create. MG-00 D33: the engine reports, the UI never assumes. |
| D4 | Regular masternodes only. A shared evonode is refused (`invalid_argument`), and `evonode_shares` reads `inactive` everywhere; the codecs still read and hash evonode payloads (the platform node id in the consent digest, rc.3's change) for a masternode created elsewhere. | `evo_shares` is `NEVER_ACTIVE` on mainnet (`chainparams.cpp:222`), and no masternode signs it at rc.3 (`llmq/ehf_signals.cpp:53-55`), so it can't activate on any EHF network; only a regtest with `useehf=0` has it. UX-SPEC §4.25.8 already disables Evonode. Accepted, DEC-310. |
| D5 | The coordinator's service is required and IPv4 (`a.b.c.d:port`, one or more, MG-04 D17); as a participant the wallet accepts a dash-qt coordinator's empty, IPv4 or IPv6 `coreP2PAddrs`, and refuses Tor, I2P, CJDNS or a domain (`unsupported{feature: "shared_masternodes", field: "service"}`). | It must encode the net info to check the payload (`payloadMatchesEnvelope`); IPv6 is ADDRv2's simple case. Core allows an empty service at registration (`specialtxman.cpp:1174-1177`). |
| D6 | The session protocol is dash-qt's v1 exactly: both envelopes, key order, stages, merge rules, `payloadMatchesEnvelope`, the funding template, the standby hex and file; the wallet adds stricter checks (D12, D15, D16) but no field. | Interop with dash-qt (the brief). Any extra key would change the code dash-qt displays (H-12), and a different structure would be a protocol Core doesn't define. |
| D7 | `dw-protx::shared::json`: an own reader and writer reproducing UniValue's model (ordered keys, duplicates kept, first match on lookup, number tokens kept) and `write(2)` byte for byte. Its oracle is rc.3's `src/univalue` compiled with a short driver that prints `write(2)` and the fingerprint of each input. The driver's source, its inputs, the vectors it produced and a README on regenerating them from rc.3 are committed under `testdata/oracle/univalue/`; tests and CI read only the vectors, so CI builds no C++ (DEC-310). | The message code depends on the exact rendering, including unknown keys and number tokens (`mnsharesession.cpp:225-232`). `serde_json` with `preserve_order` and `arbitrary_precision` would change number handling for every crate in the graph (feature unification). Core's own writer is the only authority on its output. |
| D8 | `fundingTx` from a message is never trusted: the wallet rebuilds it from `contributions` and compares; a coordinator prepares from its rebuild. | dash-qt doesn't validate it on import (H-7). The consent digest binds the transaction, and the rebuild is what the parties agreed in `contributions`. |
| D9 | **The coordinator pays the whole registration fee; each participant contributes exactly its share** (dust change goes to the fee). The fee is set by the coordinator (default 100,000 duffs, Core's), at least the relay minimum for the final size. This **replaces MG-00 D41's fee split with a per-participant maximum.** | dash-qt's model (`sharedmncreatedialog.cpp:81,3010-3015`; H-18). The envelope has no field for a split or a maximum, and an added key breaks interop (D6). A participant can't be made to pay a part it can't see in the terms. The fee is small (0.001 DASH default). MG-10b's copy changes: the invitation says the coordinator pays the network fee. Accepted, DEC-310. |
| D10 | Keys per party: the share owner key is a keychain owner key (lowest free index 0–19, MG-04 D13, sessions counting as in use); the refund is a fresh BIP44 receive address; the reward defaults to none (the refund gets rewards); change goes to a fresh internal address; the coordinator's voting key is its keychain voting key unless given. A wallet starts or joins with one share per session. | Owner keys at keychain paths make the share visible to MG-03's role scan and recoverable from the seed (a standby can be signed again after a restore). A funds-account refund keeps the principal in the balance (MG-04 D55). One share per wallet keeps "my row" unambiguous, as `inferMyShare` assumes; a wallet holding several (another device of the seed, a dash-qt import) still signs for each. |
| D11 | Owner signatures come from vault methods that take the transaction or payload and compute Core's digest themselves (§6.2), at keychain owner paths, `KeyUse::OwnerPayload`, `MasternodeOwner` only. The engine verifies each returned signature (canonical, recovers to the share's key) before using it. | MG-04 D11's argument: a digest signer at an owner key would sign anything, and owner keys are the shared masternode's authority (consent, dissolution, reward, keys). MG-12 reviews these signers. The check catches a vault or key mismatch before a message leaves. |
| D12 | Stricter than dash-qt before signing: `tx.nVersion` 3, payload version 3, `inputsHash`, and at `combined` every join signature verified against the consent digest (and equal to `sigs`). | dash-qt leaves these to consensus (H-6). A party that signs its inputs on a transaction that can't be valid gives nothing away, but checking costs nothing and gives the user an error instead of a silent refusal (MG-04 D5). |
| D13 | Only the coordinator sends the registration; a participant's copy is never handed off by the wallet. | dash-qt's rule (`sharedmncreatedialog.cpp:2908-2912`). The participant still treats its signed copy as may-be-out (D17): anyone holding it can broadcast (H-9). |
| D14 | The wallet writes `prepareWallet` as `""` and infers its role from its own session row, not from names. | dash-qt writes the coordinator's wallet name (`sharedmncreatedialog.cpp:2218`), which leaks a local name to every participant; it uses it only to recognise its own file (`coordinatedHere`, `:3360-3376`). The wallet's sessions are in app.sqlite, so it needs no name. A dash-qt coordinator reading the wallet's envelope sees "" and decides by role as for any foreign file. |
| D15 | Stricter merges: a reply fills at most one row, the row its one new contribution names (H-8); an import whose `revision` is higher than the wallet's while its stage is past `draft` is refused as `shared_message{conflict}`, never adopted (H-1); as a participant the wallet echoes `revision` unchanged in Approvals and Signed Contributions. | dash-qt's fill-empty-only lets any reply fill any row and its revision-first merge lets a late draft replace locked terms (`mnsharesession.cpp:1238-1257,1361-1377`). dash-qt replies satisfy both rules, so interop holds. |
| D16 | The wallet never signs a registration or dissolution with an unsatisfied lock time or a relative lock (Core's `HasTimeLock`, `providertx_service.cpp:683-705`); its own transactions have lock time 0 and sequence `0xffffffff`. | dash-qt never sets `allowTimeLocks` (`:1100-1109`), and both digests commit to the lock fields, so a lock a co-signer missed would delay the exit. Core's funding template and dissolution builder use these values (`mnsharesession.h:108-113`, `providertx_service.cpp:661-664`). |
| D17 | A session's `out_at` is set, durably, **before** a Signed Contribution is returned (participant) or a hand-off begins (coordinator, maintenance). From then on the session is never deleted and its reserved coins stay locked until `confirmed` or `conflicted`. Every signing call re-checks that each reserved input is still an unspent wallet coin locked by this session, and refuses otherwise; no call re-selects coins after the reservation. | MG-07's lesson: once bytes may be out, deleting the row or freeing the coins lets a second spend race the first. A participant can't know whether the coordinator broadcast. A resumed session that re-selected coins could sign a second transaction over coins already committed (the brief's mutant). |
| D18 | Charges: consent signatures charge 0; funding signatures charge the wallet's net debit of the registration; maintenance charges its outflow (the fee), and a dissolution what the wallet's shares give up (penalty + fee when it is the actor). | MG-05 D4 charges outflow. A consent signature moves nothing without the party's funding signature. The registration moves the share out of the wallet to the template (not the wallet's), so it is outflow. A dissolution pays the wallet back; what it costs the wallet is the penalty and the fee from its own share. |
| D19 | Sessions and standbys are not carried by `.dwbackup`. | MG-04 D24's reasons: a row is this installation's working state and its locks. A restored wallet re-derives its owner keys and can sign a new standby while the masternode exists. Core keeps standbys in a separate file for the same reason; export is the backup (§9.4). |
| D20 | No timeouts. A session lives until sent, discarded or dead. | Core has none (`sharedmncreatedialog.cpp:3226-3246`); a timeout would release coins another party may already have counted on. MG-00 D32: the user keeps or discards. |
| D21 | The standby is prepared, not silently: after confirmation the session reads `next: create_standby`, and `create_standby` needs a grant (the owner key signs). | It signs with the owner key and commits up to penalty + fee (the immediate variant), so it is a signing step under MG-05. MG-00 D32's "prepares and stores" is kept as a prompt the UI shows right after confirmation. |
| D22 | The standby file's header lines are English; only the two hex lines are read on import, and a paste must be one hex line, as in Core. | Core translates the header (`tr()`, `sharedmndialogs.cpp:1702-1728`) and never parses it (`sharedmnwidgets.cpp:388-396`). |
| D23 | Two shared operations of one masternode can't be out at once from this wallet: a key change or a reward change waits while another `update_share`, key change or dissolution of it is `signed`, `not_sent` or `sent` and not in a block (`masternodes.update_pending{operation}`), as MG-04 D49. | Core's mempool allows several ProUpShareTx of one share (the last mined wins, `specialtxman.cpp:620-648`) and conflicts a key change with a pending reward change paying the new voting key (`txmempool.cpp:1737-1786`). One at a time keeps "what the user asked last" what is mined. |

### Follow-ups

- BL-187 with #1136 (D1): a platform-fork pin branch, staffed separately by the manager (DEC-310).
- MG-10b copy: the coordinator pays the registration's network fee; participants pay none (D9, replacing MG-00 D41).
- MG-03b: reading other parties' reward changes (ProUpShareTx by ProTx hash) and per-share payouts; until then the
  wallet's own share's reward is known from its own operations only.
- A one-call abandon for a session stuck `out` (MG-04's follow-up applies).
- Shared evonodes once `evo_shares` can activate (D4).
- Tor/I2P/CJDNS/domain services in the net info (D5).
