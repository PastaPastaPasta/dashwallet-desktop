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

Revision 2 answers design review r1 under DEC-311; the appendix lists each finding and where it is answered.
Revision 3 folds in design review r2 (ACCEPT WITH NITS, DEC-312): r2-m1 to r2-m6 and n-r2-1 to n-r2-10, each listed
in the appendix.

## 0. Blocker: the dashcore pin (BL-187)

**Since BL-187** (desktop main 534bb7c7, follow-merged at a1c4ece1, DEC-338) the pin is rust-dashcore 626c91d1 with
platform fork 35e96ca16b (dc0c946307 since slice 1c, D75): it decodes a version-3 ProRegTx with its `SharedCollateral` and types 10–12, and a block
holding them reaches dash-spv. The wallet keeps its own codecs (D2), which `tests/dashcore_shared.rs` holds equal to
dashcore's on every dashd vector (D62). `can_send` stays false in slice 1a; slice 1b adds M3 and the hand-off calls
behind one switch that stays off (DEC-678, D71), and slice 1c turned it on once D39 (D75) and M9 (D78) were closed (§13, DEC-338). The rest of this section is the state before BL-187.

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
| Codecs: ProRegTx v3 shared, `ExtNetInfo`, raw special transactions, the consent digest, the reward and dissolution split, canonical signatures | `rust/crates/dw-protx/src/shared/{mod,registration,netinfo,rawtx}.rs` |
| Core's rules for types 1 (shared), 10, 11 and 12, before anything is signed (§5, §5.1) | `rust/crates/dw-protx/src/shared/rules.rs` |
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
| Activation | v24 gates everything; a shared registration is payload version 3 (`specialtxman.cpp:1112-1116,1158-1161`). Shared evonodes also need `evo_shares` (`specialtxman.cpp:1163-1166`), which no masternode signs at rc.3 (`llmq/ehf_signals.cpp:53-55`) on any EHF network (`chainparams.cpp:222,431,613`) | the engine reports v24 from evidence on every network (D3); regular masternodes only (D4) |
| Shares | 2–8; each ≥ 100 DASH and ≤ the collateral; they sum to exactly 1,000 DASH; owner keys unique and non-null; refund scripts unique; refund and reward P2PKH or P2SH, never paying the voting key or any owner key (`providertx.cpp:63-130`) | the same rules, with Core's reject reasons (`dw-protx::shared::rules`), before anything is signed |
| Exit terms | `nEarlyPeriodBlocks` ≤ 420,480; `0 ≤ nEarlyPenalty <` the smallest share; penalty 0 without a period (`providertx.cpp:63-130`) | the same; the zero-penalty warning text is Core's (`providertx_service.cpp:618-619`) |
| Funding | the coordinator's wizard builds a funding template from every party's inputs and change (`mnsharesession.cpp:129-158,802-813`); `shared_register_prepare` appends the 1,000 DASH template output last (`providertx_service.cpp:1011-1088`) | the same template and assembly, byte for byte (§2.4) |
| Fee | paid by the coordinator: its contribution is its share plus the fee it types (default 100,000 duffs); a participant contributes exactly its share (`sharedmncreatedialog.cpp:81,3010-3015`) | the same (D9); the wallet checks its own side exactly and can't value other parties' inputs (D26) |
| Consent | each share owner signs `MakeSharedRegConsentHash` (`providertx.cpp:335-373`): every prevout and sequence, the lock time, every output, the share table, the terms | the same digest, computed by the vault from the transaction it is shown (D11) |
| Signing order | join signatures first, then each party signs its own inputs `SIGHASH_ALL` (`providertx_service.cpp:1195-1199`); dash-qt's coordinator signs its own inputs right after combining (`sharedmncreatedialog.cpp:2576-2591`) | join signatures first, then the participants' inputs; **the wallet's coordinator signs its own inputs last**, in `send_shared_registration` (D24) |
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
| `sessionId` | string | 64 lower-case hex | dash-qt: non-empty (`mnsharesession.cpp:399-403`); the wallet: exactly 64 lower-case hex, else `shared_message{parse}` (D28) |
| `revision` | int | starts at 1 | ≥ 1 |
| `stage` | string | `draft`, `frozen`, `signing`, `combined`, `fundingSigned`, `broadcast` | one of these |
| `fundingTx` | string | hex of the funding template (§2.4), `""` before a contribution | not trusted: rebuilt from `contributions` (D8) |
| `shares[]` | objects | `{label, amount, ownerAddress, refundAddress, rewardAddress}` in share order; `""` for a field not yet given | ≤ 8; `amount` in money range; owner and refund required (may be `""`); the wallet adds: labels non-empty and unique after trimming (D28) |
| `terms` | object | `{coreP2PAddrs, operatorPubKey, votingAddress, operatorReward, earlyPeriodBlocks, earlyPenalty}`; `coreP2PAddrs` is `ip:port` entries joined by `,`; `operatorReward` in basis points | members optional; compared as exact strings and integers (`sameTerms`, `mnsharesession.cpp:1098-1106`) |
| `contributions[]` | objects | `{label, inputs[{txid, vout, sequence}], change{address, amount}?, changeIndex}`, sorted by share index | ≤ 8 entries, ≤ 64 inputs each; a contribution belongs to the share whose `label` it carries (first match; unknown labels sort last, `:91-97,1274-1278`); the wallet adds: every label names a share, at most one contribution per share (D28) |
| `protx` | string | `""` (draft) or the shared ProRegTx hex | at `frozen`+: decodes, its consent digest equals `consentHash`, and `payloadMatchesEnvelope` holds (§2.4) |
| `consentHash` | string | `""` or the digest in display order (byte-reversed hex) | 64 hex or empty |
| `collateralIndex` | int | `-1` or the collateral output | equals the number of change outputs |
| `sigs[]` | objects | `[{shareIndex, signature}]`, ascending; base64 (padded) of 65 bytes | at `combined`+ every share has one and each verifies |
| `prepareWallet` | string | the coordinator's wallet **name** in dash-qt | the wallet writes `""` (D14) |
| `operatorSecretHolder` | string | free text "node run by" | carried |
| `coordinatorLabel` | string | the coordinator's share label | carried |
| `fingerprint` | string | §2.2 | warns on mismatch |

Unknown keys are dropped by dash-qt and change its displayed code (H-12 in the research note); the wallet writes none.

More of `fromJson` (`mnsharesession.cpp:582-655`), mirrored: a duplicate `shareIndex` in `sigs` is refused; at
`frozen` and after, a signature's `shareIndex` must be in range; at `fundingSigned` and after, every input of the
transaction has a non-empty scriptSig; `consentHash` is lower-cased on parse; integers are read per field as
`getInt<int>` or `getInt<int64_t>`, so an over-range `operatorReward` parses and fails later, with the same error as
dash-qt; the fingerprint comparison is case-insensitive (`:244`), and `WithoutFingerprint` removes every
`fingerprint` key, a duplicate too (`:46-56`).

**Codes on import (H-12).** `SharedImport::Session.received_code` is the received object's recomputed fingerprint
(the code the sender reads out); the merged session's code is `SharedSession.message_code`, which is what dash-qt
displays after a merge (`sharedmncreatedialog.cpp:2890-2894`). Both are shown, so "read the codes to each other"
works with a dash-qt party.

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

**Maintenance payloads** (all `tx.nVersion 3`, payload version 1; `providertx.h:436-462,484-503,517-545`):

```
ProDisTx (type 10)         u16 nVersion | [32] proTxHash | u16 actorIndex | u8 n | n × [65] sig
ProUpShareTx (type 11)     u16 nVersion | [32] proTxHash | u16 shareIndex | script scriptReward | [32] inputsHash
                           | cs vchSig                                   (SerializeHash drops vchSig)
ProUpSharedRegTx (type 12) u16 nVersion | [32] proTxHash | [48] pubKeyOperator (basic) | [20] keyIDVoting
                           | [32] inputsHash | u8 n | n × [65] sig      (SerializeHash drops the count and sigs)
```

`scriptReward` of a ProUpShareTx is explicit: Core's comment is "use the refund script to reset rewards"
(`providertx.h:493`) and an empty one is `bad-proupshare-payee` (§5.1, D31).

### 2.5 The sigs envelope

```
{ "type": "dash-shared-mn-sigs", "version": 1, "network": …, "kind": "dissolve" | "registrar",
  "proTxHash": …, "tx": <hex>, "signatures": [{shareIndex, signature}] (ascending), "fingerprint": … }
```

(`sharedmndialogs.cpp:653-665`.) A reader checks the network, the kind, the ProTx hash (case-insensitive), adopts a
non-empty `tx` only if it is the one it holds (or it holds none), and verifies each signature against the digest it
recomputes (`sharedmndialogs.cpp:253-309,559-620,683-730`): for `dissolve`, `MakeSignHash(tx, n)` of a template that
returns every non-actor exactly its principal and pays at most the actor's refund (`SharedMnReadDissolution`,
`sharedmnwidgets.cpp:272-352`); for `registrar`, `SerializeHash(payload)`.

Reader rules in full (`importEnvelopeChecked`, `addSignatures`, `setTransaction`; `sharedmndialogs.cpp:253-309,
559-620,683-730`), mirrored unless marked:
- `type` exact; the fingerprint warns only; a missing `network` is a mismatch (`:698-704`); `kind` must equal the
  session's kind exactly (`:705-709`); `proTxHash` compared case-insensitively (`:710-714`);
- `version` is never read by dash-qt (H-4); **the wallet requires `1`** (`shared_message{parse}`), safe because dash-qt
  always writes 1 (`:657`);
- a dissolve `tx` must carry empty `vchSigs` (`:279-283`); **the wallet also requires** `vin == [collateral]` with an
  empty scriptSig (`specialtxman.cpp:1689-1693`; `SharedMnReadDissolution` checks only the count), and since 2c the
  whole transaction equal to Core's builder output for its actor and fee (D92);
- each signature entry needs `shareIndex` (`getInt<int>`, in range) and a base64 signature of exactly 65 bytes that
  verifies (`VerifyHashCanonical`); a bad entry is skipped with Core's warning and the rest absorbed; a byte-identical
  duplicate is ignored; a different second signature for a share is skipped with a warning, the first kept
  (`:577-613`).

The wallet writes the keys in the order above (`:655-663`), signatures ascending by `shareIndex`, `proTxHash` as
lower-case display hex (`GetHex`), padded base64. dash-qt also takes a **bare JSON array** as signatures for the
transaction it already holds (`:685-691`); the wallet accepts one only with a target maintenance session named in
the import call (`import_shared_message(…, target)`, §7), and refuses it otherwise (`shared_message{parse}`).

## 3. State machines

### 3.0 Import decisions

dash-qt's `absorbSession` dispatches on (our stage, their stage) only (`sharedmncreatedialog.cpp:2920-2960`):
draft + draft → `absorbDraftReply`; our draft + anything else → `adoptLockedTerms`; otherwise `mergeEnvelope`, which
replaces the session with any higher revision (`OtherNewer` → `replaceSession`, `mnsharesession.cpp:1362-1368`,
`sharedmncreatedialog.cpp:2952-2953`) and, at one revision, unions signatures, merges funding inputs and adopts a
further stage (`mnsharesession.cpp:1376-1494`). The wallet decides by role as well, and refuses the cells where
dash-qt would adopt a copy the wallet can't check. **The table is read top-down and the first matching row decides**
(r2-m1). Every cell is a T0 case. "Own row and contribution unchanged" is
`adoptLockedTerms`' test (`SameShare`, `SameContribution`, `mnsharesession.cpp:1333-1349`); `rev` compares the
imported `revision` with the session's; any import must first pass §2.3 and the session id.

| Our session | Their copy | Result |
|---|---|---|
| none | `draft` | a new participant session, `invited`; no coin touched |
| none, or participant `invited` | `frozen` or later | `shared_message{not_participant}`: there is no recorded row or contribution to check it against (§4); a wallet whose seed holds a row but which didn't join from this installation continues on the device that joined (D10) |
| participant `invited` | `draft`, rev higher | replaces the invitation (nothing of ours is in it) |
| participant `joined` | `draft`, rev higher | taken if our row and contribution are unchanged (the coordinator absorbed other replies), else `row_changed`/`contribution_changed` |
| participant `joined` | `frozen`/`signing` | `adoptLockedTerms`: terms, labels and amounts equal our copy's (`terms_changed` otherwise), our row and contribution unchanged → `to_approve` |
| participant `to_approve`/`approved`, `out_at` unset | `draft`, rev higher | **M-1 case 1:** our row and contribution unchanged → back to `joined`, our consent signature dropped, the same reservation kept; `shared_message` re-emits Details with the same owner, refund, reward and change |
| participant `to_approve`/`approved`, `out_at` unset | `frozen`/`signing`, rev higher | **M-1 case 2:** our row and contribution unchanged → `to_approve` with the masternode-wide terms treated as new: the session carries the diff against the terms we last approved, and only a fresh `approve_shared` signs |
| participant `to_approve`/`approved` | `signing`, same rev | merges others' approvals (no change to ours), if its consent hash equals ours, else `shared_message{conflict}` (`mergeEnvelope`, `mnsharesession.cpp:1391-1397`) |
| participant `approved` | `combined`, same rev | → `to_sign` after D12's checks |
| participant, any | `combined` or later, rev higher | `shared_message{conflict}` (M-1 case 3, D15) |
| participant `to_sign` or later, or `out_at` set | rev higher | `shared_message{conflict}` |
| participant `to_sign` or later | same rev, `combined` or later | per-input merge (below, m-7); `broadcast` records the Final Record |
| coordinator `draft` | `draft` | `absorbDraftReply`, filling at most the one row its one new contribution names (D15). **The revision is not compared**: every participant answers the same Invitation, so replies carry equal or lower revisions than ours (`mnsharesession.cpp:1204-1284` never reads it). A reply that adds nothing is a no-op with no bump (`:1265-1270`); one that adds something sets ours + 1 (`:1280`). A reply whose copy of the coordinator's contribution isn't the current one is refused `shared_message{older}` (D33) |
| coordinator `draft` | `frozen` or later | `shared_message{older}`: a copy of terms this coordinator unlocked (dash-qt would adopt it) |
| coordinator `signing` | `draft` | `shared_message{older}` |
| coordinator `signing` | `signing`, same rev | merges approvals, each verified as added, if its consent hash equals ours, else `shared_message{conflict}` (`:1391-1397`) |
| coordinator `combined`/`fundingSigned` | `combined`/`fundingSigned`, same rev | per-input merge (below) |
| coordinator past `draft` | rev higher | `shared_message{conflict}` (D15) |
| coordinator past `draft` | stage ahead of ours that we didn't produce (`combined` while we are `signing`) | `shared_message{conflict}` |
| any other | rev lower | `shared_message{older}` |

**Per-input merge** (`mnsharesession.cpp:1426-1463`, stricter, m-7): the transaction with scriptSigs cleared must be
identical. An incoming scriptSig on **one of this wallet's inputs** is never taken (`foreign_input`); ours are only
the ones this wallet made. A foreign input's scriptSig can't be verified (the wallet has no prevout); before `out_at`
a later one for the same input replaces the earlier (dash-qt calls that a conflict), and after `out_at` the
transaction is fixed. A bad foreign scriptSig only makes the registration invalid: the coordinator's send then sits
`sent` with no reject (MG-04 D5), and the exit is §9.3's abandon.

### 3.1 Creation: coordinator

```
start ─▶ inviting(draft) ─▶ collecting(draft) ─lock+consent─▶ approving(signing) ─all approvals: combine─▶
signing(combined) ─all participants' funding sigs─▶ ready ─send: sign own + save─▶ sent(broadcast) ─▶ confirmed
     ▲  set fee (draft, before any reply)            ◀─unlock (discards approvals)─┘
any state before `sent` ──discard──▶ (deleted, coins unlocked)
sent, not confirmed ──abandon──▶ (an own input spent to self; row kept, §9.3)
```

1. `start_shared` (§7): checks the request (§5, D28's labels), picks this wallet's keys (D10), reserves its coins
   (share + fee, D9) with persistent locks, writes the session (`draft`, revision 1, the coordinator's row filled, its
   contribution added), and returns the Invitation.
2. `import_shared_message` with a Details reply: §3.0's cell (`absorbDraftReply`, `mnsharesession.cpp:1204-1284`):
   same session, same terms (exact), same labels and amounts; empty row fields filled, new contributions added and
   sorted by share index, `revision` = ours + 1; at most one row, the one its one new contribution names (D15, D28).
3. `lock_shared_terms` (grant): every row has owner and refund and a contribution; builds the ProRegTx (§2.4), runs
   §5's rules and `payloadMatchesEnvelope`; computes the minimum fee from the final size with **each foreign input
   counted as P2PKH** (148 bytes; the wallet can't see their scripts), and refuses `shared_funding_mismatch{fee_low}`
   with the needed fee when its reserved fee is below it (the coordinator discards and restarts with a higher fee:
   `set_shared_fee` is closed once others replied, D33); then runs
   §4's consent checks on its own side (`foreign_input`, `overdraw` against its recorded contribution, its row,
   D16, as dash-qt's `checkOwnFunding` does for both roles, `sharedmncreatedialog.cpp:2533-2538`), signs this
   wallet's consent and moves to `signing` with `revision` unchanged (`freeze` keeps it, `mnsharesession.cpp:885-923`).
   It does **not** claim Core's "fee positive" check: that needs every input's value (`fundingCheck`,
   `sharedmncreatedialog.cpp:3259-3324`), which an SPV wallet has for its own coins only. The session carries
   Core's warning instead, naming the contributions it can't value (`:3298-3305`; D26).
4. Approvals, in any order (§3.0; `mergeEnvelope`, `mnsharesession.cpp:1353-1495`): same session and consent digest,
   each signature verified as it is added. When every share has signed, the wallet combines (join signatures into
   the payload) and moves to `combined`: the Signing Request, with **no coordinator scriptSig** (D24).
5. Signed contributions, in any order: §3.0's per-input merge. When only the coordinator's inputs are unsigned the
   session reads `ready`; `send_shared_registration` (grant; refused while `can_send` is false, D1) signs them,
   saves the bytes durably with `out_at`, then hands them to the fence (§6.3) and moves to `broadcast`: the Final
   Record.
6. The follower marks the session `confirmed` once the ProRegTx is in a block of the wallet's history, and
   `conflicted` once another transaction in a block spends one of **this wallet's** inputs. Another party's
   double-spend is invisible to the wallet's filters (they match scripts, not outpoints, D27): the session stays
   `sent` until the user abandons it (§9.3).

`unlock_shared_terms` returns a `frozen`/`signing` session to `draft` (revision + 1, approvals discarded,
`mnsharesession.cpp:925-935`). Not past `signing`: once combined, participants may have signed inputs.

### 3.2 Creation: participant

```
import invitation ─▶ invited(draft) ─join: reserve─▶ joined(draft) ─import locked terms─▶ to_approve(signing)
─approve─▶ approved(signing) ─import signing request─▶ to_sign(combined) ─sign funding─▶ signed(combined/fundingSigned)
─import final record or see the ProRegTx─▶ confirmed
to_approve / approved ─import newer draft (own row unchanged)─▶ joined          (M-1 case 1, §3.0)
approved ─import newer locked terms (own row unchanged)─▶ to_approve (re-approve) (M-1 case 2)
invited / joined / to_approve / approved ──discard──▶ (deleted, coins unlocked)
signed, not confirmed ──abandon──▶ (an own input spent to self; row kept, §9.3)
```

1. The first import of an Invitation creates a session in `invited` (role participant; no coin is touched). The wallet
   infers its share as dash-qt does (`inferMyShare`, `sharedmncreatedialog.cpp:3326-3348`): the row whose owner key
   it holds, else the single empty row, else the request must name one.
2. `join_shared`: fills its row (owner key, refund, reward: D10), reserves exactly its share (D9), records its
   contribution (inputs, change) as the base for every later check, writes the session, and returns the Details: the
   invitation with its row and contribution, `revision` + 1 (as `addContribution` does). The participant's empty
   service warning (D5) is on the session from here.
3. Import of Locked Terms: §3.0 (`adoptLockedTerms`, `mnsharesession.cpp:1286-1351`).
4. `approve_shared` (grant): §4's checks, then a consent signature for each share whose owner key this wallet holds;
   returns the Approval (the locked copy plus its signature, `revision` unchanged: H-1). The approval records the
   terms it approved, the base of M-1 case 2's diff.
5. Import of the Signing Request (`combined`): every join signature verified (D12). `sign_shared_funding` (grant;
   **refused with `unsupported{feature: "shared_masternodes"}` while `can_send` is false**, D1) signs this wallet's
   inputs (SIGHASH_ALL), checks that only they changed (`signedOnlyOwnInputs`, `sharedmncreatedialog.cpp:2448-2466`),
   **saves the signed copy durably and marks it may-be-out before returning it** (D17), and returns the Signed
   Contribution.
6. From then on anyone holding a complete copy can broadcast it (H-9); under a dash-qt coordinator, which signs its
   own inputs at combine, this wallet's signature may be the last one (D24). The session is never deleted and its
   coins stay locked until it reads `confirmed` or `conflicted` (D17), or the user abandons it (§9.3).

### 3.3 Maintenance

| Flow | Parties | States |
|---|---|---|
| Change my reward address (ProUpShareTx) | this wallet | MG-04's operation states: `signed`, `not_sent`, `sent`, `confirmed`, `conflicted` (kind `update_share`) |
| Dissolve now (unilateral ProDisTx) | this wallet | the same (kind `dissolve`), no fee inputs |
| Standby (two unilateral ProDisTx, not sent) | this wallet | `stored` → (sent through `send_standby` as a `dissolve` operation) |
| Dissolve together (unanimous ProDisTx) | every owner; any can send | `collecting` → `complete` → `sent` → `confirmed` / `conflicted` (as `SharedStage`: `signing` → `combined` → `sent` → `confirmed` / `conflicted`, or `dead{delisted}`, D94) |
| Change keys together (ProUpSharedRegTx) | every owner; the funding wallet sends | `collecting` → `complete` → `sent` → `confirmed` / `stale` / `dead` |

**Where the share table comes from (D29).** The synced list (SML) carries no shares, early terms or registered height
(`CSimplifiedMNListEntry::shares` is mem-only, `evo/simplifiedmns.h:40`). Every maintenance builder and check reads
the registration ProRegTx from the wallet's history (the wallet funded or joined it, so it is there) and the
registered height from the block that holds it; a maintenance call for a masternode whose registration isn't in the
history is refused (`masternodes.shared_registration_unknown`).

**One block deep, every call (m-16).** Consensus validates types 10–12 against `pindexPrev`'s list
(`specialtxman.cpp:1576-1579,1633-1636,1808-1811`); dash-qt gates only its dissolve dialog
(`sharedmndialogs.cpp:1113-1129`). Every maintenance call (`preview_update_share`, `update_share`,
`preview_dissolve`, `dissolve`, `create_standby`, `send_standby`, `start_dissolve_together`, `start_key_change`,
`approve_maintenance`, `send_maintenance`) requires `tip > registered_height` and the masternode in the synced list,
else `masternodes.shared_too_early`.

**Maintenance sessions.** The two multi-party flows are maintenance sessions with the id **(kind, ProTx hash, sign
hash)**: SHA-256 of `kind ‖ proTxHash ‖ signHash`, exposed as 64 lower-case hex of the digest in its natural byte order (m-17). The bytes are the kind as the sigs envelope's ASCII `kind` (`dissolve` or `registrar`), then the ProTx hash and the sign hash, 32 bytes each in internal (wire) order, not the reversed display order (n-r2-5). The sign hash is what the owners sign
(`MakeSignHash(tx, n)` for a dissolution, `SerializeHash(payload)` for a key change), so it is fixed while the txid
changes as signatures are inserted and as the funder re-signs its inputs (`providertx_service.cpp:1209-1237`). A
session is created by the wallet that prepares it (`start_dissolve_together`, `start_key_change`) or by the first
import of a sigs envelope. Each import adds verified signatures; `approve_maintenance` (grant) adds this wallet's;
`send_maintenance` hands off a complete one (refused while `can_send` is false): a dissolution from any wallet
(`CombineShared` needs none, `providertx_service.cpp:1209-1220`), a key change only from the wallet whose inputs fund
it, which re-signs them after the payload changed (`providertx_service.cpp:1221-1237`).

**A key change binds its base (M-5, D25).** A ProUpSharedRegTx carries absolute keys, and "keep" copies the current
values (`providertx_service.cpp:1345-1353`); consensus applies them as given and an operator change resets the
operator fields and PoSe-bans the masternode (`specialtxman.cpp:663-673`). Its only replay protection is
`inputsHash`, the funder's fee inputs (`providertx.h:527`). So the session stores its **base**, the list's
`(pubKeyOperator, keyIDVoting)` when it is prepared or first imported, and:
- `approve_maintenance` and `send_maintenance` refuse once the synced list's keys differ from the base
  (`shared_message{stale}`), and the session reads `stale`;
- the approval preview diffs **every** payload key against the synced list at approval time ("voting: V1 → V0"), so
  a request built on an older state shows its revert whatever base this wallet imported at; the approval records the
  diff it showed, and `approve_maintenance` refuses (`stale`) if the list moved since that preview (r2-m3);
- the funder's `stale` session offers MG-04 D49's abandon (spend a fee input to itself), which voids every signature
  still out; until then the signed request stays usable by its funder, and the preview says so;
- the approval preview shows Core's effects to every approver (m-12): an operator change resets the operator fields
  and PoSe-bans the masternode until a ProUpServTx (`specialtxman.cpp:666-672`); the new key is basic-scheme
  (`providertx.cpp:582-584`); the mempool takes one operator change per masternode (`txmempool.cpp:1730-1735`).

**States from what the wallet can see (D27).** BIP158 filters match scripts, so the wallet sees spends of its own
coins only. A dissolution is seen in the history when it pays this wallet (`confirmed`; `conflicted` if another ProDisTx spent
the collateral), else by the collateral leaving the list (`confirmed`, D81). A key change is `confirmed` when the list's keys equal the requested ones, `stale`
when they move elsewhere, `dead` when the masternode leaves the list; the funder also sees its fee inputs spent
(`conflicted` when spent by another transaction). dash-qt's maintenance liveness asks the node's UTXO set
(`sharedmndialogs.cpp:947-971`), which the wallet doesn't have.

## 4. Consent: what each party signs, and what it checks first

Every share-owner signature is Core's `SignCompact` of a raw 32-byte digest, no message prefix, verified by
`VerifyHashCanonical`: 65 bytes, header 27–34, low S, the recovered key's id equal to the share's `keyIDOwner`
(`messagesigner.cpp:48-107`). The vault computes each digest itself from the transaction or payload it is handed
(D11), and the engine checks the signature it gets back (canonical, recovers to the share's key) before using it.
Every row's checks include §5.1's Core rules for that type, run before the signature (M-4).

| Signature | Digest (rc.3) | Signed by | The wallet checks first |
|---|---|---|---|
| Registration consent (join signature) | `MakeSharedRegConsentHash(tx)`: `cs "DashSharedMNReg"`, payload version, tx version, tx type, lock time, `CalcTxInputsHash`, every input's sequence, `CalcTxOutputsHash`, MN type, mode, net info, (evonode: platform node id), voting key id, operator key, operator reward, `u8 n`, the shares, period, penalty (`providertx.cpp:335-373`) | each share owner whose key this wallet holds | the transaction decodes and is the registration of §2.4; `payloadMatchesEnvelope` and D12; its own row (amount, owner, refund, reward) and contribution exactly as it recorded them at join or start; no other contribution holds one of its coins (`foreign_input`, `foreignWalletInputs`, `sharedmncreatedialog.cpp:2433-2446`); **its own debit exactly** (m-1, below); §5's rules; lock time 0 and every sequence `0xffffffff` (D16); the digest recomputed equals `consentHash`; for a participant, M-1 case 2's terms diff acknowledged |
| Funding inputs | `SIGHASH_ALL` over the combined transaction | each party, its own inputs | the consent checks again on the combined transaction; every join signature present and valid (D12); only its own inputs gain scriptSigs (`signedOnlyOwnInputs`); its reserved inputs still unspent wallet coins locked by this session (`inputs_spent`, D17); `can_send` (D1) |
| ProUpShareTx | `SerializeHash(payload)` without `vchSig`: version, ProTx hash, share index, reward script, `inputsHash` (`providertx.h:497-503`) | the share's owner | §5.1's ProUpShareTx rules; the share index is this wallet's share (its owner key); a `Refund` choice is encoded as the share's own refund script, never empty (D31); `inputsHash` is the transaction's |
| ProDisTx unilateral | `MakeSignHash(tx, 1)`: `cs "DashSharedMNDissolve"`, payload version, tx version, tx type, lock time, prevouts, sequences, outputs, ProTx hash, actor index, `u8 1` (`providertx.cpp:506-533`) | the actor's owner; `actorIndex` must be the signing share (`providertx_service.cpp:1267-1268`) | §5.1's ProDisTx rules; the transaction is Core's builder output (§5.4) for this masternode, actor, penalty and fee, recomputed by the wallet from the share table and compared byte for byte; fee at least the relay minimum for its size and at most 1,000,000 duffs; an actor output in (0, dust) is valid and kept by Core's builder (`providertx_service.cpp:674-676`) but won't relay, so the preview warns |
| ProDisTx unanimous | `MakeSignHash(tx, n)` | every owner | §5.1's ProDisTx rules; `SharedMnReadDissolution`'s rule: one input (the collateral), every non-actor gets exactly its principal at its refund, at most one more output, to the actor's refund, fee in (0, 1,000,000] (`sharedmnwidgets.cpp:272-352`); lock time 0, sequence final (D16); **when this wallet holds the actor share of a request it didn't prepare**, the fee it pays from its principal is shown and must be accepted explicitly (`accept_actor_fee`), unless it is at most the relay minimum for the size (m-15; dash-qt's preparer can only choose its own share, `sharedmndialogs.cpp:988-996,1373-1375`, but its reader accepts any actor) |
| ProUpSharedRegTx | `SerializeHash(payload)` without the count and signatures: version, ProTx hash, operator key (basic), voting key id, `inputsHash` (`providertx.h:531-545`) | every owner | §5.1's ProUpSharedRegTx rules; the synced list's keys still equal the session's base (`stale`, M-5, D25) and the list the preview diffed against (r2-m3); the masternode is listed and shared; the user saw every key as current → requested and Core's effects (§3.3, m-12; the preview, MG-04 D41) |

**Own debit (m-1).** At join (participant) or start (coordinator) the wallet records its contribution: its reserved
inputs, their total `I`, and its change `C` (0 when none). At every consent and funding check the transaction must
spend exactly those inputs and pay exactly that change, so the debit is exactly `I − C`; and the recorded figure must
satisfy `I − C ≤ share + (coordinator ? fee : 0) + (C == 0 ? dust − 1 : 0)`. That is Core's `ownContributionError`
bound (`sharedmncreatedialog.cpp:2475-2513`) without its `MAX_EXPECTED_FUNDING_FEE` allowance, which a coordinator
that set its own fee doesn't need; it is exact because the wallet's own inputs are the only values it needs.

What consent does **not** give: a consent signature moves no coin. A party's coins move only with its own funding
signature, which it makes only on the combined transaction it checked. A unanimous dissolution or key change needs
every owner; a unilateral dissolution moves at most the actor's own share to the others (Core caps the bonuses at the
penalty and the fee at 1,000,000 duffs, `specialtxman.cpp:1762-1766`).

## 5. Collateral and share accounting

- **Shares:** 2–8 rows; amounts in duffs, each ≥ 100 DASH and ≤ 1,000 DASH, summing to exactly 1,000 DASH (regular
  only, D4). A coordinator sets labels and amounts; each party fills its own owner, refund and reward.
- **ProRegTx rules** (`dw-protx::shared::rules`), in Core's order and with Core's reasons. Trivial
  (`providertx.cpp:265-333`): `bad-protx-version`, `-evo-version`, `-type`, `-mode`; for a shared payload
  `bad-protx-shares-external`, `-shares-owner-key`, `-shares-payouts`; `bad-protx-key-null`, `-operator-pubkey`;
  the share list (`providertx.cpp:63-130`): `bad-protx-shares-count`, `-early-period`, then per share `-amount`,
  `-key-null`, `-dup-key`, `-dup-refund`, `-payee`, `-payee-dest`, `-payee-reuse` (the voting key, then any share's
  owner key), then `-amount-sum`, `-penalty` (twice: range, and non-zero without a period); then
  `bad-protx-netinfo-version`, the net info's trivial rules, `-netinfo-bad`, `bad-protx-operator-reward`.
  Contextual (`specialtxman.cpp:1148-1321`): `bad-protx-version-disallowed` (1160), `-shares-evo` (1165), the
  network fields (1174-1177), `-collateral-index`/`-shares-collateral-script` (1205-1214), `-dup-netinfo-entry`
  (1239-1244), `-dup-key` (1254-1273), `-inputs-hash` (1290), `-sig` (1304), `-shares-sig` (1314). **SPV can't
  check:** a share owner key already held by a listed masternode (`specialtxman.cpp:1256`; the SML carries no owner
  key ids), except the wallet's own keys (MG-04 D13); everything else above is checked from the payload, the
  transaction and the synced list (service and operator key under either scheme, MG-04 D30). `GetValidatedPayload`'s
  prefix (`bad-protx-type`, `bad-protx-payload`, trailing bytes, `specialtxman.cpp:1098-1121`, `specialtx.h:18-28`)
  is the decoder's: `SpecialTx::from_bytes` and the payload decoders refuse trailing bytes (n-r2-7).
- **Early period and penalty:** blocks, 0..420,480 (UX presets 0, 52,560, 105,120, 210,240, 420,480,
  `sharedmncreatedialog.cpp:628-631`); `0 ≤ penalty <` the smallest share, and 0 without a period. A zero penalty
  carries Core's warning text.
- **Reward split** (for display; Core pays it): the masternode reward less the operator's share once its payout
  address is set, then `SplitAmountByShares`: each share `floor(total × amount / 1,000 DASH)` with 128-bit
  intermediates, the remainder to the last; a zero output omitted (`payments.cpp:162-196`,
  `providertx_util.cpp:88-115`). MG-00 D46's two figures (now and once the operator payout is set) come from this.
  "Reward = refund" is displayed by comparing `RewardScript()` (empty reads as the refund), so a share registered
  with an empty reward and one reset to its refund script read the same (D31).
- **Funding:** the coordinator contributes its share plus the network fee (default 100,000 duffs, editable in draft,
  D9, D33); each participant contributes exactly its share. Each party selects largest-first from its own spendable,
  unlocked, confirmed coins until it covers the target with change 0 or above dust; change below dust goes to the fee
  (`sharedmncreatedialog.cpp:3017-3072`); change goes to a fresh internal address of the wallet (D10).
- **Dissolution** (`BuildDissolution`, `providertx_service.cpp:639-681`): one input, the collateral, empty scriptSig,
  sequence `0xffffffff`, lock time 0, `nVersion 3`, type 10. Outputs: each non-actor in share order, `amount + bonus`
  to its refund script, where the bonuses are `SplitAmountByShares(penalty, non-actors)`; then the actor's `amount −
  penalty − fee` to its refund script, omitted when 0. `penalty` is `nEarlyPenalty` when the dissolution is
  unilateral and early (`height + 1 − registered < period`), else 0; a standby's variants fix it either way. A
  together dissolution has penalty 0 and the chosen actor pays the fee. The unilateral fee's floor is the relay
  minimum for the size (dash-qt's default is 1,000 duffs, `sharedmndialogs.cpp:71-72`).
- **Validity over time:** a ProDisTx valid at some height is valid at every later one: the required penalty is
  non-increasing in the spend height (`specialtxman.cpp:1704-1708`) and both ceilings are height-independent
  (`:1752-1766`). So a penalty-free standby becomes valid at `registered + period` and stays valid; it is spent only
  if the masternode is still there.

### 5.1 Rules for types 10–12 (M-4)

`dw-protx::shared::rules` checks each before the wallet signs or sends, in Core's order, returning Core's reason
(`protx_refused{reason}`). "List" means the synced list at the tip (Core uses `pindexPrev`'s, §3.3); "share table"
the registration's (D29). Signature checks run on what the wallet assembled.

| Type | Trivial (`providertx.cpp`) | Contextual (`specialtxman.cpp`) | SPV can't check |
|---|---|---|---|
| ProDisTx (10) | `bad-prodis-version` (≠ 1, 538); `-sig-count` (0 or > 8, 541) | `-too-early` (v24, 1799); `-hash` (listed, 1810); `-not-shared` (1685); `-input` (one input, the collateral, empty scriptSig, 1691-1692); `-actor` (1695); `-sig-count` (1 or n, 1701); `-payee-count` (1714); per non-actor `-payee` (its refund, 1728) and `-penalty-floor` (`ProRataFloor`, 1734); actor `-payee` (1741), `-actor-output-zero` (1744); `-penalty-sum` (1750); `-fee` (> 1,000,000, 1763); `-bonus` (unilateral, > `nEarlyPenalty`, 1766); `-sig` (1775, 1781) | nothing: the share table, registered height and collateral are immutable after registration (`:1802-1807`) |
| ProUpShareTx (11) | `bad-proupshare-version` (555); `-sig-size` (≠ 65, 558); `-payee` (`scriptReward` not P2PKH/P2SH, **empty included**, 561) | `-too-early` (1573); `-hash` (1579); `-not-shared` (1583); `-index` (1586); `-payee-dest` (1592); `-payee-reuse` (the voting key, 1595; any share's owner key, 1599); `bad-protx-inputs-hash` (1603); `-sig` (1612) | the list's current voting key is read from the SML; a pending key change paying the new reward is a mempool conflict (D23) |
| ProUpSharedRegTx (12) | `bad-proupsharedreg-version` (577); `bad-protx-key-null` (invalid operator or null voting, 580); `bad-protx-operator-pubkey` (legacy, 583); `bad-proupsharedreg-sig-count` (0 or > 8, 586) | `-too-early` (1630); `-hash` (1636); `-not-shared` (1640); `-sig-count` (≠ n, 1645); `bad-protx-dup-key` (the operator key under any scheme, self excluded, 1650-1653); `-payee-reuse` (`IsShareListVotingKeySafe`: no share's refund or reward pays the new voting key, 1655-1656, `providertx_util.cpp:132-145`); `bad-protx-inputs-hash` (1659); `-sig` (every share, 1668) | nothing beyond the list |

A reward change is also re-checked at block apply against the then-current voting key (`specialtxman.cpp:639-640`),
and a key change against the then-current refunds and rewards (`:692-693`): D23 keeps this wallet from racing
itself; another party's concurrent change can still make one of them invalid, and the wallet's row then sits `sent`
(MG-04 D5) until resent or abandoned.

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
  share count of a payload it decodes, nothing else; with `sig_count` 1 the payload's `actorIndex` must be the share
  whose key `index` is (`providertx_service.cpp:1267-1268` signs with `shares[actor_index]`'s key).

None takes a digest. An owner key still signs no transaction input and no message (MG-04 D50). The scope's
`max_duffs` charge stays the engine's (MG-05 §2).

### 6.3 Grants, leases, charges

Every signing call takes one `MasternodeOp{max_duffs}` grant for the acting wallet and begins its own lease
(MG-05 §4): `FlowKind::SharedMasternode` ("Shared masternode") for creation, `update_share` and the key change;
`FlowKind::Dissolution` ("Dissolution") for dissolve now, standby and together (MG-05 D8, MG-00 D35). The lease's
`masternode` budget is charged before anything is signed (D18):

| Call | Charged |
|---|---|
| `lock_shared_terms`, `approve_shared` (consent only), `export_standby` (D21) | 0: no coin of the wallet can move without its funding signature; the export is gated, not charged |
| `sign_shared_funding`, `send_shared_registration` | the wallet's net debit of the registration: its inputs minus outputs paying it (its share; plus the fee for the coordinator) |
| `update_share`, `send_maintenance` of a key change, a resend | the transaction's outflow (MG-05 D4): the fee |
| `dissolve`, `create_standby`, `send_standby`, `start_dissolve_together`, `approve_maintenance` of a dissolution | what the wallet's shares give up: penalty + fee when it is the actor, else 0 (a together dissolution's penalty is 0) |
| `send_maintenance` of a dissolution, its resend | 0: the approvals were charged (D95) |
| `approve_maintenance` of a key change | 0 (the funder pays the fee) |

The registration's charge is MG-05 §2's net debit applied **outside** `TxDraft::prepare`: the shared registration is
not a `TxDraft` (its other outputs, the collateral template and other parties' change, aren't this wallet's), so the
engine computes `I − C` from the session's recorded contribution (§4) and charges it in the same J step that hands
out the funding signer, before signing, with `send.grant_exceeded` when it doesn't fit (D18; the lease's
`Exceeded` as `TxDraft` reports it, `send::lease_failure`).

The fence (E0-04 §5, MG-04 §2): the coordinator's registration send, `update_share`, `dissolve`, `send_standby` and a
complete maintenance send are row-less `CoreTx` artifacts through `PreparedTx::assembled` and `TxDraft::broadcast`,
the row saved durably first and `sent_at` set before each hand-off (MG-04 D6, D7). A resend reserves the same inputs
again and runs under its own lease (MG-07). Inputs that aren't the wallet's coins (other parties' contributions, the
template collateral) are reserved under the same owner too: harmless, and it keeps one path.

## 7. Facade

All on `Masternodes`. Kind as in mn-engine §2. "Slice" is when a row joins the contract (§13, §14). **Every call
that produces bytes the network can accept** (a funding signature, a hand-off, a standby export; in part 2 also the signature that completes a unanimous ProDisTx, which anyone can broadcast, `providertx_service.cpp:1209-1220`; n-r2-6) checks `can_send`
first and refuses `masternodes.unsupported{feature: "shared_masternodes"}` while it is false (D1, M-2); at the
current pin that is every row marked †.

| Call | Kind | Grant | Slice | Does |
|---|---|---|---|---|
| `shared_capability(wallet)` | async | — | 1a | `SharedCapability`: v24 `active` / `inactive` / `unknown` (D3), evonode shares always `inactive` (D4), and `can_send` (`false` through slice 1a; 1b lifts it, D1). |
| `shared_sessions(wallet)` | async | — | 1a | Every `SharedSession` of the wallet (creation and maintenance), newest first, state derived on read. |
| `shared_session(wallet, session)` | async | — | 1a | One `SharedSession`. |
| `start_shared(wallet, request)` | async | — | 1a | Coordinator: checks `StartSharedRequest` (§5, D28), picks keys, reserves share + fee, saves the draft. Returns the session and its Invitation (`SharedMessage`). |
| `set_shared_fee(wallet, session, fee)` | async | — | 1a | Coordinator, `draft` only, and only while the session holds no other party's contribution (else `shared_stage{stage: "replied"}`): replaces its own contribution with one covering share + the new fee (re-reserving its coins; the old locks released in the same write), `revision` + 1 (D33). |
| `import_shared_message(wallet, text, target)` | async | — | 1a | Detects and applies a pasted or opened message (§2.1) by §3.0's table: creates or advances a creation session, a maintenance session, or stores a standby (§9.4). `target`: an optional maintenance session, required for a bare signatures array (§2.5). Returns `SharedImport`. Signs nothing. |
| `join_shared(wallet, session, request)` | async | — | 1a | Participant: fills its row, reserves its share, records its contribution, returns the Details. |
| `shared_message(wallet, session)` | async | — | 1a | The message to copy or save now: text, code, Core's file name (`shared-mn-session-<CODE>.json`). |
| `lock_shared_terms(wallet, session, revision, grant)` | async | `MasternodeOp` | 1a | Coordinator: at the `revision` it reviewed (else `shared_message{conflict}`, D56), builds the registration, §3.1 step 3's checks, signs its consent, returns Locked Terms. |
| `unlock_shared_terms(wallet, session)` | async | — | 1a | Coordinator: back to draft before `combined`. |
| `approve_shared(wallet, session, consent_hash, grant)` | async | `MasternodeOp` | 1a | Participant: of the `consent_hash` it reviewed (else `shared_message{conflict}`, D56), §4's checks, consent signatures, returns the Approval. |
| `sign_shared_funding(wallet, session, grant)` † | async | `MasternodeOp` | 1a (refusing) | Participant: signs its inputs on the combined transaction, saves it with `out_at`, returns the Signed Contribution. In 1a it is on the facade and **always refuses** (`can_send` false); the in-process three-wallet test reaches the signed registration through a test-support capability override, never by removing the check. The override (r2-m4) is an explicit argument of a constructor in `dw-engine`'s `test_support` module, under MG-03 D40's gate `cfg(all(debug_assertions, feature = "test-support"))`: no environment variable, global or config key, and no dwcli command or flag reaches it; a release-profile check proves it absent. |
| `send_shared_registration(wallet, session, grant)` † | async | `MasternodeOp` | 1b | Coordinator: signs its inputs once every other input is signed, saves, hands off; `resend` the same way. |
| `discard_shared_session(wallet, session)` | async | — | 1a | Deletes a session nothing of which may be out (D17) and unlocks its coins, from the stored row (an envelope that no longer parses doesn't block it, D57). |
| `abandon_shared_session(wallet, session, grant)` † | async | `MasternodeOp` | 1b | A session that may be out (`out_at` set, not `confirmed`): MG-04 D49's abandon, spending one of the wallet's reserved inputs to a fresh address of its own; the row is kept, read `conflicted` once that spend is in a block, and its locks are then released (D26). For a funder's `stale` key change the input is a fee input. |
| `preview_update_share(wallet, request)` | async | — | 1a | The ProUpShareTx it would build for one of this wallet's shares, §5.1 run on it. |
| `update_share(wallet, request, grant)` † | async | `MasternodeOp` | 1b | A `ProTxOperation` of kind `update_share`. |
| `preview_dissolve(wallet, request)` | async | — | 2 | `DissolvePreview` (each share's return, penalty, fee, early, valid from, the dust warning). |
| `dissolve(wallet, request, grant)` † | async | `MasternodeOp` | 2 | Unilateral ProDisTx now (kind `dissolve`). |
| `create_standby(wallet, pro_tx_hash, fee, grant)` † | async | `MasternodeOp` | 2 | Signs both standby variants (one when the penalty is 0) for each of this wallet's shares and stores them (D86). |
| `standby(wallet, pro_tx_hash)` | async | — | 2 | The stored standbys' descriptions (no bytes). |
| `export_standby(wallet, pro_tx_hash, share, fee, grant)` † | async | `MasternodeOp{0}` "Dissolution" | 2 | Core's file text and name (§9.4): the signed bytes leave the wallet, so it needs a grant (m-22, D21). `fee` names the standby (D84). |
| `send_standby(wallet, pro_tx_hash, share, fee, variant, grant)` † | async | `MasternodeOp` | 2 | Hands a stored or imported variant to the fence as a `dissolve` operation; m-20's guard (D87, D88). |
| `start_dissolve_together(wallet, request, grant)` † | async | `MasternodeOp` | 2 | Builds the unanimous ProDisTx, signs this wallet's shares, returns the session and its sigs envelope. |
| `start_key_change(wallet, request, grant)` | async | `MasternodeOp` | 2 | Builds and funds the ProUpSharedRegTx (keys kept or set), records the base (D25), signs this wallet's shares, returns the session and envelope. |
| `approve_maintenance(wallet, session, grant, accept_actor_fee)` † | async | `MasternodeOp` | 2 | Signs this wallet's shares of an imported maintenance session; refuses `stale` (D25); `accept_actor_fee` per §4 (m-15). |
| `send_maintenance(wallet, session, grant)` † | async | `MasternodeOp` | 2 | Inserts every signature in share order and hands off (a key change re-signs its fee inputs); refuses `stale`. |
| `operations`, `resend_operation`, `discard_operation` | | | 1b | MG-04's, now also over `update_share` (1b) and `dissolve` (2) rows. |

**dwcli (slices 1a and 1b).** `dwcli mn shared <cmd>` is one facade call each and prints MG-03 §9's JSON line; `--wallet W`
(default: the oldest wallet) goes with every command. The message text is read from stdin, never argv, at most
2 MiB + 1 bytes of it (`too_large` beyond, before the UTF-8 check). `lock` and `approve` take a `MasternodeOp` grant
capped at 0, and the `revision` or `consent_hash` that `mn shared show` printed for the copy reviewed (D56). Slice 1b's
`sign` and `send` cap their grant at the session record's `debit` (its inputs less its change, D9),
`update-share` at its preview's `total_debit`, `abandon` at `--max-fee` (default 1,000 duffs, the minimum relay fee of
its one-input transaction and more, D72).

| Command | Facade call |
|---|---|
| `mn shared capability` | `shared_capability` |
| `mn shared sessions` | `shared_sessions` |
| `mn shared show <session>` | `shared_session` |
| `mn shared start --share LABEL:DUFFS … --service A … --early-period-blocks N --early-penalty DUFFS --fee DUFFS [--operator-key K \| --operator-index N] [--operator-reward BP] [--voting-address A \| --voting-index N] [--node-run-by T] [--reward refund\|new\|ADDR]` | `start_shared` |
| `mn shared fee <session> <fee>` | `set_shared_fee` |
| `mn shared import [--target <session>]` (text on stdin) | `import_shared_message` |
| `mn shared join <session> [--share-index N] [--reward refund\|new\|ADDR]` | `join_shared` |
| `mn shared message <session> [--out FILE]` | `shared_message` |
| `mn shared lock <session> --revision N` / `unlock <session>` | `lock_shared_terms` / `unlock_shared_terms` |
| `mn shared approve <session> --consent-hash H` | `approve_shared` |
| `mn shared sign <session>` | `sign_shared_funding` |
| `mn shared send <session>` | `send_shared_registration` |
| `mn shared abandon <session> [--max-fee DUFFS]` | `abandon_shared_session` |
| `mn shared discard <session>` | `discard_shared_session` |
| `mn shared preview-update-share <hash> --share-index N --reward refund\|new\|ADDR` | `preview_update_share` |
| `mn shared update-share <hash> --share-index N --reward refund\|new\|ADDR` | `update_share` |
| `mn shared preview-dissolve <hash> --share-index N [--fee DUFFS]` (default 1,000, dash-qt's) | `preview_dissolve` |
| `mn shared dissolve <hash> --share-index N [--fee DUFFS]` (a grant at its preview's `total_debit`) | `dissolve` |
| `mn shared create-standby <hash> [--fee DUFFS] [--max-debit DUFFS]` (default grant: the wallet's shares its session names, each the penalty and the fee; 0 without a session) | `create_standby` |
| `mn shared standby <hash>` | `standby` |
| `mn shared export-standby <hash> --share-index N [--fee DUFFS] [--out PATH]` (a grant at 0) | `export_standby` |
| `mn shared send-standby <hash> --share-index N [--fee DUFFS] --variant full\|immediate [--max-debit DUFFS]` (default grant: the fee, plus the penalty its session names for `immediate`; 0 without a session) | `send_standby` |

Refusals, in order, as MG-04 §2.3: arguments (`invalid_argument`; a session id is 64 lower-case hex, creation and
maintenance alike, D28, m-17), `wallet_not_found`, `masternodes.unsupported{feature: "shared_masternodes"}` (v24 not
`active` for creation; `can_send` false for a † call), the list (`list_unavailable`, `shared_too_early`,
`shared_registration_unknown`), the session (`shared_session_not_found`, `in_flight`, `shared_stage{stage}`; then
lock's and approve's `shared_message{conflict}` for a copy other than the one reviewed, D56; then lock's, approve's and
sign's `shared_stage{dead}` for a reservation no longer held, D17, D50), the message (`shared_message{reason}`),
Core's rules (`protx_refused{reason}`), the consent checks (`shared_funding_mismatch{reason}`; at lock the fee
floor's `fee_low` comes before the coordinator's own-side checks, §3.1 step 3), then the lease (`Platform`) and the
send (`send.*`).

**MG-04's registrar calls refuse a shared masternode (m-13, D30).** `preview_update_registrar` and
`update_registrar` refuse `protx_refused{reason: "bad-protx-shared-mn"}` before building when the masternode is
shared (a version-3 ProRegTx with shares in the history, or a session of this wallet): consensus refuses any
ProUpRegTx on one (`specialtxman.cpp:1444-1448`; block apply 534).

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
    Sent, Confirmed { height: u32 }, Conflicted,
    Stale,                                // a key change whose base moved (D25)
    Dead { reason: DeadReason },
}
pub enum DeadReason {
    InputsSpent,                          // one of this wallet's reserved inputs spent elsewhere before out_at
    Delisted,                             // the masternode left the list (maintenance)
}
pub struct ShareRow {
    pub index: u16, pub label: String, pub amount: u64,
    pub owner_address: Option<String>, pub refund_address: Option<String>, pub reward_address: Option<String>,
    pub mine: bool, pub contributed: bool, pub approved: bool, pub funding_signed: bool,
}
pub struct SharedSession {
    pub id: String, pub code: String,     // 64 hex (D28, §3.3); session code (6 upper-case hex)
    pub message_code: String,             // the current envelope's fingerprint, XXXX-XXXX (§2.3)
    pub kind: SharedKind, pub role: SharedRole, pub stage: SharedStage, pub revision: u32,
    pub consent_hash: Option<String>,     // `consentHash` as the envelope shows it, from the locked terms on (D56)
    pub rows: Vec<ShareRow>, pub terms: ShareTermsView, pub fee: Option<u64>,
    pub pro_tx_hash: Option<String>, pub txid: Option<String>,
    pub reserved: Vec<String>,            // this wallet's locked outpoints
    pub next: SharedNext,                 // what this wallet does next (copy, wait, approve, sign, send, nothing)
    pub warnings: Vec<SharedWarning>,     // Core's zero-penalty text, unvalued inputs (D26), empty service (D5), …
    pub approved_terms: Option<ShareTermsView>, // the terms this wallet last approved (M-1 case 2's diff)
    pub base_keys: Option<KeyBase>,       // a key change's base (D25)
    pub actor_fee: Option<u64>,           // a dissolve-together fee this wallet's actor share pays (m-15)
    pub created_at: u64, pub updated_at: u64, pub last_error: Option<String>,
}
pub struct SharedMessage { pub text: String, pub code: String, pub name: String, pub file_name: String }
pub enum SharedImport {
    Session { session: SharedSession, changed: bool,
              received_code: String,      // the received object's recomputed fingerprint (n-2)
              warnings: Vec<SharedWarning> },
    Standby { description: StandbyDescription, stored: bool },   // stored for send_standby once valid (m-19)
}
pub enum SharedWarning {
    ZeroPenalty, EditedMessage,
    UnvaluedInputs { shares: Vec<u16>, count: u32 },   // contributions this wallet can't value (D26)
    EmptyService, ActorOutputDust, PenaltyPaidAfterPeriod,
}
pub struct KeyBase { pub operator_public_key: String, pub voting_address: String }
pub struct DissolveRequest { pub pro_tx_hash: String, pub share_index: u16, pub fee: u64 }
pub struct DissolvePreview {
    pub early: bool, pub penalty: u64, pub fee: u64,
    pub valid_from_height: u32,           // the first penalty-free height (D82)
    pub returns: Vec<ShareReturn>,        // per share: amount back, to which refund address, mine
    pub warnings: Vec<SharedWarning>,     // ActorOutputDust (n-10)
    pub total_debit: u64,                 // penalty + fee (D82)
}
pub struct Standby { pub pro_tx_hash: String, pub share_index: u16, pub fee: u64,
                     pub full_valid_from: u32,
                     pub full: bool, pub immediate: bool,   // the variants stored (one copy at penalty 0)
                     pub source: StandbySource,             // created | imported
                     pub created_at: u64 }                  // the signed bytes stay inside
pub enum StandbyVariant { Full, Immediate }
pub struct StandbyDescription {           // masternodelist.cpp:300-373's description of a pasted one
    pub standby: Standby, pub variant: StandbyVariant, pub txid: String,
    pub penalty: u64, pub returns: Vec<ShareReturn>, pub returns_principal: bool,
}
pub struct StandbyFile { pub file_name: String, pub text: String }   // Core's file (§9.4)
pub struct UpdateShareRequest { pub pro_tx_hash: String, pub share_index: u16, pub reward: RewardChoice }
                                          // Refund = the share's refund script, never empty (D31)
pub struct KeyChangeRequest { pub pro_tx_hash: String,
                              pub operator: KeepOr<OperatorChoice>, pub voting: KeepOr<VotingChoice> }
```

`ProTxOperationKind` gains `UpdateShare` (1b) and `Dissolve` (part 2). Records follow mn-engine §1. No record holds
a secret; a session holds public data only (addresses, outpoints, the public operator key, signatures), as dash-qt's
file does (`sharedmncreatedialog.cpp:2758-2781`).

## 9. Persistence, crash and restart, timeouts, standby

### 9.1 Tables

app.sqlite, migration `V2026102201__mg_shared_sessions.sql`, scoped by wallet and network, deleted with the wallet,
not in `.dwbackup` (D19):

`shared_mn_sessions`: `id` (64 hex), `wallet_id`, `network`, `kind`, `role`, `envelope` (the latest canonical message
JSON, public data only), `my_shares` (JSON: index → keychain owner index), `contribution` (JSON: this wallet's
recorded inputs, their total and its change, §4), `reserved` (the wallet's locked outpoints), `approved_terms`,
`base_keys` (a key change's base, D25), `signed_tx` (this wallet's signed copy, written before it is returned or
handed off), `txid`, `out_at` (set when bytes that may be broadcast leave this wallet: a Signed Contribution
returned, or a hand-off begun), `abandon_txid`, `created_at`, `updated_at`, `last_error`;
`PRIMARY KEY (wallet_id, id)` (m-4: one session per wallet and id); `CHECK (out_at IS NULL OR signed_tx IS NOT
NULL)`.

`shared_mn_standby` (migration `V2026103001__mg_shared_standby.sql`, slice 2b; one row per actor share and fee,
D84): `wallet_id`, `pro_tx_hash`, `share_index`, `fee`, `full_tx`, `immediate_tx` (NULL when the
penalty is 0: one copy, m-20), `full_valid_from`, `source` (`created` or `imported`), `created_at`. **Stored standbys
are plaintext signed transactions** (as Core's file is): anyone who reads app.sqlite can dissolve the masternode with
one. That is why reading them out takes a grant (D21).

`update_share` and `dissolve` operations are rows of MG-04's `masternode_ops` (new `kind` values), with every MG-04
rule unchanged.

### 9.2 Rules

- **The state is derived on read** from the row, the wallet's history and the list (D27): `confirmed` once the txid
  is in a block of the history; `conflicted` once another transaction in a block spends one of **this wallet's**
  inputs of it; `dead{inputs_spent}` once one of this wallet's reserved inputs of a session that is not out is spent
  elsewhere; for maintenance sessions, §3.3's list-derived states. A spend of another party's input is invisible.
- **Save before anything can leave.** Every message that adds this wallet's signature, and every hand-off, is
  preceded by a durable write (`synchronous=FULL`) of the session with the signed bytes and, when the bytes can be
  broadcast by others or the network, `out_at`. A crash between the write and the return leaves a row that is out
  and was never returned: harmless, the coins stay locked and the row is never deleted.
- **A row whose bytes may be out is never deleted:** `DELETE … WHERE out_at IS NULL` (MG-04 D6). Discard refuses
  otherwise (`shared_stage{stage}`). Such a session ends `confirmed` or `conflicted`; the user's exit is §9.3's
  abandon.
- **One call at a time per session** (an in-flight set as MG-04's).
- **Coins:** reserved coins are persistent locks (`lock_manual`) written in the same transaction as the session
  (MG-04 D53); released on discard, on `conflicted`, on `confirmed`, and when `dead`; never released while `out_at`
  is set and the session is open. **A resumed session never reuses a spent reservation:** every call that signs
  re-reads the wallet's reserved inputs and refuses (`shared_stage{stage: "dead"}`, the session `dead{inputs_spent}`)
  if one is no longer an unspent wallet coin locked by this session; nothing re-selects coins after
  `join_shared`/`start_shared`, except the coordinator's draft-only `set_shared_fee` (D17, D33).
- **Restart:** sessions, locks and standbys are reloaded as stored; an in-memory reservation is re-made only by a
  hand-off, from the saved inputs (MG-04 D7).

### 9.3 Timeouts and abandonment

Core has none: no expiry, no timestamps in the envelope; a session is dead only when a contributed coin is spent
(`sharedmncreatedialog.cpp:3226-3246`, `sharedmndialogs.cpp:947-971`). The wallet adds none (D20). A participant can
discard any time before `out_at` (its coins come back, the others start over); a coordinator likewise.

**After `out_at`** a session can be stuck for reasons the wallet can't see (D26, D27): another party's input was
worth less than its share and change (the registration can never be valid), a foreign scriptSig was bad, another
party double-spent its input, or the coordinator never broadcast. The network sends no reject (MG-04 D5), so the
row reads `sent` (coordinator) or `funding_signed` (participant) indefinitely, with this wallet's share (and, for the
coordinator, the fee) locked. **The exit is `abandon_shared_session`** (MG-04 D49's path, D26): it spends one of the
wallet's reserved inputs of the session to a fresh address of its own, under a grant, through the fence; the row is
kept with `abandon_txid`; once either transaction is in a block the session reads `confirmed` (the registration won
the race) or `conflicted`, and only then are the remaining locks released. The UI offers it after the session has
been `sent` for a while; the wallet itself never decides (D20).

### 9.4 Standby

After the registration confirms, every `shared_masternodes` read offers `create_standby` for each share without one
(MG-00 D32's "prepares and stores", gated by a grant, D21). It builds Core's two variants with the same actor and fee
(`createStandby`, `sharedmndialogs.cpp:1634-1668`): **full principal** (penalty 0; valid from `registered + period`)
and **immediate** (penalty paid, `pay_penalty = true` unconditionally, `:1645-1647`; valid now). With a zero penalty
the two are identical and one copy is stored.

- `export_standby` (grant, D21) writes Core's file `standby-dissolution-<first 8 of the ProTx hash>-share<k>.txt`,
  `k` 1-based, with Core's lines ("Share k of n", …) and the two hex lines, ending with `\n`
  (`sharedmndialogs.cpp:1695-1741`); the header lines are English (Core translates them; only the hex lines are read,
  D22).
- **Import (m-19, D32).** Pasting one hex line decodes it and checks it as a standby: type 10, `nVersion` 3; `vin ==
  [collateral]` with an empty scriptSig; the masternode listed and its registration in the history (D29); §5.1's
  ProDisTx rules; its signature verifies; its outputs equal `BuildDissolution` for its actor and fee with penalty 0
  or `nEarlyPenalty` (not `SharedMnReadDissolution`, which flags a valid immediate variant,
  `masternodelist.cpp:346-353`). A standby passing all of it is stored (`source = imported`) and described as
  `masternodelist.cpp:300-373` does; `send_standby` can then send it, as dash-qt offers (`:358-379`). Anything else
  is refused with the failing reason.
- **`send_standby`'s guard (m-20).** Once the full-principal variant is valid (`tip + 1 − registered ≥ period`),
  sending the immediate one would pay the penalty for nothing (overpaying stays valid, `specialtxman.cpp:1704-1708,
  1765-1766`): `send_standby` refuses the immediate variant then (`shared_stage{stage: "full_valid"}`) and sends the
  full one instead on request.

## 10. Errors

New `MasternodesError` variants and §4 codes:

| Variant | Code | When |
|---|---|---|
| `SharedSessionNotFound` | `masternodes.shared_session_not_found` | |
| `SharedStage { stage }` | `masternodes.shared_stage` | the call doesn't apply at the session's stage or role; `stage` names it, including the derived `dead`, `stale`, `full_valid` (n-6: a spent reservation is a session state, reported here) |
| `SharedMessage { reason }` | `masternodes.shared_message` | `not_shared` ("This isn't a shared-masternode message."), `too_large`, `parse` (including a `sessionId` that isn't 64 hex, a sigs `version` ≠ 1, a bare array without a target), `network` (the message's network, never echoing it), `other_session`, `terms_changed`, `older`, `conflict`, `bad_signature{share}`, `not_participant`, `labels` (D28), `stale` (D25), `not_standby` (a type-10 transaction that isn't a decodable ProDisTx with one signature, D89), `outputs` (a valid ProDisTx that isn't `BuildDissolution`'s for its actor and fee, D89) |
| `SharedFundingMismatch { reason }` | `masternodes.shared_funding_mismatch` | a consent or funding check of §4 fails: `foreign_input`, `overdraw`, `row_changed`, `contribution_changed`, `time_lock`, `template`, `join_signature`, `not_own_inputs`, `fee_low{needed}` (§3.1 step 3) |
| `ShareNotMine` | `masternodes.share_not_mine` | a maintenance call for a share whose owner key this wallet lacks |
| `SharedTooEarly` | `masternodes.shared_too_early` | a maintenance call with `tip ≤ registered_height` (§3.3) |
| `SharedRegistrationUnknown` | `masternodes.shared_registration_unknown` | the registration ProRegTx isn't in the wallet's history (D29) |

`masternodes.unsupported{feature: "shared_masternodes"}` now also covers v24 not `active` (creation) and every †
call of §7 while `can_send` is false (D1, D3); `protx_refused{reason}` carries Core's shared reasons (§5, §5.1) and
MG-04's `bad-protx-shared-mn` (D30). `invalid_argument`, `send.*`, `Platform` as MG-04. No message or code repeats
pasted text: a refused paste names only the reason (MG-00 D36; mn-engine D23). New §4 codes need Swift `engineCodes`
and the Swift T0 suite (dw-common).

## 11. Events

`EngineEvent::Masternodes { network }` after every session or standby change, as MG-04's operations. The follower
pass derives `confirmed`/`conflicted`/`dead`/`stale` and releases settled sessions' locks.

## 12. Tests

**T0, `dw-protx`:**
- the v3 shared ProRegTx, `ExtNetInfo` (IPv4, IPv6, the D5 rules), consent digest, `SplitAmountByShares`, the
  dissolution builder, canonical signatures (high S, header aliases);
- the rules module over **all four payload types** (shared ProRegTx, ProDisTx, ProUpShareTx, ProUpSharedRegTx):
  dashd's payloads pass, and each mutated payload returns dashd's reason in Core's order (§5, §5.1), including a
  ProUpShareTx with an empty `scriptReward` (`bad-proupshare-payee`);
- the JSON reader and writer against UniValue's own output (D7), the test checking the vectors' structure too (r2-m5):
  the exact case count the generator writes, `fingerprint` present iff the case parsed to an object and isn't
  read-only, `ints` iff it parsed to an array and isn't read-only, `read_only` only on the generator's deep cases, and
  a committed corrupted-vector test that fails for a value and for a structural corruption; inputs include escaped,
  control-character and non-ASCII keys (n-r2-9); the fingerprint, envelope round trips, every
  `fromJson` refusal (§2.3, n-1's list), the sigs-envelope reader rules (§2.5), `absorbDraftReply`,
  `adoptLockedTerms` and `mergeEnvelope` cases (the dash-qt test cases of `mnsharesessiontests.cpp`), the H-1 and H-8
  refusals;
- an 8-share session whose seven Details arrive in any order, with equal and lower revisions than the coordinator's
  (r2-m1), a re-pasted reply (no bump, n-r2-3), and a reply to an Invitation from before `set_shared_fee` (refused
  `older`, D33);
- **every cell of §3.0's table**, including M-1's three cases: unlock → new invitation (back to `joined`, same
  reservation, Details unchanged), unlock → re-lock with unchanged terms (fresh approval required, no diff), unlock →
  re-lock with a changed penalty (fresh approval, the diff shows the penalty);
- `tests/oracle_shared.rs` over dashd vectors (`gen_shared_vectors.py`, rc.3, regtest with v24 by `-vbparams`): for a
  three-party registration built by `shared_register_prepare` from a funding template the wallet's code made, the
  payload, consent digest and each `shared_sign` signature equal the wallet's; `shared_combine`'s bytes equal the
  wallet's combine; `shared_update_share`, `shared_update_registrar_prepare`, `shared_dissolve` (both `payPenalty`)
  and `shared_dissolve_prepare` payloads, digests and signatures equal the wallet's on dashd's own transactions
  (RFC 6979 makes the signatures comparable, as MG-04's oracle does).

**T0, `dw-vault`:** each new signer signs only under `MasternodeOwner` at an owner path, refuses `Full` and every
other scope, hashes what it is given (no digest entry point); `sign_dissolution` refuses `sig_count ∉ {1, n}` and a
count of 1 for another share's `actorIndex`.

**T0, `dw-engine`:** every call's refusals in order; **`sign_shared_funding` and every † call refuse at the pin**
(`can_send` false, M-2); a three-wallet session end to end in one process (coordinator and two participants, messages
passed as text) up to the signed registration, through the test-support capability override; a dash-qt participant
simulated by dashd's `shared_sign` output; restarts at each stage (locks kept, nothing re-selected, `out_at` rows not
deletable), with a crash injected between the `out_at` write and the return; the charges; the in-flight mark; the
key-change base (`stale`); MG-04's registrar calls refusing a shared masternode; the imported standby's checks and
m-20's guard; the contract test.

**T1, `regtest/functional/dwd_shared.py`** (after BL-187): three wallets (dwcli) create a shared masternode from a
v24 regtest (dashd `-vbparams=v24:0:999999999999:0:10:8:6:5:0`, `rpc.md` §11), one changes its reward address (and
one resets it to its refund), `protx info` shows the share table, the operator key rotated together and revived by
dashd's `update_service`; then dissolved alone in the early period (penalty paid), and in a second run together; a
standby sent once valid. Two mixed sessions: dashd's wallet as one participant through `shared_register_prepare` /
`shared_sign`; and **dashd as the coordinator** (its RPCs follow `runCombineAndSign`'s order), so the wallet
participant's funding signature completes the transaction (D24, D17).

**Mutants** (a scratch script applies each and runs the suites; each must fail a test). The brief's six and the
doc's first three:
1. a share below the minimum accepted;
2. a party's payout (refund or reward) swapped;
3. a signature over the wrong hash (consent without the sequences; ProUpShareTx with the signature; ProDisTx with
   the wrong `sig_count`);
4. a dissolution combined with fewer signatures than shares;
5. operator signing attempted (a BLS secret or an operator-signed builder: the guard);
6. a resumed session reusing a spent reservation;
7. the `overdraw` check removed or loosened (m-1's exact debit);
8. H-1: a newer draft replacing a locked session past `signing`;
9. D16: a time lock signed.

Review r1's fifteen:
10. `adoptLockedTerms`' own-row/own-contribution check removed: a participant approves after its refund, reward or
    change was altered;
11. D15/H-8: a reply filling a second row accepted;
12. `payloadMatchesEnvelope`'s output loop removed: a change output redirected;
13. D12's join-signature check at `combined` removed;
14. `signedOnlyOwnInputs` removed, or the funding sighash type changed from `SIGHASH_ALL`;
15. the `foreign_input` check removed: a wallet coin smuggled into another contribution;
16. `SharedMnReadDissolution`'s "every non-actor exactly its principal" removed;
17. `sign_dissolution` accepting `sig_count ∉ {1, n}`, or a count of 1 with `actorIndex ≠ index`;
18. `out_at` written after the message is returned (the crash-injection test), and `DELETE` without `WHERE out_at IS
    NULL`;
19. the network check removed from envelope import;
20. M-2: `sign_shared_funding` callable while `can_send` is false;
21. M-4: `RewardChoice::Refund` encoded as an empty script in a ProUpShareTx;
22. M-5: a key-change approve or send after the list moved off the base;
23. m-13: MG-04's ProUpRegTx offered on a shared masternode;
24. M-1: a participant adopting a re-locked copy whose own row changed.

Review r2's nine (r2-m6, DEC-312):
25. M-1 case 2: a re-locked copy approved without a fresh `approve_shared`, or the old approval carried over or
    re-signed silently;
26. M-1 case 1: a newer draft adopted keeping the old consent signature, or re-selecting coins;
27. m-7: an incoming scriptSig taken on one of this wallet's own inputs;
28. m-15: `approve_maintenance` as the actor of a request this wallet didn't prepare, without `accept_actor_fee`, for
    a fee above the relay minimum;
29. m-20: `send_standby` sending the immediate variant once the full one is valid;
30. D32: an imported standby stored without the `BuildDissolution` output comparison, or with a non-collateral input;
31. D26/§9.3: `abandon_shared_session` releasing locks or deleting the row before a block settles it;
32. D33/r2-m2: `set_shared_fee` outside `draft`, or after another party's contribution is in the session;
33. D12: `tx.nVersion`, the payload version or `inputsHash` unchecked before consent.

The mutant list lives in the scratch script with, for each, the test that kills it. Each READY report lists the
mutants whose code is in that slice with their killing test (1a: the creation and envelope paths, and 20's release
check; 1b: 18 and 31 on the coordinator's send and the abandon, with the fence, hold and release paths of D68–D73;
20's release check stays with the switch, D71); part 2's lists all 33. Of 1b's 30 scratch mutants, three are defense in
depth no reachable state exercises, so no test kills them: the held-coin filter behind the lock in `set_shared_fee`'s
re-selection (a held coin stays locked, D69), and the fences in `sign_shared_funding` and `approve_shared` (every
writer of a reservation or its lock is fenced, D68, D69). The held coins in `protx_context`'s collateral set (mutant 10)
guard a reachable state: an existing collateral may be a coin the user locked, so that set alone keeps an MG-04
registration from naming a session's reserved coin, existing or external; `test_QT_126_a_reserved_coin_is_no_mg04_collateral`
kills it (review r1 of 1b, Opus M-1).

## 13. Slices

| Part | Slice | Content | Needs | Checkpoint |
|---|---|---|---|---|
| — | 0 | This document and the contract delta (§14) | — | design review r2 |
| 1 | 1a | `dw-protx`: codecs, rules (§5, §5.1), JSON and envelopes, session merge logic (§3.0); vault signers; the oracle vectors (UniValue, dashd) and `oracle_shared.rs`; engine: capability, sessions, persistence, `start_shared`, `set_shared_fee`, `import_shared_message`, `join_shared`, `shared_message`, `lock/unlock_shared_terms`, `approve_shared`, `sign_shared_funding` (**refusing**: `can_send` false), `discard_shared_session`, `preview_update_share`; MG-04's registrar refusal (D30); dwcli `mn shared start|fee|import|join|message|lock|unlock|approve|discard|sessions|show` (no `sign` until 1b); the in-process three-wallet test through the test-support override | — (nothing in 1a produces bytes the network accepts) | READY part 1a |
| 1 | 1b | `sign_shared_funding` enabled, `send_shared_registration`, `abandon_shared_session`, `update_share`, resend; the follower's shared sessions; dwcli `sign|send|abandon|update-share`; regtest creation (both mixed sessions) and reward change | **BL-187 (+ #1136)**: the hand-off calls behind the switch, off (DEC-678); slice 1c (D39, M9) turned it on (D71) | READY part 1 |
| 2 | 2 | `preview_dissolve`, `dissolve`, standby (create, export, import, send), dissolve together, key change; regtest create → reward change → dissolve; all 33 mutants | BL-187 | READY part 2 |

Slice 1a is reviewable and mergeable on its own (DEC-310, DEC-311): the engine reports `can_send: false`, the only
call on its facade that would produce a broadcastable signature (`sign_shared_funding`) refuses, the hand-off calls
aren't on the facade yet, and nothing reaches a user (MG-10b's UI waits for part 1). A 1a wallet can take part in a
session up to its consent signature, which moves no coin (§4).

## 14. Contract delta (mn-engine.md)

Each version lists only the calls it ships (the contract test fails an orphan §2 row, n-5):

| Version | Slice | §2.4 rows | Other sections |
|---|---|---|---|
| 5 → 6 | 1a | `shared_capability`, `shared_sessions`, `shared_session`, `start_shared`, `set_shared_fee`, `import_shared_message`, `join_shared`, `shared_message`, `lock_shared_terms`, `unlock_shared_terms`, `approve_shared`, `sign_shared_funding` (refuses while `can_send` is false), `discard_shared_session`, `preview_update_share` | §0: `shared.rs`, `shared_records.rs` (facade), `shared/` (not facade); §1 Keys: these calls sign with keychain owner keys only, through the vault's shared signers (D11), under a `MasternodeOp` grant; the attached owner key stays ProUpRegTx-only; nothing signs as operator; §2.3: MG-04's registrar calls refuse a shared masternode (D30); §3 regenerated; §4: §10's codes; §5: `SharedStage`, `SharedCapability`, the session records; §6: the event after a session change; §7: `shared_mn_sessions`, its locks |
| 6 → 7 | 1b | `send_shared_registration`, `abandon_shared_session`, `update_share`; `operations`/`resend_operation`/`discard_operation` over `update_share` | §5: `ProTxOperationKind::UpdateShare`; `shared_unsupported` narrowed: a shared masternode the wallet holds a session or a share key for reads as shared (`you hold k of n`; until then MG-03 D25 stands) |
| 7 → 8 | 2 | `preview_dissolve`, `dissolve`, `create_standby`, `standby`, `export_standby`, `send_standby`, `start_dissolve_together`, `start_key_change`, `approve_maintenance`, `send_maintenance` | §5: `ProTxOperationKind::Dissolve`, the standby and maintenance records; §6: the event after a standby change; §7: `shared_mn_standby` |

MG-05's FlowKind gains `SharedMasternode` (1a) and `Dissolution` (2) (MG-05 D8). `events.ts` and `docs/parity.md`
(QT-126, QT-127 to `partial` with 1a, `done` with part 2) are touched minimally (siblings in flight).

## D-rows

| D | Decision | Why |
|---|---|---|
| D1 | **The hand-off and the regtest acceptance wait for BL-187**, extended by #1136: a graph-wide `[patch]` to a rust-dashcore with #1136, #1138, #1152 and #1156 on top of `d3d50520a` (or platform's next pin). Until then `shared_capability().can_send` is `false`; every call that produces bytes the network accepts refuses while it is: `sign_shared_funding` (on the facade from 1a, refusing), and the hand-off calls (`send_shared_registration`, `abandon_shared_session`, `update_share`, `dissolve`, `export_standby`, `send_standby`, `send_maintenance`), which aren't on the facade before their slice. The row is ready only as slice 1a. | §0: the pin can neither represent nor decode a version-3 ProRegTx or types 10–12, every hand-off needs a typed `dashcore::Transaction` (`send/mod.rs` `PreparedTx::assembled`), and the wallet would stall on the first block holding one (MG-04 D48, MG-03 D25, BL-187). The probe showed #1138 needs #1136. Hand-rolled broadcasting of raw bytes would still leave the sync stalled. DEC-09 makes the pin move the manager's. **DEC-310 (10-21):** none of the four is in the desktop pin nor in platform v5.0-dev's (d631aaca, v0.45.0), so the bump is a platform-fork branch, a separate pin task the manager staffs; part 1 ends at slice 1a until it lands. **DEC-311 (M-2):** a participant's Signed Contribution is broadcastable by whoever holds the rest (H-9), and a dash-qt coordinator signs its own inputs at combine (`sharedmncreatedialog.cpp:2576-2591`), so the wallet's may be the last signature; testnet's v24 window opened 2026-09-24 (`chainparams.cpp:421-428`), and a mined version-3 ProRegTx spending the wallet's inputs would stall its sync (MG-04 D48). So `sign_shared_funding` is gated like the hand-offs; consent (`approve_shared`) moves nothing and stays. **The test-only override (r2-m4, DEC-312)** lives only in `dw-engine::test_support`, compiled under MG-03 D40's `cfg(all(debug_assertions, feature = "test-support"))` (`dw-engine/src/lib.rs:48-53`), and is an explicit per-engine constructor argument; dwcli enables `test-support` for its own tests but has no command or flag that reaches it, and the gate builds `dw-engine` with `--release --features test-support` and runs a `cfg(not(debug_assertions))` test asserting `can_send` is false. BL-187 merged (534bb7c7, DEC-338): the pin now represents and decodes all of it, and `can_send` stays false through 1a; slice 1b lifts it with M3 and the hand-off calls. |
| D2 | Codecs stay `dw-protx`'s own (raw `Encodable` structs and byte builders), as the type 10–12 ones already are, also after BL-187; the engine converts to dashcore's types only at the hand-off. | The oracle compares bytes the wallet writes itself, independent of the dashcore revision; after the pin moves, a unit test compares `dw-protx`'s bytes with dashcore's encoding of the same transaction. |
| D3 | On every network v24 is `active` once the synced list holds a version-3 entry or a version-3 provider transaction is known (MG-03's `any_shared_seen`), else `unknown`; `inactive` is never inferred from chain parameters (m-14). Creation needs `active`. The regtest harness lets dashd register one version-3 masternode first. Mainnet's `NEVER_ACTIVE` (`chainparams.cpp:213`, marked `// TODO`) is kept only as a pin-tied test that mainnet reads `unknown` at the rc.3 pin. | SPV sees no deployment state; a version-3 entry is valid only after v24 (`specialtxman.cpp:1112-1116,1158-1161`), and an EHF deployment never deactivates, so it proves activation on any network, including a mainnet release that sets a start time. A registration sent before activation is refused silently (MG-04 D5) and holds coins until abandoned, so `unknown` doesn't create. MG-00 D33: the engine reports, the UI never assumes. |
| D4 | Regular masternodes only. A shared evonode is refused (`invalid_argument`), and `evonode_shares` reads `inactive` everywhere; the codecs still read and hash evonode payloads (the platform node id in the consent digest, rc.3's change) for a masternode created elsewhere. | `evo_shares` is `NEVER_ACTIVE` on mainnet (`chainparams.cpp:222`), and no masternode signs it at rc.3 (`llmq/ehf_signals.cpp:53-55`), so it can't activate on any EHF network; only a regtest with `useehf=0` has it. UX-SPEC §4.25.8 already disables Evonode. Accepted, DEC-310. |
| D5 | The coordinator's service is required and IPv4 (`a.b.c.d:port`, one or more, MG-04 D17); as a participant the wallet accepts a dash-qt coordinator's empty, IPv4 or IPv6 `coreP2PAddrs` (in any form `AddEntry` reads, D78), and refuses Tor, I2P, CJDNS or a domain (`unsupported{feature: "shared_masternodes", field: "service"}`). Core's v3 `ExtNetInfo` rules, applied to both: at most 4 entries per purpose (`netinfo.h:20`); addresses distinct even across ports (`netinfo.cpp:437-439,609-611`); port ≠ 0 and not `IsBadPort`, with **no** mainnet-9999 rule for v3 (`:463-475`); routable on mainnet and testnet (`RequireRoutableExternalIP`, `:460-462`); an fc00::/8 IPv6 is flipped to CJDNS before validation (`MaybeFlipIPv6toCJDNS`, `:543`), so the wallet flips first and then refuses it as CJDNS; entries keep insertion order, so the bytes follow `coreP2PAddrs`' order. An empty service is legal (`specialtxman.cpp:1172-1177`) but the masternode then starts PoSe-banned (`:433-436`): the participant's session carries `EmptyService`. | It must encode the net info to check the payload (`payloadMatchesEnvelope`); IPv6 is ADDRv2's simple case. The rules are what makes a payload the wallet assembles valid; the order rule is what makes it byte-equal to dash-qt's. |
| D6 | The session protocol is dash-qt's v1 exactly: both envelopes, key order, stages, merge rules, `payloadMatchesEnvelope`, the funding template, the standby hex and file; the wallet adds stricter checks (D12, D15, D16) but no field. | Interop with dash-qt (the brief). Any extra key would change the code dash-qt displays (H-12), and a different structure would be a protocol Core doesn't define. |
| D7 | `dw-protx::shared::json`: an own reader and writer reproducing UniValue's model (ordered keys, duplicates kept, first match on lookup, number tokens kept) and `write(2)` byte for byte. Its oracle is rc.3's `src/univalue` compiled with a short driver that prints `write(2)` and the fingerprint of each input. The driver's source, its inputs, the vectors it produced and a README on regenerating them from rc.3 are committed under `testdata/oracle/univalue/`; tests and CI read only the vectors, so CI builds no C++ (DEC-310). | The message code depends on the exact rendering, including unknown keys and number tokens (`mnsharesession.cpp:225-232`). `serde_json` with `preserve_order` and `arbitrary_precision` would change number handling for every crate in the graph (feature unification). Core's own writer is the only authority on its output. |
| D8 | `fundingTx` from a message is never trusted: the wallet rebuilds it from `contributions` and compares; a coordinator prepares from its rebuild. | dash-qt doesn't validate it on import (H-7). The consent digest binds the transaction, and the rebuild is what the parties agreed in `contributions`. |
| D9 | **The coordinator pays the whole registration fee; each participant contributes exactly its share** (dust change goes to the fee). The fee is set by the coordinator (default 100,000 duffs, Core's), at least the relay minimum for the final size. This **replaces MG-00 D41's fee split with a per-participant maximum.** | dash-qt's model (`sharedmncreatedialog.cpp:81,3010-3015`; H-18). The envelope has no field for a split or a maximum, and an added key breaks interop (D6). A participant can't be made to pay a part it can't see in the terms. The fee is small (0.001 DASH default). MG-10b's copy changes: the invitation says the coordinator pays the network fee. Accepted, DEC-310. The fee floor is computed from an estimated size (foreign inputs as P2PKH) and can't prove the registration's real fee (D26). |
| D10 | Keys per party: the share owner key is a keychain owner key (lowest free index 0–19, MG-04 D13, sessions counting as in use); the refund is a fresh BIP44 receive address; the reward defaults to none (the refund gets rewards); change goes to a fresh internal address; the coordinator's voting key is its keychain voting key unless given. A wallet starts or joins with one share per session. | Owner keys at keychain paths make the share visible to MG-03's role scan and recoverable from the seed (a standby can be signed again after a restore). A funds-account refund keeps the principal in the balance (MG-04 D55). One share per wallet keeps "my row" unambiguous, as `inferMyShare` assumes; a wallet holding several (another device of the seed, a dash-qt import) still signs for each. |
| D11 | Owner signatures come from vault methods that take the transaction or payload and compute Core's digest themselves (§6.2), at keychain owner paths, `KeyUse::OwnerPayload`, `MasternodeOwner` only. The engine verifies each returned signature (canonical, recovers to the share's key) before using it. | MG-04 D11's argument: a digest signer at an owner key would sign anything, and owner keys are the shared masternode's authority (consent, dissolution, reward, keys). MG-12 reviews these signers. The check catches a vault or key mismatch before a message leaves. |
| D12 | Stricter than dash-qt before signing: `tx.nVersion` 3, payload version 3, `inputsHash`, and at `combined` every join signature verified against the consent digest (and equal to `sigs`). | dash-qt leaves these to consensus (H-6). A party that signs its inputs on a transaction that can't be valid gives nothing away, but checking costs nothing and gives the user an error instead of a silent refusal (MG-04 D5). |
| D13 | Only the coordinator sends the registration; a participant's copy is never handed off by the wallet. | dash-qt's rule (`sharedmncreatedialog.cpp:2908-2912`). The participant still treats its signed copy as may-be-out (D17): anyone holding it can broadcast (H-9). |
| D14 | The wallet writes `prepareWallet` as `""` and infers its role from its own session row, not from names. | dash-qt writes the coordinator's wallet name (`sharedmncreatedialog.cpp:2218`), which leaks a local name to every participant; it uses it only to recognise its own file (`coordinatedHere`, `:3360-3376`). The wallet's sessions are in app.sqlite, so it needs no name. A dash-qt coordinator reading the wallet's envelope sees "" and decides by role as for any foreign file. |
| D15 | Stricter merges, by §3.0's table: a reply fills at most one row, the row its one new contribution names (H-8). A **coordinator** past `draft` refuses every higher-revision import (`shared_message{conflict}`, H-1). A **participant** at `to_approve`/`approved` with `out_at` unset takes a higher revision only when its own row and contribution are unchanged: a new draft returns it to `joined` with the same reservation and Details; a re-locked copy returns it to `to_approve` with the terms treated as new, the diff against its last approval shown, and only a fresh approval signs (M-1, DEC-311). Anything higher-revision at `combined` or later, or after `out_at`, is `conflict`. As a participant the wallet echoes `revision` unchanged in Approvals and Signed Contributions. | dash-qt's fill-empty-only lets any reply fill any row, and its revision-first merge lets a late draft replace locked terms (`mnsharesession.cpp:1238-1257,1361-1377`; `replaceSession`, `sharedmncreatedialog.cpp:2858-2918,2952-2953`). But that replace is also dash-qt's only unlock → re-lock path (`unfreeze`, `mnsharesession.cpp:925-935`): refusing it outright stranded a participant, whose re-join with a fresh refund `absorbDraftReply` refuses (`:1245-1252`). Re-adopting with the participant's own row fixed, and never past `signing`, keeps H-1's protection: nothing of the participant's that it signed changes without its new approval. dash-qt replies satisfy all of it, so interop holds. |
| D16 | The wallet never signs a registration, dissolution or key change with a lock time other than 0 or any sequence other than `0xffffffff`; its own transactions use those values. That is deliberately stricter than Core's `HasTimeLock` (`providertx_service.cpp:683-705`, which allows non-final sequences that set no relative lock), and Core applies no lock check to a key change at all (`:1145-1154`). | dash-qt never sets `allowTimeLocks` (`:1100-1109`), and both digests commit to the lock fields, so a lock a co-signer missed would delay the exit. Core's funding template and dissolution builder use these values (`mnsharesession.h:108-113`, `providertx_service.cpp:661-664`), and dash-qt always writes `0xffffffff` (`mnsharesession.h:112`), so the stricter rule loses no interop. |
| D17 | A session's `out_at` is set, durably, **before** a Signed Contribution is returned (participant) or a hand-off begins (coordinator, maintenance). From then on the session is never deleted and its reserved coins stay locked until `confirmed` or `conflicted`. Every signing call re-checks that each reserved input is still an unspent wallet coin locked by this session, and refuses otherwise; no call re-selects coins after the reservation, except the coordinator's `set_shared_fee` in `draft` (D33). The user's exit from an out session is `abandon_shared_session` (D26). | MG-07's lesson: once bytes may be out, deleting the row or freeing the coins lets a second spend race the first. A participant can't know whether the coordinator broadcast. A resumed session that re-selected coins could sign a second transaction over coins already committed (the brief's mutant). |
| D18 | Charges: consent signatures charge 0; funding signatures charge the wallet's net debit of the registration; maintenance charges its outflow (the fee), and a dissolution what the wallet's shares give up (penalty + fee when it is the actor). | MG-05 D4 charges outflow. A consent signature moves nothing without the party's funding signature. The registration moves the share out of the wallet to the template (not the wallet's), so it is outflow. A dissolution pays the wallet back; what it costs the wallet is the penalty and the fee from its own share. The registration's figure is MG-05 §2's net debit computed outside `TxDraft::prepare` (the registration isn't a `TxDraft`) from the recorded contribution, and charged in the J step that hands out the funding signer (§6.3). |
| D19 | Sessions and standbys are not carried by `.dwbackup`. | MG-04 D24's reasons: a row is this installation's working state and its locks. A restored wallet re-derives its owner keys and can sign a new standby while the masternode exists, once its filters watch the owner key ids (D29); an exported standby imports and sends without the session (D32). Core keeps standbys in a separate file for the same reason; export is the backup (§9.4). |
| D20 | No timeouts. A session lives until sent, discarded or dead. | Core has none (`sharedmncreatedialog.cpp:3226-3246`); a timeout would release coins another party may already have counted on. MG-00 D32: the user keeps or discards. |
| D21 | The standby is prepared, not silently: after confirmation the session reads `next: create_standby`, and `create_standby` needs a grant (the owner key signs). Stored standbys are plaintext signed transactions in app.sqlite (§9.1); `export_standby` hands out the bytes only under a `MasternodeOp{0}` "Dissolution" grant, charging nothing; `standby` returns descriptions, never bytes (m-22). With a zero penalty one copy is stored, and `send_standby` refuses the immediate variant once the full one is valid (m-20). | It signs with the owner key and commits up to penalty + fee (the immediate variant), so it is a signing step under MG-05. The immediate variant dissolves the masternode for every party at any time, so a renderer not trusted to sign mustn't read it either. Core builds the immediate variant with the penalty unconditionally and overpaying stays valid (`sharedmndialogs.cpp:1645-1647`, `specialtxman.cpp:1704-1708,1765-1766`), hence the guard. MG-00 D32's "prepares and stores" is kept as a prompt the UI shows right after confirmation. |
| D22 | The standby file's header lines are English; only the two hex lines are read on import, and a paste must be one hex line, as in Core. | Core translates the header (`tr()`, `sharedmndialogs.cpp:1702-1728`) and never parses it (`sharedmnwidgets.cpp:388-396`). |
| D23 | Two shared operations of one masternode can't be out at once from this wallet: a key change or a reward change waits while another `update_share`, key change or dissolution of it is `signed`, `not_sent` or `sent`, or in a block not yet buried (D80) (`masternodes.update_pending{operation}`), as MG-04 D49. | Core's mempool allows several ProUpShareTx of one share (the last mined wins, `specialtxman.cpp:620-648`) and conflicts a key change with a pending reward change paying the new voting key (`txmempool.cpp:1737-1786`). One at a time keeps "what the user asked last" what is mined. |
| D24 | **The wallet's coordinator signs its own funding inputs last**, in `send_shared_registration`, after every participant's; its Signing Request carries no coordinator scriptSig. As a participant under a dash-qt coordinator, the wallet's signature may complete the transaction, so its `out_at` (D17) is written before the Signed Contribution is returned. A deliberate deviation from dash-qt (m-2). | dash-qt's `runCombineAndSign` signs the coordinator's inputs right after combining (`sharedmncreatedialog.cpp:2576-2591`), so the last participant to sign holds a broadcastable transaction (H-9). Signing last means only the coordinator can complete, which enforces D13 structurally. It is interop-safe: dash-qt merges funding signatures per input in any order (`mnsharesession.cpp:1426-1463`). The T1 mixed session with dashd as coordinator covers the other order. |
| D25 | **A key change binds its base.** The session stores the list's `(pubKeyOperator, keyIDVoting)` when it is prepared or first imported; `approve_maintenance` and `send_maintenance` refuse once the synced list differs (`shared_message{stale}`), the session reads `stale`, and its funder is offered `abandon_shared_session` on a fee input. Every approver's preview shows each key as current → requested against the list at approval time, Core's effects (operator reset and PoSe ban, basic-scheme key, one operator change in the mempool) and that a signed request stays usable by its funder until a fee input is spent (M-5, DEC-311). The base protects against the list moving **after** import; the diff protects against a request built on an older state (r2-m3, DEC-312). | The payload carries absolute keys, "keep" copies the current ones (`providertx_service.cpp:1345-1353`), the owners sign only the ProTx hash, keys and `inputsHash` (`specialtxman.cpp:1663-1669`), and consensus applies the keys as given, resetting and banning on an operator change (`:663-673`). A complete request sent after a later change would revert it. `inputsHash` (the funder's fee inputs, `providertx.h:527`) is the only thing that voids it, so the funder's abandon is the real cancel; the base check stops this wallet from approving or sending into a reverted state. dash-qt shows the ban only to the preparer (`sharedmndialogs.cpp:1805-1806`). |
| D26 | **The wallet can't value other parties' inputs**, so it checks its own side exactly (§4, m-1), floors the fee on an estimated size with foreign inputs counted as P2PKH (a floor only when they are P2PKH, as Dash Core wallets write; a P2SH input makes it low, n-r2-10), and carries Core's non-blocking warning naming the contributions it can't value (`UnvaluedInputs`, as `fundingCheck` does with unresolved inputs, `sharedmncreatedialog.cpp:3298-3305`). It does not claim the "fee positive" check. A session stuck after `out_at` has a recorded exit, `abandon_shared_session`: it spends one of the wallet's reserved inputs to itself through the fence, keeps the row (`abandon_txid`), and releases the locks only once a block settles it (`confirmed` or `conflicted`); it never deletes a row that may be out (MG-04 D49, MG-06/MG-07) (M-3, DEC-311). | `fundingCheck` values every input through the node's UTXO set (`resolveInputValue`, `m_node.getUnspentOutput`, `:3259-3292`); an SPV wallet knows its own coins only, and the consent digest covers prevouts, not amounts. A party contributing less than share + change makes the registration invalid; with no P2P reject (MG-04 D5) the coordinator's row would sit `sent` holding its share and fee. Resolving foreign prevouts through DAPI is a follow-up. |
| D27 | **States come from what SPV sees.** `conflicted` and `dead` mean a spend of **this wallet's** inputs; another party's double-spend is invisible (the session stays `sent` until abandoned, D26). A key change's state for a non-funder comes from the synced list (`confirmed` when the keys equal the requested ones, `stale` when they move elsewhere, `dead` when the masternode leaves); a dissolution's from the collateral leaving the list (m-6). | BIP158 filters match scripts, not outpoints, and the wallet's filter holds its own scripts only. dash-qt's creation liveness is wallet-coin-only too (`sharedmncreatedialog.cpp:3226-3246`); its maintenance liveness asks the UTXO set (`sharedmndialogs.cpp:947-971`), which the wallet doesn't have. A non-funding approver of a key change can't see its fee inputs or the transaction. |
| D28 | **Session ids and labels.** A `sessionId` must be 64 lower-case hex, else `shared_message{parse}` (refused, not lower-cased: dash-qt compares ids exactly, `mnsharesession.cpp:1206`, so a lower-cased re-emission would be another session to it; n-r2-4); `(wallet, sessionId)` is the table's key; a maintenance session's id is SHA-256 of (kind, ProTx hash, sign hash), 64 hex (§3.3). On every import and in `start_shared` labels are non-empty and unique after trimming, every contribution's label names a share, and a share has at most one contribution (`shared_message{labels}`) (m-4, m-5, m-17). | dash-qt accepts any non-empty id (`mnsharesession.cpp:399-403`) but always writes `GetRandHash().ToString()` (`:253`), so the rule is interop-safe, keeps every session reachable by the facade's 64-hex argument, and keeps the session code (`left(6).toUpper()`, `mnsharesession.h:166`) from echoing pasted free text (MG-00 D36, mn-engine D23). Labels bind contributions to shares (first match, `mnsharesession.cpp:91-97,1274-1278`); dash-qt's coordinator enforces unique labels in its UI only (`sharedmncreatedialog.cpp:1276-1288`), and D15's "the row its one new contribution names" needs them unambiguous. A maintenance id from the sign hash is fixed while the txid changes on combine and re-sign (`providertx_service.cpp:1209-1237`). |
| D29 | **The share table comes from the registration in the wallet's history**, with the registered height from its block; maintenance is refused (`shared_registration_unknown`) when it isn't there. D19's restore claim ("a restored wallet can sign a new standby") holds once the wallet watches its owner key ids 0–19 in its filters, which is added to BL-187/MG-03b's scope (m-11). | The SML has no shares, early terms or registered height (`evo/simplifiedmns.h:40`, mem-only). Filters do index each share's refund, reward and owner key id (`evo/specialtx_filter.cpp:80-85`), so watching the owner key ids finds the registration after a restore. |
| D30 | **MG-04's registrar calls refuse a shared masternode** (`protx_refused{reason: "bad-protx-shared-mn"}`) before building (m-13). | Consensus refuses any ProUpRegTx on one (`specialtxman.cpp:1444-1448`; block apply `:534`). A coordinator holds the registration among its own operations, so MG-04's `update_registrar` would otherwise offer one that sits `sent` forever. |
| D31 | **"Reward = refund" in a ProUpShareTx is the share's refund script**, never an empty script; reading a share, an empty `scriptReward` and one equal to the refund both display "rewards go to the refund address" (`RewardScript()`) (M-4, DEC-311). | A ProUpShareTx's `scriptReward` must be P2PKH/P2SH, so empty is `bad-proupshare-payee` (`providertx.cpp:560-562`); Core's own comment says "use the refund script to reset rewards" (`providertx.h:493`). At registration an empty reward means the refund (`providertx.cpp:96`), which is why the two encodings differ. |
| D32 | **An imported standby is checked and stored for sending**, not just described: type 10, `nVersion` 3, `vin == [collateral]` with an empty scriptSig, the masternode listed and its registration known, §5.1's rules, its signature verified, its outputs equal to `BuildDissolution` with penalty 0 or `nEarlyPenalty` (m-19). | Core offers to broadcast a pasted standby (`masternodelist.cpp:358-379`) and promises the file "works even if this wallet is lost" (`sharedmndialogs.cpp:1564-1565`); D19 makes the export the backup, so the import must be able to use it. `SharedMnReadDissolution` flags a valid immediate variant (`masternodelist.cpp:346-353`), so the comparison is with the builder. |
| D33 | **The coordinator can change its fee in `draft` until another party's contribution is in the session** (`set_shared_fee`; r2-m2 option (a)): its contribution is replaced and its coins re-reserved in one write, `revision` + 1 (the wallet's own +1; dash-qt's remove-and-add bumps twice). After that, `shared_stage{stage: "replied"}`; a `fee_low{needed}` at lock then means discard and start again. A Details reply whose copy of the coordinator's contribution isn't the current one (it answered an Invitation from before the change) is refused `shared_message{older}`, and that participant joins again from the current Invitation. The start-time default is Core's 100,000 duffs, which covers the relay minimum of an 8-share registration with 64 P2PKH inputs per party (about 77 kB at 1,000 duffs/kB), so the change is rarely needed. `lock_shared_terms` computes the minimum fee for the final size and refuses `fee_low{needed}` when the reserved fee is below it (m-8). | The final size is known only at lock. A dash-qt participant's copy keeps the coordinator's contribution from the Invitation it answered: `absorbDraftReply` skips a contribution whose label it already holds (`mnsharesession.cpp:1258-1261`), and `adoptLockedTerms` compares **every** contribution of its copy with the locked one (`:1339-1346`, `SameContribution` `:107-118`), refusing "The locked terms do not contain the details you sent." (`:1300-1301`). A fee change after a reply would strand that participant, and a re-join can't help it once the coordinator holds its old row (`:1245-1252`). So the change is allowed only while nobody else has replied into this copy, and an old reply is refused rather than absorbed, so no stranded party enters the session; the refusal is stricter than dash-qt, which absorbs it. Nothing is out and no one has consented in `draft`, so re-selecting is safe under D17. |
| D34 | **A session envelope string that isn't UTF-8 is refused** `shared_message{parse}`. | dash-qt reads with `QString::fromStdString`, which substitutes U+FFFD and would re-emit different bytes (a different message code). dash-qt only writes UTF-8 (`toStdString`), so nothing it sends is refused. |
| D35 | **`fromJson`'s past-`draft` checks run inside the reader, in dash-qt's order** (`mnsharesession.cpp:612-655`): the transaction and consent hash present, `decodeFrozen`, `payloadMatchesEnvelope`, every signature's share in range, from `combined` a verifying signature per share, from `fundingSigned` every input signed. A failure is `shared_message{parse}`, except a `combined` approval that doesn't verify: `shared_message{bad_signature{share}}`. | dash-qt drops such a message as unreadable; the wallet names the one failure a user can act on (which party's approval is wrong). |
| D36 | **Addresses decode as base58 P2PKH or P2SH on the wallet's network only** (`DecodeDestination` on `Params()`); surrounding whitespace isn't stripped. | Core's base58 decoder skips leading and trailing whitespace; dash-qt never writes it, so the stricter reading refuses only text dash-qt doesn't produce. |
| D37 | **`NetInfoMatches` takes numeric `ip:port` entries only**: no address twice whatever the port (`IsAddrDuplicate`), at most 4; a domain or privacy-network entry never matches. | D5 refuses those services when building; a session naming one can't be one the wallet built or would consent to. |
| D38 | **Funding inputs are signed over `SpecialTx::signature_hash_all`**: legacy `SIGHASH_ALL` over the transaction including its extra payload (`interpreter.cpp:1437-1438`), with the scriptSig `<DER+0x01> <pubkey>`. | The pin's `Transaction` can't carry a version-3 payload, so its sighash would sign a different message. `oracle_shared.rs` verifies dashd's own funding signatures against this hash. |
| D39 | **`bad-protx-dup-netinfo-entry` compares every entry of the registration with every net-info entry of every listed masternode on another collateral, of every purpose (slice 1c).** platform-wallet's `MasternodeListSummary.net_info` carries the list entry's whole `MasternodeNetInfo` (platform fork dc0c946307, D75), and `ListedMasternode.net_info` (`dw_protx::rules::listed_net_info`) every entry of it. Entries are compared by the bytes Core hashes for a unique property (`GetUniquePropertyHash`, rc.3 `deterministicmns.h:526-547`, an untagged `SerializeHash`; `NetInfoEntry::unique_property`): a service as its `CService`, ADDRv1 for IPv4 and IPv6 (`netaddress.h:329-361`; an IPv4-mapped IPv6 address is its IPv4 one, a legacy IPv6 address is never flipped to CJDNS) and ADDRv2 for the others (`:381-395`), then the port big-endian (`:563-567`); a domain as its `DomainPort`, a compact-size string and the port big-endian (`evo/netinfo.h:176-180`). Services and domains share the one namespace: an IPv6 service and a 15-character domain can be the same 18 bytes, and Core refuses that as a duplicate (`[f61:6263:6465:6667:6869:6a2e:6b6c:6d6e]:19999` is `abcdefghij.klmn:19999`; review r1 of 1c, n-3). `check_registration` (version 3, MG-04b) and `check_registration_list` (version 2, MG-04) both use it. A summary read back from platform-wallet's tracked snapshot carries no entries; its primary service stands in. | Core treats every entry of every masternode as a unique property (rc.3 `deterministicmns.cpp:459-470`, `specialtxman.cpp:1235-1245`). Slices 1a and 1b compared the primary Core P2P address only, because the pin's summary kept nothing else; 1b sent nothing with the switch off (DEC-678). |
| D40 | **`lock_shared_terms` before every row and contribution is in refuses `shared_stage{stage: "incomplete"}`.** | The registration can't be built (`BuildFundingParts` needs every contribution); a named stage tells the coordinator what is missing, where `invalid_argument` would not. |
| D41 | **A call made by the wrong role refuses `shared_stage{stage: "coordinator"|"participant"}`**, naming the role the call needs. | §7 gives each call one role; the stage code already carries a word the UI shows, so no new code is needed. |
| D42 | **The owner, voting and operator keys of every shared session count as in use in `protx_context`**, so MG-04's registrations skip them as well as D10's sessions. | Core refuses a reused owner, voting or operator key (`bad-protx-dup-key`, `-payee-reuse`); a key held by a session that later registers must not go to a second registration. |
| D43 | **`join_shared` without `share_index` takes the single open row**; with two or more open it refuses `invalid_argument`, and naming a filled row is `invalid_argument`. A row already filled with this wallet's key is never adopted. | dash-qt's participant picks its row by label; the engine needs it named once the choice is ambiguous. Adopting a filled row would let a coordinator's copy claim this wallet's consent for a row it never joined. |
| D44 | **The coordinator refuses a whole import carrying a bad approval, or a second approval for a share, as `shared_message{bad_signature, share}`.** | dash-qt only warns and keeps the rest (`addSignature`); refusing keeps a combined registration from ever holding a signature that doesn't verify, and the party can resend. Stricter than dash-qt, never weaker. |
| D45 | **In 1a a sigs envelope, a standby and a bare signature array with a target refuse `unsupported{feature: "shared_masternodes"}`**; a bare array without a target is `shared_message{parse}`. Narrowed, not removed: a standby is imported since 2b (D89), a `dissolve` envelope and a targeted bare array since 2c (D92, D97); a `registrar` envelope still refuses until the key change (2d). | They apply to maintenance (part 2); there is no session in 1a they could apply to. |
| D46 | **`shared_message` carries `share` (for `bad_signature`) and `shared_funding_mismatch` carries `needed` (for `fee_low`)** as optional fields. | §10's `bad_signature{share}` and `fee_low{needed}` need a place in the code's shape; one optional field each keeps the code set unchanged. |
| D47 | **A participant's `txid` column holds the txid of its own signed copy.** A session's ProTx hash and its `confirmed` state come from the complete envelope, or from the history transaction that spends the frozen template's inputs in order to its outputs; the payload isn't compared. | The participant never sees the coordinator's final scriptSigs before broadcast, so its own copy's txid differs from the ProTx hash (scriptSigs are in the txid). The pin can't decode a version-3 payload from history (BL-187), so inputs and outputs identify it. |
| D48 | **Locked Terms taken from `joined` (and M-1's case 2) drop this wallet's own approval and any that doesn't verify; `own_approval` verifies the signature, not just its presence.** | A Locked Terms message never carries approvals (it is sent before any); an approval that comes with one can't be this wallet's real consent, so it is never taken as such. |
| D49 | **The in-flight mark is claimed before the session row is read**, as MG-04's `claim_operation`; under concurrency `in_flight` can come before `shared_session_not_found`. | Claim-then-read means no two calls act on one row read; the order of the two refusals only differs for a racing caller. |
| D50 | **D17's re-check also needs each input in this session's `reserved` list and still persistently locked by it**, else `shared_stage{stage: "dead"}`. | An input unlocked by another path (a crash between writes, a manual unlock) could be spent elsewhere; signing over it would race that spend. |
| D51 | **Charges: lock and approve charge 0; signing charges the debit `I − C`, refunded when signing fails or the `out_at` write fails or reports no row.** | D18; consent moves no coins. A charge is spent only for bytes that may be out. |
| D52 | **`shared_sessions` skips a row whose session doesn't parse, with a warning log; `shared_session` on that id still reports its error.** | One unreadable row (a downgrade, a bug) must not hide every other session; the direct read keeps it visible. |
| D53 | **`start_shared` refuses fewer than `MIN_SHARES` (2) rows as `invalid_argument`**, before Core's `bad-protx-shares-count` would apply. | A one-row table isn't a shared masternode; a request-shape error is the caller's, not Core's. |
| D54 | **The follow pass releases the coin locks of `confirmed`, `conflicted` and `dead` sessions.** | D17 keeps them until then; after that the coins are spent or the session can't spend them, and kept locks would hide spendable coins. |
| D55 | **A joined participant importing a re-fee'd Invitation at the same revision gets `conflict`**; per D33 it discards and joins again. | D33 refuses the stale reply at the coordinator; the participant's side sees two different Invitations at one revision, which §3.0 calls a conflict. |
| D56 | **Consent binds to the copy reviewed: `lock_shared_terms` takes the `revision` and `approve_shared` the `consent_hash` the caller read from the session record (`SharedSession.revision`, `.consent_hash`); another value now is `shared_message{conflict}`.** It is checked right after the session's role and stage (for lock, before `incomplete`; for approve, before an approved session's Approval is returned again), then the reservation (D17, D50), before any message, rule or consent check. | Every session call claims the in-flight mark, so an import absorbed between the user's review and the call (a reply bumping the revision, M-1 case 2's re-lock moving the digest) would otherwise get a signature over a table or terms the user never saw. The record already shows both values, and dash-qt's dialog only signs what it displays. The consent hash binds what is signed: a re-lock changing only envelope-side text (`operatorSecretHolder`) keeps it, which is harmless (r2 nit 3); it is 64 hex, compared ignoring case. |
| D57 | **`discard_shared_session` reads `out_at` from the stored row**: unset, the row is deleted and its `reserved` unlocked without parsing the envelope; set, it refuses `shared_stage` with the derived stage, or, when the envelope no longer parses, `sent` (coordinator) or `funding_signed` (participant), the stage an out row with nothing in a block derives. | A row this version can't read (a downgrade, a bug) must not hold its coins locked for ever; D17 only needs `out_at`, which the appdb's delete already checks. |
| D58 | **`payloadMatchesEnvelope`'s operator key is Core's `pubKeyOperator.Get().ToString()`**: the key's hex only when Core reads it as valid (a G1 point, not infinity, re-serializing to the same bytes); any other payload key reads as 48 zero bytes, so terms naming its own bytes don't match. | rc.3 `CBLSLazyWrapper::Get` returns its invalid object for such bytes, whose `ToString` is zeros (`bls/bls.h:125-131,492-511`). Terms of 96 zeros would match it, as in Core; Core's rules then refuse the key (`bad-protx-key-null`) before any signature. |
| D59 | **`NetInfoMatches` runs `AddEntry`'s per-entry checks on the session's network** (`ValidateService`: a valid address of a known network, routable except on regtest, a non-zero port not in `IsBadPort`), and splits `coreP2PAddrs` on commas and ASCII HT, LF, VT, FF, CR and space only. | Core adds each entry through `ExtNetInfo::AddEntry` (rc.3 `evo/netinfo.cpp:500-551`); Qt's `[,\s]+` has no `UseUnicodePropertiesOption`, so PCRE2's `\s` is the ASCII set and a no-break space is part of an entry. |
| D60 | **The follower's pass skips a session another call holds in flight, re-reads the others under the mark, and releases a session's locks only while its `reserved` is still the value it judged** (`release_shared_session_locks_durable`'s `expected`). | Without the mark the pass could unlock coins a concurrent `set_shared_fee` had just re-reserved; the conditional write also covers a writer that doesn't take the mark. A row changed since the pass listed it (a call re-reserved it, possibly onto coins newer than the pass's chain snapshot) waits for the next pass too (review r2 m1). While the pass holds a session's mark, a user call on it gets `in_flight` and retries (r2 m2). |
| D61 | **Every v24 shared-masternode signature (join, consent, ProUpShareTx, ProUpSharedRegTx, ProDisTx) is checked as `CHashSigner::VerifyHashCanonical`** (rc.3 `messagesigner.cpp:58-88`): 65 bytes, header 27–34, low S, then `RecoverCompact`'s key. `shared::rules::verify_hash_canonical` recovers the key itself and never goes through `rules::signed_by`, which stays Core's legacy `VerifyHash` for the ProRegTx/ProUpRegTx owner checks (BL-187 review). | `RecoverCompact` masks the header, so headers past 34 recover the same key, and `(r, n − s)` with the other parity does too; Core refuses both on these types, so a wallet that took them would hold, merge or combine a signature no node accepts. `oracle_shared.rs`'s `test_QT_126_shared_signatures_are_checked_canonically` refuses the high-S and header forms of dashd's own signatures on every path with Core's reason and passes the originals. |
| D62 | **dw-protx's shared codecs and the BL-187 pin's read dashd's transactions alike.** `tests/dashcore_shared.rs` decodes every transaction of `shared_registration.json` and `shared_maintenance.json` (templates, combined and final registrations, maintenance transactions, their mutations) with both: the same bytes back, the same txid, dashcore's payload re-encoded to the bytes dw-protx decoded, and every field equal (`ProviderRegistrationPayload` v3 with its net-info entries, payouts and `SharedCollateral`, `ProviderDissolvePayload`, `ProviderUpdateSharePayload`, `ProviderUpdateSharedRegistrarPayload`). The codecs stay the wallet's own (D2); 1b converts a `SpecialTx` to a `dashcore::Transaction` by its bytes at the hand-off. | D2's planned cross-check, now that the pin decodes these types (BL-187 D14). Where the two overlap they agree on all 19 vector transactions and all four payload types. |
| D63 | **`start_shared` and `join_shared` hold the wallet's coin fence from `protx_context` through the session row's write**; `shared_reserve` takes that fence as an argument, and `set_shared_fee` takes it the same way. Two creation calls of one wallet at once (start + start, join + join, start + join) serialize, so the second reads the first one's keys and coins in use. The tests start the second call while the first is held at a test-only pause point just before its write (`pause_shared_write`, Opus-h r2 NIT-1), so taking the fence after the keys or dropping it before the write fails them. | Review r1 (Opus-h MAJ-1) reproduced two concurrent `start_shared` calls taking the same owner key (`pick_owner` reads a `used` set from before any guard), which 1b would later lock, fund and send into `bad-protx-dup-key`. `issue_address` doesn't take the fence, so holding it across the key and address picks can't deadlock. MG-04's `register` has the same window; its fix is a separate row (Follow-ups). |
| D64 | **A session whose envelope no longer parses still holds the keychain owner keys its `my_shares` column names** (`{share: owner_index}`, which the engine alone writes); `shared_keys_in_use` counts them. An index that names no keychain owner key (past 19) holds nothing and fails nothing, since `protx_context` reads this for every masternode call. Its terms' voting and operator keys can't be read without the envelope. | Review r1 MIN-2: skipping the row (D52) freed its owner key for the next registration while its coins stayed locked. |
| D65 | **`sign_shared_funding` signs only a copy whose consent hash is the one this wallet approved**, compared after §4's checks; another is `shared_message{reason: "conflict"}`. | Review r1 NIT-1: the join-signature re-check already binds it, indirectly (13c was equivalent); the explicit compare states D56's rule at the funding step and survives later changes to how approvals merge. |
| D66 | **The rules' operator-key check is Core's `CheckMalleable` too**: the 48 bytes must decode to a G1 point under the payload's scheme and serialize back to the same bytes (rc.3 `bls/bls.h:211-219,492-511`), for MG-04's ProRegTx/ProUpRegTx rules, the version-3 ProRegTx and type 12 (§5). | Review r1 Sol nit 2. The legacy reader ignores the first byte's unused flag bits: the generator's legacy key with `0x20` or `0x40` set decodes to the same point and was accepted (a probe found 44 such single-byte variants of it, none in the basic serialization the shared types use). |
| D67 | **`ExtNetInfo`'s decoder reads purpose and entry counts as Core's map and vector readers do** (up to `MAX_SIZE`, nothing reserved ahead of the bytes), so five entries in a purpose or a fourth, unknown purpose decode and `validate` gives Core's `bad-protx-netinfo-maxlimit` or `-bad`. Purposes out of order or repeated, and entries Core's reader would `Clear()`, still fail as `bad-protx-payload`; an unknown ProRegTx version stops at the decoder (Core's `bad-protx-version`), as the wallet reads version-3 shared messages only. | Review r1 Sol nit 1. Core's own vectors (`src/test/evo_deterministicmns_tests.cpp:833-918`) are the tests, and a five-entry registration round-trips its payload bytes to the rules. |
| D68 | **MG-04's `register`, `prepare_external_registration` and `update_registrar` take the coin fence before `protx_context` (BL-352)**, as D63 does for `start_shared` and `join_shared`: the keys and coins are picked, and the row written, under one fence. The lease begins under it; its wait is bounded by the vault-lock drain. `refresh_external_registration` releases the old inputs under the fence too. Tests hold the first call at `pause_row_write`, just before its row write: a second registration, external preparation or update waits and takes other keys and coins. | Before, two calls could both read the same unused keys and coins before either wrote its row (the window D63 closed for shared sessions). The fence isn't reentrant, so it is taken once per call, outside `protx_context`. |
| D69 | **Owned locks (M3, BL-346): a coin is held by the masternode row that stored it**, a shared session's `reserved` or an MG-04 operation's `fee_inputs` (`dw-appdb::holds`, one query over both tables; a column that isn't a JSON array of strings holds nothing). A row's own release (`release_lock`, `Holder::Session`/`Holder::Operation`) skips a coin another row holds. The user's `unlock_outpoints` refuses a held coin (`invalid_argument`, naming no outpoint). `protx_context` counts held coins as used, and a session's reservation and its re-check (`reserved_inputs`) treat a coin another session holds as not this session's (`dead`, D17), so a reservation is exclusive across sessions. A masternode's collateral is not a hold: MG-04 D9's release stays the user's. | Before, each release unlocked whatever it named, so a discarded session could free a coin an operation still spends, and a manual unlock could free a reserved one. The stored columns are the truth a restart reads, so a row whose envelope no longer parses still holds its coins (as D64 for keys). |
| D70 | **A conflicted out session releases its coins only once the conflict is buried (NIT-4)**: the conflicting block at least `RECOMMENDED_CONFIRMATIONS` (6) deep, or at or below the ChainLock height. `confirmed` and `dead` release at once. A row that doesn't parse is judged by its stored columns: released when its stored txid is confirmed; before `out_at`, when a reserved coin is no longer unspent; once out, by the same buried-conflict rule. MG-04's operations, `update_share`'s among them, follow the same rule: `settle_operation_locks` releases a conflicted operation's fee inputs only once the conflicting spend is buried (`NetworkSession::buried`, shared with the session follower; review r1 of 1b, n-5). | A shallow conflict may be reorged out, and the registration mined after all; releasing early would let a new send double-spend a coin the network may still see in the registration. Six blocks is the wallet's settled depth (`history::RECOMMENDED_CONFIRMATIONS`); a ChainLock is final. |
| D71 | **Sending (slice 1b): the hand-off calls are on the facade, behind one switch that is off (DEC-678).** `SHARED_SENDING` (`masternodes/shared.rs`) stayed false until slice 1c closed D39 (D75) and M9 (D78); 1c's last commit turned it on, and the switch stays so that turning sending off is one line. While it is off, `can_send` is false and `sign_shared_funding`, `send_shared_registration`, `abandon_shared_session` and `update_share` refuse `unsupported{feature: "shared_masternodes"}` right after the wallet check, reading, writing and charging nothing. `resend_operation` of an `update_share` row (D73) refuses the same, after reading the row whose kind decides it (after the id's `invalid_argument`, `in_flight` and `operation_not_found`), before any lease or charge (r2 of 1b, n-1). 1a's test-support override (`test_support::shared_masternodes_with_send_override`, compiled only in a debug `test-support` build) and `tests/mg04b_release_check.rs` stay; since 1c the engine tests send through the shipped facade and use the override to turn sending off. `sign_shared_funding` and `send_shared_registration` hold the fence from the reservation's re-check through the `out_at` write and the hand-off. The coordinator signs last: `send_shared_registration` refuses `shared_stage{stage: "unsigned"}` while a participant's input is unsigned (D24; the precedent is `incomplete`). It signs its inputs, checks the bytes re-serialize as a typed `dashcore::Transaction` unchanged (D62), writes `out_at` durably with the signed copy, then hands off through MG-04's path (`TxDraft::assembled`, `broadcast_inner`). A failed hand-off keeps `out_at` (the bytes may be out) and records its code in `last_error`. Calling it again once `sent` resends the stored bytes, charged the debit again (a first hand-off that never left was released and refunded; the fence refunds the charge when the bytes may be out already), after checking no other pending spend holds one of their coins; once abandoned it refuses `shared_stage{stage: "abandoned"}` (D72). Fees as D9: the coordinator's debit is its share and the whole fee, a participant's exactly its share (dust change given up to the fee), and `SharedSession.debit` reports it so a grant can be sized. | D1, DEC-338: BL-187's pin represents and decodes every shared transaction. dash-qt's coordinator also signs at combine (`sharedmncreatedialog.cpp:2576-2591`). Clearing `out_at` after a failed hand-off would let a discard free coins a relayed copy spends (D17). The resend is charged the debit because a released first attempt moved nothing: the hand-off that moves the share is then the resend (validator r1 of 1b, M1). |
| D72 | **`abandon_shared_session` spends the largest unspent reserved coin of an out session that no other pending spend holds back to the wallet** (one input, a fresh change address, the relay-minimum fee, refused below dust), so the registration can no longer be mined. `abandon_txid` is written durably first. The row and its locks stay until the follower settles it (D70), and the registration is never handed off again (`shared_stage{stage: "abandoned"}`), even when the abandon's own hand-off was refused outright and so nothing of it is out: that is intended, as the user's exit stands and the user can simply abandon again (review r1 of 1b, n-6). With no such coin the call refuses `shared_stage` naming the caller's out stage (`sent` or `funding_signed`, n-2). A coordinator whose registration the wallet recorded must abandon that transaction first (QT-091, MG-04 D49). `confirmed` and `conflicted` sessions are refused. dwcli's `abandon` caps its grant at `--max-fee` (default 1,000 duffs). | dash-qt has no abandon. A coin the session reserved is the only spend that certainly conflicts with every copy of the registration. One input keeps the fee minimal. Keeping the row lets the follower release the rest once the abandon is buried. |
| D73 | **`update_share` is an MG-04 operation of kind `update_share`** (`ProTxOperationKind::UpdateShare`): planned under the fence (the update lock first), its payload signed by the share's keychain owner key (`sign_update_share`) and its fee inputs by the wallet's, under one `SharedMasternode` lease charged the fee, saved with its input locks (D69), and handed off as MG-04's rows are. `operations`, `resend_operation` and `discard_operation` cover it, and a resend begins a `SharedMasternode` lease. One at a time per masternode: `update_pending{operation}` while another `update_share` of it is `signed`, `not_sent` or `sent` (D23). It claims no key (Core's mempool indexes no key of a ProUpShareTx). | The operation table already gives a signed transaction its states, resend, discard and lock release (MG-04 D6, D7, D25). A second store would duplicate them. |
| D74 | **`shared_unsupported` is narrowed for a masternode the wallet's confirmed sessions registered.** `MasternodeRow.shares` reads `HeldShares{held, of}` ("you hold k of n"): `held` is the wallet's confirmed sessions naming it (one share each), `of` the session's share count. The masternode is `mine` and appears in the overview, and its payouts stay `shared_unsupported` (MG-03 D25) until MG-03b reads the share table. A share key without its session (a restore) stays MG-03 D25's until MG-03b. | §13's narrowing. The session holds the share table the list summary lacks (D29). Per-share payout attribution needs the registration's payout scripts read from the chain, which is MG-03b. |
| D75 | **D39's follow-up is closed by a platform-wallet change and a pin move (slice 1c, DEC-678).** Until 1b the pinned platform-wallet's `MasternodeListSummary` kept only the primary Core P2P endpoint, and its SPV runtime exposes no raw list. The fork branch `dw/bl187-v24-dashcore` adds `MasternodeListSummary.net_info` (dc0c946307, not persisted by its tracked snapshots), and the desktop pins it. | No invented data. Had a clash been missed, Core would have refused it at the hand-off (`last_error`) and the coordinator could abandon (D72): the abandon's relay fee, not funds. |
| D76 | **A session's ProTx hash is its registration in the wallet's history first, the envelope's complete txid second** (`session_hash`): the transaction spending the frozen template's inputs, in order, to its outputs, whatever its scriptSigs (`ChainView::registration`; of several copies, `copy_rank`'s best: one in a ChainLocked block, else one in a block, the sent copy among those first, then the lowest block, then the lowest txid). The stage (`confirmed` at that copy's height, `conflict_height` excepting it), the follower's release, `shared_confirmed` (so `update_share`) and `held_shares` all key on it, the hash the list shows. | A copy mined with another valid scriptSig on any input (a party re-signing with another nonce, or a miner) has another txid than the Final Record's. Keyed on the envelope's txid, the coordinator read `conflicted`, lost the masternode from `held_shares` and was refused its reward change, while the participants, who match by inputs and outputs (D47), read `confirmed` (review r1 of 1b, Opus m-2). Two copies in blocks without a ChainLock mean a reorg left one record stale; the lowest block would let that stale record move a registration the list shows under the sent txid, so the sent copy ranks first among them (validator r1 of the fix round, NIT-4). |
| D77 | **`copy_rank`'s evidence is the wallet's history alone; with two copies in blocks and no ChainLock, the sent copy first is a guess** (r2 of 1b, n-3). A ChainLocked copy is final and ranks first; otherwise which of two in-block records is stale depends on which branch won, which the history doesn't say, so the envelope's complete registration (the copy the coordinator handed off, what it keyed on before D76) is taken over the lower block; a party whose envelope holds no complete registration falls back to the lowest block, then the lowest txid. The synced masternode list is better evidence: a copy whose txid the list holds is the ProTx hash. Preferring it in `shared_confirmed` and `held_shares` is a follow-up, not done here. | Funds aren't at stake: either copy reads `confirmed` and spends the same inputs, so the release is right whichever is picked, and holds come from the stored columns, which D76 doesn't touch. A wrong pick costs `update_share` naming a hash the list doesn't hold: Core refuses it at the hand-off and the lease refunds the charge. A ChainLock, seconds on Dash, settles the pick. |
| D78 | **The session envelope is proven against dash-qt's own `MnShareSession` (M9, slice 1c); `coreP2PAddrs` is a Core-exact read, the coordinator's input strict.** `testdata/oracle/dashqt` links rc.3's `qt/mnsharesession.cpp` unmodified. It replays the two dashd registrations of `shared_registration.json` step by step (terms, each `addContribution`, `freeze`, each `addSignature`, combine, funding signed, broadcast). It also runs `fromJson` on 220 texts, each with one change. `tests/oracle_envelopes.rs` requires `Session::to_text` and `message_code` byte-equal to `toJsonString`/`fingerprint` at every step, every text read back as the session that wrote it, and `fromJson`'s verdict on every corpus text (its `read_back` and fingerprint check when it reads one). The wallet refuses only D28's texts that dash-qt reads (an id that isn't 64 lower-case hex; a label blank, repeated after trimming or naming no share). The replay drives the wallet's own code for the funding transaction and each signature (`rebuild_funding_tx`, `add_signature`); the freeze, combine, funding-signed and broadcast states it sets by hand (stage, `protx`) and then compares, so the engine's `build::freeze`, `build::combine` and its broadcast transition, and dash-qt's merge rules (`mergeEnvelope`, `absorbDraftReply`, `adoptLockedTerms`, `replaceProTx`, `mnsharesession.cpp:1169-1497`), are checked by reading, not by the oracle (review r1 of 1c, n-4). Core-exact read: `coreP2PAddrs` is read as rc.3's `ExtNetInfo::AddEntry` reads it (`evo/netinfo.cpp:500-551`; `netinfo::parse_core_service`). That is `SplitHostPort` (`util/strencodings.cpp:101-126`, the port by `ParseUInt16`, `:234-270`), then the `SAFE_CHARS_IPV4_6` filter (`evo/netinfo.cpp:28,512`). Then `Lookup` without name lookups (`evo/netinfo.cpp:542`; `netbase.cpp:194-209`) splits the host again and resolves it through `LookupIntern` and `WrappedGetAddrInfo` (`netbase.cpp:147-174`, `:44-59`, `AI_NUMERICHOST`): glibc's `inet_aton_exact` (`127.1`, `2130706433`, octal parts), else an IPv6 address, an IPv4-mapped one read as IPv4 (`SetLegacyIPv6`, `netaddress.cpp:135-162`). That reader serves the frozen check (`net_info_matches`), a participant's D5 check and the placeholder's net info. The reading is target-independent: a pure-Rust port of glibc's, the same on every OS the wallet builds for, so there is no libc gate, which would only refuse valid participants (review r1 of 1c, Sol m-1 partly upheld, DEC-743). `testdata/oracle/glibc` checks it against glibc 2.43's `getaddrinfo` under a line-by-line port of `AddEntry`'s IP path: 300,075 entries, 0 mismatches. The fail-closed point is the coordinator's libc: v24 defines no text form beyond `AddEntry`, which asks the platform's `getaddrinfo`, so a dash-qt coordinator on another libc may read an unusual form otherwise and freeze another address. The participant's frozen check refuses that payload (`check_frozen`, `shared_message{reason: "parse"}`) before anything is signed, and the share it reserved at join is released by discarding the session (D17). Because a form like `8.8.8.010` reads as 8.8.8.8, the participant's terms view carries Core's reading beside the text (`ShareTermsView.service_read`, when an entry reads other than as written), and consent shows the address the registration holds. Coordinator input strict: this wallet's own service stays `a.b.c.d:port` (D5). Tests: the 39 forms in the corpus, `test_QT_126_services_parse_as_core_reads_them`, `test_QT_126_a_participant_joins_a_core_form_service` (its `service_read`), `test_QT_126_a_frozen_service_read_otherwise_is_refused_and_releases_the_share`. `check_registration` gives dashd's reason for the six mutated join signatures. | Review r1 of 1a (M9 and its nit on `SocketAddr`). dash-qt's verdicts decide the forms: it reads `[127.0.0.1]:19901`, `[[127.0.0.1]]:19901`, `[127.0.0.1:19901]` and `127.0.0.1:+19901` as 127.0.0.1:19901, and refuses `0x7f.0.0.1` (a domain to `AddEntry`) and bad octal. A participant that refused a form dash-qt wrote couldn't join; the coordinator writes only the plain form, which every reader takes. |
| D79 | **Dissolve now (slice 2a) is Core's `BuildDissolution` output for the wallet's share as actor, with `DissolveShared`'s penalty at the list's height + 1** (`providertx_service.cpp:639-681,1242-1279`; `dw_protx::shared::dissolve`, its bytes equal to dashd's `shared_dissolve` on the committed vectors). The list's height stands for the tip, as m-16's check reads it. Core's builder refusals (`fee must be positive`, `fee exceeds the consensus ceiling of 1000000 duffs`, `penalty and fee exceed the actor's share`) and the relay minimum (`the fee is below the relay minimum of N duffs`, from the exact size with its one signature) are `invalid_argument`; the two fee bounds are checked with the arguments, before the wallet. Core's rules run on the transaction before signing and again on the signed copy, signature included. | Core's RPC reports its builder's refusals as `INVALID_PARAMETER`, so no new §4 code is needed. The size is exact because the only input's scriptSig is empty. The penalty Core picks depends only on the next height, so a dissolution previewed late in the early period may pay none when it is sent; the preview's `early` says which. |
| D80 | **A `dissolve` row has no fee inputs and holds no lock; its `request` column keeps the caller's request with the penalty and the actor output** (`DissolveRecord`). A resend is charged the penalty and the fee again under a `Dissolution` lease, and refuses while sending is off, as an `update_share` row's (D73). D23 runs both ways: `update_share` and `dissolve` each wait while an `update_share` or a `dissolve` of the masternode is `signed`, `not_sent` or `sent`, or `confirmed` in a block D70's `buried` rule doesn't yet count (ChainLocked, or 6 confirmations). A resend of either kind is held under the update lock as an `update_registrar` row's is (MG-04 D43, D49): `update_superseded{operation}` while an `update_share` or `dissolve` of the masternode saved after it is `signed`, `not_sent`, `sent` or `confirmed`, and `update_pending{operation}` while one saved before it is pending as D23 reads it (review r1 n-2). | §6.3's table: the collateral isn't a coin of the wallet, so there is nothing to lock in app.sqlite; the in-memory reservation of `PreparedTx::assembled` still holds it for the hand-off. The signed bytes don't tell the penalty without the share table, which the row doesn't hold. One confirmation isn't enough to settle a row for D23: a reorg can put it back out, and with a later dissolution already out the two race, so a reward change mined second is invalid forever and its row stays `sent` with its fee-input locks. If a reorg deeper than that still puts two back out, neither resends: the earlier is superseded (resent after a newer reward change it would put the old reward back; after a dissolution it can only be invalid), the later waits until the earlier is mined and buried again, which a reorged-out transaction back in the mempools normally is. Rows are ordered by when they were saved (`created_at`, then insertion), so a wall clock stepped back between two saves can swap them. A dissolution confirmed by the list (D81) is at most as deep as the list, so without ChainLocks it never reads buried: later maintenance of its masternode reads `update_pending` rather than `shared_too_early`, and the masternode is gone either way. |
| D81 | **D27's list rule applies to one case only: a `sent` `dissolve` row whose transaction omits its actor output reads `confirmed`, at the list's height, once its masternode is out of the synced list, the wallet's filter-scan checkpoint (`synced_height`) has reached a height the list can't be above, and the registration is in a block at or below that height.** Every other state is the history's. | Such a transaction pays this wallet nothing and spends no script of it, so BIP158 filters never match it. Every other dissolution of the masternode pays this wallet's refund script, so it is in the history: this wallet's own as `confirmed`, another's as `conflicted` (it spends the collateral). Until the scan reaches the list's height, the dissolution that removed the masternode may be another's not yet in the history, so the row stays `sent` (review r0). The checkpoint, not the best processed block: filter batches commit with gaps, and only the checkpoint says every block below it is applied (review r1 m-1). The pinned SPV library hands out the entries without their height, and its engine applies a diff before its progress reports it (rust-dashcore 626c91d `sync/masternodes/manager.rs:520-527`), so the list's height is bounded by the masternode and header readings taken after the entries: the engine requests no diff above a stored header, and the header reading is published before that request's round trip ends. That is not certain: if the reading lags past it, another's dissolution in the newest block can read `confirmed` here until the scan reaches that block (display only: resend and discard refuse a `confirmed` row, and no lock depends on it). A list height read with its entries needs the pinned platform-wallet to return one. That height is the one `confirmed` reports, an upper bound of the real one. A registration reorged out of its block empties the list without any dissolution, so the rule waits for it to be in a block again (review r1 n-1). The rule reads the list only when such a row exists. |
| D82 | **`DissolvePreview.valid_from_height` is the first height a penalty-free dissolution is valid (`registered + period`), and the preview also carries `warnings` (`ActorOutputDust`, n-10) and `total_debit` (the penalty and the fee: what the grant must cover).** | A dissolution now is valid at the next height either way (the penalty is paid while early), so the useful date is when it stops costing the penalty; it is the standby's `full_valid_from` (§9.4). §7's row already lists the dust warning; `total_debit` sizes the grant as `UpdateSharePreview`'s does. |
| D83 | **`dissolve` refuses an actor output below dust (`invalid_argument`) before it signs; `preview_dissolve` only warns of it (D82).** | Core's builder keeps such an output and the ProDisTx rules take it, but no node relays it (`IsStandardTx`'s `dust`, `policy.cpp:139-141`; rc.3's `testmempoolaccept` on `last_actor_dust` in `shared_dissolve_shapes.json`). Without the refusal the hand-off ends `Out` unless a reject comes back, so the row reads `sent`: it can't be discarded (MG-04 D6), is never mined or conflicted, and D23 then refuses every later `update_share` and `dissolve` of the masternode, the penalty-free one included, until it leaves the list some other way. The guard is what keeps the share's exit open (review r1 m-2). The share can raise the fee by the output to omit it (tested), or wait out the penalty. |

| D84 | **A standby is keyed by (wallet, masternode, actor share, fee); writes only add a row or the variant a row lacks.** `create_standby` signs again to the same bytes (RFC 6979), so repeating it stores nothing; an import of a variant held already is `stored: false`, and an imported file's second line completes the row its first began. A stored variant, its source and its date are never replaced. The stored `full_valid_from` is a record: `standby`, the import's description and the file show the registration's current block plus the period, which a reorg may move (review r1 n-1). `export_standby` and `send_standby` take the `fee` with the share to name the standby (§7's rows had only the share). | Two standbys of one share at two fees are both valid exits; keeping both loses nothing, and a key without the fee would make a second `create_standby` either replace signed bytes the user may have exported or refuse. dash-qt keeps no standby at all (the file is the store, `sharedmndialogs.cpp:1670-1693`). Manager ruling (a) in the 2b GO (10-30). |
| D85 | **Sending the full variant before it is valid is refused `shared_too_early`; the network's verdict on it would be `bad-prodis-penalty-floor`** (`specialtxman.cpp:1734`): mined before `registered + period`, a unilateral dissolution must pay the penalty. Valid means mined at the tip + 1 past the early period, or a penalty of 0 (`standby::full_valid`); the tip is the list's height, or the headers' when the list lags them (D81's bound, review r1 of 2b), as dash-qt judges at the chain's tip (`sharedmndialogs.cpp:1295-1315`). | No new §4 code; the refusal says when, as m-16's does. Manager ruling (c) in the 2b GO. |
| D86 | **`create_standby`, `export_standby` and `send_standby` are behind the shared sending switch (§7 †); `standby` and the import are not.** Every standby call reads the registration from the wallet's history and the block holding it (D29), not from a session, and a share is this wallet's when its owner key is a keychain owner key 0–19 (D10). `create_standby` signs for each such share, under one `Dissolution` lease charged each share's penalty and fee (§6.3; the immediate variant gives up both), with the relay minimum checked on the full variant, the larger. Each signed variant passes D32's check before it is stored. | Each produces bytes the network takes: the signatures, the file, the hand-off (manager ruling (b) in the 2b GO). Reading the history makes D19's claims hold: a restored wallet signs a new standby, and an exported one imports and sends, without the session. The engine test drives a wallet of another seed whose history the fixture gives the registration; what it exercises is that history read, which a restored wallet of the same seed takes (a wallet of another seed never holds the registration and is refused `shared_registration_unknown`). A grant sized by a UI comes from the session's `ShareTermsView.early_penalty`; `send.grant_exceeded` names the outflow otherwise. |
| D87 | **`send_standby` runs as `dissolve` does: D23 both ways (a standby send is a `dissolve` row), D81's list rule, D83's dust refusal, and the stored bytes checked again against the current list, the variant read back the one asked for, and their fee at least the relay minimum for their size (review r1 m-1, n-2).** It is charged the variant's penalty and the fee when the actor share is this wallet's, else nothing: a `dissolve` row records `foreign` (absent, so false, on 2a's rows) and its resend charges the same. A `foreign` row's actor output pays another wallet, so it may never enter the history: it settles by D81's list rule as an unseen dissolution does, the registration's height read from the history when the wallet has no session. Residual: a wallet holding no share of the masternode can't tell another dissolution that empties the list from its own, and reads `confirmed`. | §6.3: "penalty + fee when it is the actor, else 0". Core builds and keeps a dust immediate variant (`createStandby`), so the wallet stores it too and refuses only its sending, as D83 does. |
| D88 | **m-20's guard refuses the immediate variant once the full one is valid only when the full one is stored**; an imported immediate variant without its full one stays sendable. At penalty 0 the one copy is the full one, valid at once, so the immediate request is refused `shared_stage{stage: "full_valid"}` and the full one sends. | The guard keeps the wallet from paying a penalty it needn't; with no full copy to send instead, refusing would close the only exit the wallet holds. |
| D89 | **D32's import check, in order: a type-10 transaction whose payload is a ProDisTx with one signature (`shared_message{reason: "not_standby"}`); §5.1's rules mined at the tip + 1 (D85), or at `registered + period` when that is later (`protx_refused{reason}`); the signature by the actor share's owner key (`protx_refused{reason: "bad-prodis-sig"}`); and the whole transaction equal to `BuildDissolution`'s for its actor and its fee (the share amounts less the outputs) with the penalty 0 or `nEarlyPenalty`, signed (`shared_message{reason: "outputs"}`); then its fee at least the relay minimum for its exact size (`invalid_argument`, as `create_standby`'s; review r1 m-1).** The registration in the history and the list come first (`shared_registration_unknown`, `shared_too_early`, m-16), except a type-10 payload that doesn't decode, which is `not_standby` first: the masternode's hash is read from it. The variant is the penalty that matched. | A ProDisTx valid at some height is valid at every later one (§5), so checking the rules at the period's end accepts a full variant signed early and nothing that is never valid. Core's rules take any fee above 0 and dashd's `shared_dissolve` builds at any fee (only dash-qt's field has `MIN_DISSOLVE_FEE`), but one below the relay minimum is never relayed: sent, its row stays `sent` and holds the masternode's updates (D23), so the import refuses it rather than warn, and `send_standby` refuses a row stored before. Comparing the whole transaction also fixes its version, lock time and sequence, which Core's rules don't check. |
| D90 | **The standby file's header is English (D22) and its two dates are UTC ISO: `Created: YYYY-MM-DD` (when the standby was stored; Core creates and saves at once) and `(~YYYY-MM-DD HH:MM UTC)` (from the export's time); the penalty is in DASH (`tDASH` off mainnet).** Core writes the local date, the locale's short date-time (`ApproxBlockDate`, `:243-247`) and the user's display unit (`SharedMnFormatAmount`, `mnsharesession.cpp:200-216`, mirrored for DASH: `format_amount`). The labels, the hash, `Share k of n`, the hex lines, the empty lines and the final `\n` are Core's, byte for byte (`standbyFileText`, `:1702-1741`). A variant the wallet doesn't hold (half of an imported pair) is left out with its label; at penalty 0 the one copy is printed under both labels, as Core prints the identical pair. | Only the hex lines are read back (D22), and the engine has no locale or display unit; ISO dates read the same everywhere. Manager ruling on the dates in the 2b GO (10-30). |
| D91 | **D21's prompt: while sending is on, a confirmed registration session whose masternode is in the synced list reads `next: create_standby` while a share of it marked `mine` has no standby of any fee in the wallet.** `shared_sessions`, `shared_session` and every call returning a session apply it; without a synced list nothing is prompted. | MG-00 D32's "prepares and stores" is a prompt, not a silent step (D21). A delisted masternode (dissolved, say) has nothing left to sign, and `create_standby` would refuse it (m-16), so the prompt goes with the listing. |
| D92 | **2c's request check (`together::check`), in order: a type-10 transaction whose payload decodes as a ProDisTx (`shared_message{reason: "not_dissolution"}`); of the envelope's masternode (`other_masternode`); with empty `vchSigs` (`signed`); then the whole transaction equal to `PrepareSharedDissolution`'s, `BuildDissolution(dmn, actor, 0, fee)` (`providertx_service.cpp:1281-1296`), for its actor and its fee (the share amounts less the outputs) (`outputs`).** The registration in the history and the list come first (`shared_registration_unknown`, `shared_too_early`, m-16), then the relay minimum (D96). Signatures are taken entry by entry as dash-qt's `addSignatures` takes them (§2.5, `sharedmndialogs.cpp:577-613`): one out of range, unreadable or that doesn't verify by its share's owner key over `MakeSignHash(tx, n)`, and a second, different one for a share, are skipped; the import still succeeds and reports `SharedWarning::SkippedSignatures{shares, count}`; a byte-identical repeat is ignored. A message with another `tx` for a target session, or for another masternode, is `other_session`. | The byte check is D89's for the unanimous case: it covers what `SharedMnReadDissolution` and Core's rules leave open (lock time, D16; version; sequence; a non-actor's output to its own refund script at its exact principal) and every edit the oracle shows dashd accepting but the wallet must not sign (`lock_time`), while dashd's own refusals (`fee_over_ceiling`, `non_actor_short`, `actor_paid_elsewhere`) fail it too. Per-entry skipping is dash-qt's behaviour; refusing a whole reply over one bad entry would strand the good ones. |
| D93 | **A dissolve-together session is a `shared_mn_sessions` row of kind `dissolve_together`, no migration: its id m-17's (SHA-256 of `dissolve` ‖ ProTx hash ‖ sign hash), `role` coordinator for the wallet that prepared it and participant for one that imported it first, `envelope` the wallet's own sigs envelope (the request and every signature verified so far), `my_shares` this wallet's shares of the masternode by keychain owner key index, and `signed_tx`, `txid`, `out_at` the combined bytes once sent.** The registration flows' readers (keys in use, held shares, the follower's release, `shared_confirmed`) read registration rows only; a together session holds no coin and no key of its own. | The table already has every column a maintenance session needs (§9.1). Keeping the registration readers to their kind leaves their rules untouched. |
| D94 | **A dissolve-together session's stage, first match wins: `confirmed{height}` when the combined transaction is in a block of the history; then, once sent from here, the operation's: `confirmed{height}`, `conflicted`, else `sent` (`not_sent` and `signed` among them: the operation says when to `resend_operation`); then the collateral spent in a block of the history: by the request signed, `confirmed{height}` (a wallet that never held every signature), by another transaction, `dead{reason: delisted}`; then `dead{reason: delisted}` when the synced list above the registration's height omits the masternode and the wallet's filter scan has reached D81's list bound; then `combined` with every share's signature, else `signing`.** D81's guard: below the list's height, the dissolution that removed the masternode may be this session's own, not yet in the history, so the session keeps its signature stage (review r1 m-1), and `discard_shared_session` refuses it `shared_too_early` until the scan catches up. While the scan lags, `next` may still read `approve` or `send`; both refuse `shared_too_early` (m-16, the masternode isn't listed), and MG-10b's "too early" wording covers this. The guard reads the registration from the history, as the stage does: without it the stage reads `signing` and never `dead`, and discard isn't refused (residual). A wallet the dissolution pays nothing (no refund script of its shares is its own) never sees it in its history, so its unsent session reads `dead{delisted}` once the masternode leaves the list (BIP158 filters match scripts; as D81 and D87). These are §3.3's `collecting` → `complete` → `sent` → `confirmed` / `conflicted` in the existing `SharedStage` (DEC-917): the contract's enum and its decoders don't change. `next`: `approve` while a share of this wallet is unsigned, `copy` while another's is, `send` once combined, `wait` once sent, else `nothing`. | One enum for every session kind keeps the UI's decoders as they are; the names say the same thing. A participant that never imported the last reply doesn't know the final txid, so the request's unsigned bytes identify it. |
| D95 | **`send_maintenance` of a dissolution combines the signatures in share order (`InsertUnanimousSignatures`, `providertx_service.cpp:705-729`), runs Core's rules with them, D83 and the relay minimum, writes `signed_tx`, `txid` and `out_at`, and hands it off as a `ProTxOperation` of kind `dissolve` (2a's path, D23, the op states, D81's settlement): penalty 0, `actor_output` from the transaction, `foreign` when the actor share isn't this wallet's, and `together` set, so the row is charged 0 on the send and on every resend.** Any wallet holding every signature may send (`CombineShared` needs none); `start_dissolve_together` and `approve_maintenance` charge the fee when they sign the actor share, else 0 (§6.3). `discard_shared_session` deletes a together session not sent; `abandon_shared_session` refuses it (`invalid_argument`): none of its coins is this wallet's to spend back. `approve_maintenance` and `send_maintenance` refuse a session at another stage before anything else of it (`shared_stage`, `dead` for a delisted masternode), then D23. A session marked out whose operation was never saved (a storage failure in the hand-off), or whose operation was discarded before anything was sent (`signed` or `not_sent`), reads its signature stage and sends the same bytes again, or is discarded: the hand-off saves the operation before the bytes leave, so no operation holding its txid means nothing left this wallet (review r1 n-3). | The fee leaves the actor's share; consent to it is the actor's approval, so charging it again at the send would double-count it, and a non-actor's send moves none of its coins. One operation kind keeps D23 and the follower's settlement as they are. |
| D96 | **The relay minimum for the combined size (one 65-byte signature per share) is checked at `start_dissolve_together`, at import, at `approve_maintenance` and at `send_maintenance` (`invalid_argument`).** m-15: `approve_maintenance` of a request this wallet didn't prepare whose actor share is this wallet's refuses `masternodes.shared_actor_fee{fee}` unless `accept_actor_fee`, or unless the fee is at most that minimum. | D89's reason: below it no node relays it, and sent it would stay `sent` and hold the masternode's updates (D23). The fee is another party's choice taken from this wallet's coins, so it needs its own consent above the least it could be. |
| D97 | **2c imports a `dissolve` sigs envelope (creating the session as participant, or adding to the one with its id) and a bare signature array for a target dissolve-together session; a `registrar` envelope still refuses `unsupported{feature: "shared_masternodes"}` (D45) until the key change, slice 2d.** A bare array's `received_code` is its own JSON's fingerprint, the code its sender reads out. Import signs nothing and is not behind the sending switch; `start_dissolve_together`, `approve_maintenance` and `send_maintenance` are. The actor of `start_dissolve_together` must be a share of this wallet (`share_not_mine`), as dash-qt's together tab offers only its own (`sharedmndialogs.cpp:1373-1375`); starting a request this wallet imported first signs it and keeps the session's role. A `signatures` value that isn't an array adds `SharedWarning::NoApprovals` ("This message does not contain any approvals.", `sharedmndialogs.cpp:569-572`) and the rest is taken. | Scope as approved (DEC-917). |
| D98 | **The together session's message is the sigs envelope with every signature held: named `Request` for the wallet that prepared it and `Reply` for an approver (dash-qt's "Copy Request" and "Approve and Copy Reply"), file name `shared-mn-session-<CODE>.json` as every session's.** | One name per role is enough for the copy-paste loop; the envelope is the same shape either way. |

### Follow-ups

- MG-03b's scope: the owner key ids 0–19 as filter elements (D29). BL-187 merged without them.
- MG-10b copy: the coordinator pays the registration's network fee; participants pay none (D9, replacing MG-00 D41).
- MG-03b: reading other parties' reward changes (ProUpShareTx by ProTx hash) and per-share payouts; until then the
  wallet's own share's reward is known from its own operations only.
- **MG-03b, the v3 payouts gap.** Since BL-187 the pin decodes a version-3 ProRegTx's `payouts` and
  `shared_collateral` (and a v3 ProUpRegTx's `payouts`), but MG-03's masternode rows still read version 2's fields only
  (BL-187 D12, D27): a shared or v3 masternode's row shows no payout list, operator reward or share table. MG-04b's
  views take the shares from the wallet's own session and registration (D29), so 1a and 1b don't depend on it; MG-03b
  reads `payouts` and the share table into the rows.
- **Done in 1b (D69, D70):** **before 1b sends (DEC-338): M3 as shared engine code** that MG-04's fee locks also adopt (BL-346). (a) An unlock
  (coin control's `unlock_outpoints`, `unlock_reserved` on discard and the follower's release, MG-04's fee locks)
  never releases an outpoint another live row reserves (a shared session or an MG-04 operation) or a row that is out
  reserves; (b) `reserved_inputs` and `shared_candidates`' `also` refuse or exclude an outpoint another row reserves,
  so a reservation stays exclusive inside the engine whoever unlocked it; (c) a row-level release for an out row
  that no longer parses, keyed by its stored `reserved` and `txid` (review r2 nit 5).
- **Done in 1b (D68):** **before 1b sends (DEC-352): BL-352, MG-04's key-pick window**, fixed the D63 way with a concurrency test, alongside
  BL-346. MG-04's calls that pick keys do so before any coin fence, the window D63 closes for shared sessions:
  `register` and `prepare_external_registration` pick the owner, voting and operator keys (`build.rs`'s `pick_owner`,
  lowest unused), `preview_update_registrar`/`update_registrar` the voting and operator keys (`resolve_update`). So
  one of them and a concurrent `start_shared`/`join_shared` (or two `register`s) can take the same owner, voting or
  operator key (Opus-h r1 and r2 NIT-2, validator r4).
- Done in 1b (D71): `sign_shared_funding` (and `approve_shared`'s D50 re-check) hold the coin fence from `reserved_inputs` through
  the `out_at` write, as a send holds it from its coin read through signing (`coins.rs`'s fence rule).
- Foreign-input valuation through DAPI's Core `getTransaction` (D26), turning the unvalued-inputs warning into Core's
  hard `fundingCheck` where DAPI answers.
- A one-call abandon that also voids every pending key change of a masternode the wallet funds (D25, D26).
- Shared evonodes once `evo_shares` can activate (D4).
- Tor/I2P/CJDNS/domain services in the net info (D5).
- Issue the refund and reward addresses after Core's rules and coin selection pass in `start_shared`,
  `join_shared` and `update_share` (validator r1 of 1b, n4), so a refused call doesn't take receive addresses (MG-04's register has the same order; issuing is
  bounded by the pool and refuses `gap_limit` rather than pass it).
- Review r1 of slice 1a (Opus, on 9aa6571e), not fixed in 1a, before 1b sends:
  - (M3) ruled for before 1b sends (DEC-338), above;
  - (M5) fixed in 1a (D63);
  - (M9) an oracle envelope generated by rc.3's `MnShareSession::toJson`, so `toJsonBody`'s order and `fromJson`'s
    checks are proven against dash-qt rather than by inspection, and `check_registration` compared with dashd's verdicts
    for the mutated join signatures: closed in slice 1c (D78);
  - (nits) `SocketAddr` refuses some forms Core's `Lookup` takes (`[1.2.3.4]:port`, inet_aton, `::ffff:`): closed in
    slice 1c (D78); a blank
    `tx` in a sigs envelope (part 2); a negative int16 `nVersion`'s encoding; duplicate share indices in
    `SigsEnvelope::to_json`.
- `approve_shared` can refuse `unsupported{service}` after the session checks, since it needs the session's terms
  (review r2 nit 4); mn-engine §2.4 lists it with the session's refusals.
- Carry every net-info entry in platform-wallet's `MasternodeListSummary` (BL-187's pin decodes them, D62) and in
  `ListedMasternode`, so §5's `bad-protx-dup-netinfo-entry` sees them all (D39): closed in slice 1c (D75).
- T1 `regtest/functional/dwd_shared.py` for slice 1b (creation, both mixed sessions, the reward change): not in 1b.
  The run needs dashd rc.3 with v24 by `-vbparams`, and first a version-3 masternode dashd registers, since the
  wallet reads v24 `active` only from a version-3 list entry (D3); the engine tests cover the flows in process.
- Part 2: the vault's ProUpShareTx and ProUpSharedRegTx signers take the registration and check the key's share, as
  `sign_dissolution` does; they have no 1a caller (review r1 of 1a, Opus-h NIT-3).
- Done in 1b (D70): a depth before the follower reads an out session `conflicted` and releases its locks, since a reorg can undo the
  spend that made it so (Core behaves the same; no funds are lost, at worst a later conflict) (Opus-h NIT-4).

## Appendix: review r1 responses (DEC-311)

Review r1 (Opus, on cd719ae3) was CHANGES REQUIRED; DEC-311 ruled on the Majors. Each finding, and where it is
answered:

| # | Response | Where |
|---|---|---|
| M-1 | Fixed as ruled: a participant up to `signing`, `out_at` unset, re-adopts a higher revision only with its own row and contribution unchanged; a new draft → `joined` (same reservation and Details), a re-lock → `to_approve` with the terms diff and a fresh approval; `combined`+ or out → `conflict`. Three T0 cases and mutant 24 | §3.0, §3.2, D15, §12 |
| M-2 | Fixed as ruled: `sign_shared_funding` is on the 1a facade and refuses `unsupported{shared_masternodes}` while `can_send` is false, like every † call; the three-wallet test uses a test-support override; T0 test and mutant 20 | §7, §3.2 step 5, D1, §12, §13, §14 |
| M-3 | Fixed, the reviewer's option (b) as ruled: own side checked exactly; fee floor from an estimated size (foreign inputs as P2PKH); a non-blocking `UnvaluedInputs` warning naming the contributions; no "positive" claim; `abandon_shared_session` releases locks only once a block settles it and never deletes an out row; DAPI valuation is a follow-up | §3.1 step 3, §4, §9.3, D9, D26, Follow-ups |
| M-4 | Fixed: §5.1 lists types 10–12's trivial and contextual rules in Core's order with Core's reasons, run before signing; `Refund` in a ProUpShareTx is the refund script; mutant 21; the rules tests cover all four payload types | §2.4, §4, §5.1, D31, §12 |
| M-5 | Fixed as ruled: the key-change base is stored; approve and send refuse `stale` once the list differs; the funder is offered abandon on a fee input; every approver's preview shows the ban and the funder's lasting ability to send; mutant 22 | §3.3, §4, §8, §9.1, D25 |
| m-1 | Fixed: the debit must be exactly the recorded `I − C`, and that figure within share (+ fee) + (no change ? dust − 1 : 0) | §4 "Own debit" |
| m-2 | Fixed: a D-row records the coordinator-signs-last deviation, its benefit, and the dash-qt-coordinator case for D17 | §1, §3.1, §3.2 step 6, D24 |
| m-3 | Fixed: the import decision table by role × our stage × their stage; a coordinator in `draft` refuses a stale locked copy as `older`; a first import past `draft` is `not_participant` (the multi-device case continues on the joining device); every cell is a T0 case | §3.0, §12 |
| m-4 | Fixed: a non-64-hex `sessionId` is refused; `(wallet_id, id)` is the key | §2.3, §9.1, D28 |
| m-5 | Fixed: labels non-empty and unique after trimming, every contribution's label names a share, one contribution per share, on every import and in `start_shared` | §2.3, §3.1, D28 |
| m-6 | Fixed: "this wallet's inputs" throughout; non-funders' key-change states from the list; dissolutions by the collateral leaving | §3.1 step 6, §3.3, §9.2, D27 |
| m-7 | Fixed: an incoming scriptSig on an own input is never taken; before `out_at` a later foreign scriptSig replaces the earlier; the stall-and-abandon outcome is stated | §3.0 "Per-input merge" |
| m-8 | Fixed: draft-only `set_shared_fee`, and lock computes the needed fee (`fee_low{needed}`) | §3.1 step 3, §7, D33 |
| m-9 | Fixed: the full ProRegTx list in Core's order, trivial then contextual, with what SPV can't check | §5 |
| m-10 | Fixed: D5 lists the v3 `ExtNetInfo` rules (≤ 4, distinct, port, routable, no 9999, fc00::/8 flip, order) and the empty-service warning | D5, §8 |
| m-11 | Fixed: the share table comes from the registration in the history; maintenance refused when it's missing; owner-key-id filter elements added to BL-187/MG-03b's scope | §3.3, D29, Follow-ups |
| m-12 | Fixed: the key-change preview shows the reset and ban, the basic-scheme key and the one-change mempool rule to every approver | §3.3, §4, D25 |
| m-13 | Fixed: MG-04's registrar calls refuse `bad-protx-shared-mn`; test and mutant 23 | §7, §14, D30, §12 |
| m-14 | Fixed: the v3-evidence rule on every network; mainnet's `NEVER_ACTIVE` kept only as a pin-tied test | §1, D3 |
| m-15 | Fixed: an actor share this wallet holds in a request it didn't prepare needs `accept_actor_fee` unless the fee is at most the relay minimum | §4, §7, §8 |
| m-16 | Fixed: `tip > registered_height` and listed for every maintenance call, citing specialtxman's `pindexPrev` checks | §3.3, §10 |
| m-17 | Fixed: maintenance id = SHA-256(kind ‖ ProTx hash ‖ sign hash), 64 hex | §3.3, D28 |
| m-18 | Fixed: `import_shared_message` takes an optional target; a bare array without one is refused | §2.5, §7 |
| m-19 | Fixed: an imported standby is fully checked and stored for `send_standby` | §9.4, D32 |
| m-20 | Fixed: one copy at penalty 0; `send_standby` refuses the immediate variant once the full one is valid | §9.1, §9.4, D21 |
| m-21 | Fixed: the sigs-envelope reader rules in full (empty `vchSigs`, `version` required 1, exact kind, missing network, 65-byte entries, duplicates), plus `vin == [collateral]` | §2.5 |
| m-22 | Fixed: `export_standby` takes a `MasternodeOp{0}` "Dissolution" grant; `standby` returns descriptions only (no bytes, so no grant); stored standbys stated as plaintext | §6.3, §7, §9.1, D21 |
| n-1 | Fixed: the `fromJson` details listed | §2.3 |
| n-2 | Fixed: `received_code` is the received object's recomputed fingerprint; the merged session's `message_code` is exposed separately | §2.3, §8 |
| n-3 | Fixed in 68068e6c (D7 states CI reads only the vectors; D9 cites DEC-310) | D7, D9 |
| n-4 | Fixed: D16 says `0xffffffff` is deliberately stricter than `HasTimeLock` and covers key changes | D16 |
| n-5 | Fixed: §14 lists each version's calls; `UpdateShare` with 1b, `Dissolve` with part 2 | §14, §8 |
| n-6 | Fixed: a spent reservation is `shared_stage{stage: "dead"}`, not a message reason | §9.2, §10 |
| n-7 | Fixed: lock names the coordinator's own consent checks, `foreign_input` included | §3.1 step 3 |
| n-8 | Fixed: the type 10–12 layouts written out | §2.4 |
| n-9 | Fixed: monotone validity is `specialtxman.cpp:1704-1708` (the comment is at 1705-1706; the rule at 1707-1708), ceilings 1752-1766; the v24 gate is `1112-1116,1158-1161` | §1, §5, D3 |
| n-10 | Fixed: the preview warns on an actor output in (0, dust) | §4, §8 |
| n-11 | Fixed: `sign_dissolution` with count 1 requires `actorIndex` = the signing share; the unilateral fee floor is the relay minimum | §4, §5, §6.2 |
| n-12 | Fixed: the sigs-envelope key order, ascending signatures, lower-case ProTx hash; the standby file's 1-based `share<k>`, "Share k of n" and trailing `\n`; imports compared with `BuildDissolution` | §2.5, §9.4 |
| D18 | Fixed: stated as MG-05 §2's net debit outside `TxDraft::prepare`, charged in the J step that hands out the funding signer | §6.3, D18 |
| Mutants | Fixed: the review's 15 added to the doc's 9 (its six plus `overdraw`, H-1 and D16), 24 in all, each to be killed by a named test | §12 |
| T1 | Fixed: a second mixed session with dashd as coordinator, so the wallet participant's signature completes the transaction | §12 |

One correction to the review: a different second signature for a share in a sigs envelope is skipped with a warning
and the first kept, not refused (`sharedmndialogs.cpp:605-609`); §2.5 mirrors that.

## Appendix: review r2 responses (DEC-312)

Review r2 (Opus, on aca5a407) was ACCEPT WITH NITS; DEC-312 accepted the design and ruled on the Minors.

| # | Response | Where |
|---|---|---|
| r2-m1 | Fixed before implementing: §3.0 is first-match, top-down; the coordinator's draft + draft cell doesn't compare revisions (`absorbDraftReply`, `mnsharesession.cpp:1204-1284`), a reply adding nothing doesn't bump; T0 case with seven Details in any order | §3.0, §12 |
| r2-m2 | Fixed before implementing, option (a): `set_shared_fee` only while no other party's contribution is in the session; a reply to an older Invitation is refused `older` and that party re-joins; D33's interop claim corrected (`:1258-1261`, `:1339-1346`) | §3.1, §7, D33, mutant 32 |
| r2-m3 | Accepted: the approval preview shows every key as current → requested against the list at approval time; the approval records it and refuses if the list moved since | §3.3, §4, D25 |
| r2-m4 | Accepted: the override is a `test_support` constructor argument under MG-03 D40's gate, unreachable from dwcli; a release-profile build and test prove it absent (a killer of mutant 20) | §7, D1 |
| r2-m5 | Accepted: `oracle_univalue.rs` checks the case count, the per-case fields and the `read_only` flags; a corrupted-vector test is committed | §12 |
| r2-m6 | Accepted: mutants 25–33 | §12 |
| n-r2-1 | `invited` + `frozen`/`signing` → `not_participant` | §3.0 |
| n-r2-2 | Same-revision `signing` merges require an equal consent hash, else `conflict` | §3.0 |
| n-r2-3 | A reply that adds nothing is a no-op without a bump | §3.0 |
| n-r2-4 | A `sessionId` that isn't lower case is refused, not lower-cased | §2.3, D28 |
| n-r2-5 | The maintenance id's bytes are specified | §3.3 |
| n-r2-6 | "standby export"; part 2's completing signatures are † | §7 |
| n-r2-7 | `GetValidatedPayload`'s prefix is the decoder's; tests for `bad-proupsharedreg-not-shared` and sig counts above 8 | §5, `shared/rules.rs` |
| n-r2-8 | The shared ProRegTx rules and D5's net-info rules land in 1a with Core-order tests | `shared/rules.rs`, `shared/netinfo.rs` |
| n-r2-9 | Escaped, control-character and non-ASCII keys added to the UniValue inputs | `testdata/oracle/univalue/` |
| n-r2-10 | The P2PKH fee floor is a floor only for P2PKH foreign inputs | D26 |

## Appendix: slice 1a review r1 responses (DEC-338)

Review r1 of 1a at 780c5f93: Sol (dw-protx, Core parity) ACCEPT WITH NITS, Opus-h (engine, vault, store, funds)
CHANGES REQUIRED for MAJ-1. DEC-338 ruled items 1–6 into 1a and M3 before 1b sends.

| # | Response | Where |
|---|---|---|
| Opus-h MAJ-1 (M5) | Fixed: one coin fence from the key picks to the row's write | D63 |
| Opus-h MIN-1 (M3) | Ruled for before 1b sends, as shared engine code (BL-346) | Follow-ups |
| Opus-h MIN-2 | Fixed: an unreadable row's owner keys from `my_shares` | D64 |
| Opus-h NIT-1 | Fixed: the funding step compares the approved consent hash | D65 |
| Opus-h NIT-2 | Fixed: an engine test reaches `approve_shared` with a moved own row (mutant E) | `shared/tests.rs` |
| Opus-h NIT-3 | Not in 1a: the share-update signers have no 1a caller; part 2 binds their key to a share as `sign_dissolution` does | Follow-ups |
| Opus-h NIT-4 | For 1b: a depth before releasing a `conflicted` session's locks | Follow-ups |
| Sol nit 1 | Fixed: oversized net info decodes to Core's reason | D67 |
| Sol nit 2 | Fixed: `CheckMalleable` in the rules' operator-key check | D66 |
| Sol slice boundary | Already a follow-up before 1b sends: every net-info entry in the list model (D39) | Follow-ups |
