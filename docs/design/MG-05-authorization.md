# MG-05: masternode authorization

ROADMAP MG-05 (release 1.1, size M, engine, tier T0, stack-independent): the scopes `MasternodeVoting` and
`MasternodeOwner`, and the restored grant purposes `Governance` and `MasternodeOp{max_duffs}`. The cap covers the net
debit and the fees.

Acceptance: a voting lease cannot sign a transaction; a `FundNew` collateral does not count against the cap, and
fees do.

Inputs:
- E0-04's grants, leases and dispatch fence ([`E0-04-grants-leases.md`](E0-04-grants-leases.md) §3, §4, §5, §9.1);
- MG-00's rows: D35 (lease names) and the MG-05 hand-over row ([`MG-00-masternodes-ux.md`](MG-00-masternodes-ux.md));
- MG-01 §7 and its D7 and D11 (attached keys, [`MG-01-salvage.md`](MG-01-salvage.md));
- MG-02 D6, D24 and D80 (the `vote` stub, `relay_vote`, resolved outpoints).

D-numbers in this file are MG-05's (D1 and up).

| Part | Where |
|---|---|
| Grant purposes, scopes, the vote signer | `rust/crates/dw-vault/src/{types,signer,vote}.rs`, `vault.rs` |
| Flows, budgets, signers of a lease | `rust/crates/dw-engine/src/platform/{flows,lease/session,lease/table}.rs` |
| The vote artifact in the fence | `rust/crates/dw-engine/src/platform/lease/fence.rs` |
| `TxDraft::prepare` under a `MasternodeOp` lease | `rust/crates/dw-engine/src/send/mod.rs` |
| `Governance::vote` | `rust/crates/dw-engine/src/governance/voting.rs` |
| The Tauri binding | `rust/crates/dw-app/src/api/{vault,governance}.rs` |

## 1. Grants and scopes

| Purpose | Bound to | Credential (m1-engine §2.2) | Quick-unlock value | Yields | Charged |
|---|---|---|---|---|---|
| `Governance` | the acting wallet | the `Spend` column | 0, as `SignMessage` | one `VoteSigner` (scope `MasternodeVoting`) | never: a vote moves no funds |
| `MasternodeOp{max_duffs}` | the acting wallet | the `Spend` column | `max_duffs` | one `VaultSigner`, scope `MasternodeOwner` | the net debit of each Core transaction, fees included (§2) |

| Scope | Uses | Keys |
|---|---|---|
| `MasternodeVoting` | the voting key id; a signature over a funding vote whose digest the vault computes (§3). **Never a `VaultSigner`**, so there is no `Signer`, no sighash and no message. | keychain voting keys `m/9'/coin'/3'/1'/i` (non-hardened `i`); the `voting` key attached to any tracked masternode (MG-01 D7) |
| `MasternodeOwner` | sign, public key; `signmessage` | BIP44 and BIP32 addresses (inputs, change, and the ExistingUtxo collateral proof); keychain owner keys `m/9'/coin'/3'/2'/i` (ProUpRegTx's payload signature) |

Both scopes refuse every other path and use before the seed is read, as the Platform scopes do (E0-04 §3.4). Neither
reaches an operator or platform-node key. The wallet holds no such secret, and `no_operator_signing` stays green.

## 2. The cap: net debit plus fees

`MasternodeOp{max_duffs}` caps what each Core transaction of the flow takes out of the wallet. That is
`TxDraft`'s outflow: the outputs to scripts the wallet does not own, plus the fee (`send/mod.rs`, m1's
`total_debit`).
- A `FundNew` collateral pays an address of the wallet, so it is not part of the debit. The fee is.
- `ExistingUtxo` and `External` collaterals add no output.
- The engine charges the lease's `Masternode` budget at `TxDraft::prepare`, before signing, in the same J step that
  hands out the signer. This is the `Spend` path of DP3-01, with the other budget.
- A charge equal to what is left fits; one over it is `platform.grant_exceeded` and signs nothing.
- The vault cannot see the transaction it signs (E0-04 F12). So the check is the engine's, as it is for
  `PlatformFunding`. The token also bounds the signer: `masternode_signer` refuses any purpose other than
  `MasternodeOp`.

## 3. Why a voting lease cannot sign a transaction

A key that signs a 32-byte digest signs whatever that digest is. Coins sent to a voting key's address would make a
sighash signable by any digest signer at that path. So `MasternodeVoting` is not a digest scope.
- `Vault::vote_signer(wallet, token)` returns a `VoteSigner`, which has two methods:
  - `key_id(key)`, the voting key's hash160;
  - `sign_vote(key, vote)`, where `vote` is the funding vote's fields (collateral outpoint, proposal hash, outcome,
    time).
- The vault builds the digest itself, as Core does (`CGovernanceVote::GetSignatureHash` on testnet; the
  `signmessage` hash of `GetSignatureString()` elsewhere). It then returns the 65-byte compact signature.
- A transaction sighash is the double SHA-256 of a different preimage. On testnet a caller of `sign_vote` picks
  76 of the 84 preimage bytes, so the engine is its only caller and fills them from the list (collateral), the
  store (proposal) and the clock (time); elsewhere the `signmessage` prefix separates the two (D1).
- The signal is fixed to funding: a proposal's other signals are the operator's (MG-02 D89).
- The vault keeps its own ~20-line copy of the digest, because `dw-governance` pulls in `dw-p2p` and tokio. An
  engine test holds it equal to `GovernanceVote::signing_digest` on every network, and checks that the store takes
  the signed vote as its masternode's.

## 4. Leases and the dispatch fence

- **Flows (MG-00 D35 names):**

  | Flow | Grant |
  |---|---|
  | `voting` ("Voting") | `Governance` |
  | `masternode_registration` ("Masternode registration") | `MasternodeOp` |
  | `key_change` ("Key change", ProUpRegTx) | `MasternodeOp` |

  MG-06, MG-13 and MG-04b add their own flows and grants: "Proposal", "Credit withdrawal", "Shared masternode" and
  "Dissolution".
- **Budgets:**
  - `masternode` (Core duffs, charged as in §2);
  - `voting` (never charged; it only marks that the lease carries the vote signer).
- **One grant per vote batch** (MG-00): `vote` begins a `voting` lease from its one grant and signs every
  masternode's vote under it. It ends the lease when it returns.
- **Lock.** A lock revokes the lease, as any other. The signers stop at once:
  - a held key is erased when its `KeyHold` drops (E0-04 §3.5);
  - a vault-key signer ends with the epoch.
- **The fence.** A vote is a third row-less artifact kind, `ArtifactKind::Vote`, keyed by the vote hash. The vote
  is handed off when it enters the governance store, which relays it and announces it again on the next connection.
  That hand-off happens under a `First` permit, admitted in a J step that finds the lease live (I2). A lock drains
  the permit (I5), so no vote enters the store after the lock's drain (the async lock returns after it; the
  synchronous one may return before).
  - A vote the store refuses ends `NotSent`; one it takes ends `Sent`.
  - A vote handed off under an unleased dispatch scope is refused, as a row-less asset lock is: its authority is
    the voting lease (D5).
  - There is no charge and no journal row: a vote moves nothing, and a lost one is cast again with no harm. Core
    dedups by hash and keeps a masternode's newest vote.
- **`MasternodeOp` Core sends** are row-less Core transactions, like DP3-01's contact payment. `prepare` charges and
  signs, and `broadcast` goes through the fence under the lease's scope. The refund rules are unchanged (E0-04 §4.2).

## 5. `Governance::vote`

`vote(wallet, proposal, outcome, masternodes, grant) -> Vec<VoteCast>`, one per masternode in the given order.
- **Whole-call errors**, checked before any grant is redeemed:
  - sync off, not synced, list unavailable;
  - the proposal unknown or not a proposal;
  - `invalid_argument` for no masternodes, more than the list holds, a malformed or repeated proTxHash.
- **Lease errors**, returned when the grant is redeemed: `GovernanceError::Platform`, with the `platform.*` code
  (`platform.grant_invalid` for a grant of another purpose, wallet or one already used; D12).
- **Per masternode, `VoteCast{pro_tx_hash, hash, peers, rejected}`:**
  - `rejected` is set when the masternode is not in the tip list (`unknown_masternode`), when the store has not
    resolved its collateral (`collateral_unresolved`, MG-02 D80), or when the wallet holds no key with the list's
    voting key id (`no_voting_key`). A store refusal sets it to `relay_vote`'s reason vocabulary (`too_often`,
    `same_timestamp`, `obsolete`, `duplicate`, …). Three reasons are the batch's own (D13): `cancelled` (the vault
    locked, or the fence refused the vote because a lock revoked the lease), `signer_unavailable` (another vault
    error, including a keychain the vault could not read when no attached key has the id, D23) and `in_flight`
    (the same vote is still in flight or being resolved, the fence's `Deferred`).
  - Otherwise `hash` is set, and `peers` counts the peers it was relayed to. With no peer, the vote is kept and
    announced on the next connection (D24), and `peers` is 0. That is no error, unlike `relay_vote`'s
    `governance.no_peers`, so one offline moment does not hide the other results.
- **Keys.** It tries the keychain voting key first, at indexes 0–19 (`VOTING_KEY_INDEXES`, as `voting_masternodes`
  uses), then the masternode's attached `voting` key. Each must hash to the list's voting key id. A keychain the
  vault cannot read, other than a lock, leaves the attached key to try (D20).
- **Time.** Each vote is stamped with `now` (tests pin the clock, so a repeated batch is the same vote). Core's
  per-masternode limits on vote time stay the store's to enforce.

## 6. Left to other rows

- **MG-04:** the ProTx builders call `TxDraft` (or their own builder) under a `masternode_registration` or
  `key_change` lease. Signing with an attached owner key (a tracked masternode whose owner key was imported) needs a
  key reference like the vote signer's; MG-04 adds it with its first caller.
- **MG-06:** the proposal fee transaction is a plain spend, so it takes `Spend` under its own "Proposal" flow.
- **MG-07:** contested-name votes are Platform state transitions signed with a voting key. That is a second
  `VoteSigner` method with its own digest, not a digest scope.
- **MG-13:** credit withdrawal is priced in credits. It gets its own purpose there, not `MasternodeOp`'s duffs
  (MG-00).

## 7. Tests

- **dw-vault:**
  - each scope's paths and uses, allowed and refused;
  - the purposes' wallet binding, credential column and quick-unlock values;
  - `vote_signer` refuses any purpose other than `Governance`, and `masternode_signer` any other than
    `MasternodeOp`;
  - `platform_signer` refuses both new scopes;
  - a vote signed with a keychain key and with an attached key recovers the right key id;
  - a lock ends the vote signer.
- **dw-engine:**
  - the digest equals `GovernanceVote::signing_digest` on all four networks;
  - **a voting lease yields no transaction signer**: `TxDraft::prepare` under it fails `send.grant_invalid` (D14),
    and the lease carries no `Signer` at all;
  - **a `FundNew`-shaped draft charges only its fee**: an output of 1,000 DASH to the wallet itself, under a cap of
    exactly the fee, signs, and one duff less refuses;
  - a draft paying a foreign script charges the amount plus the fee;
  - a draft paying a DashPay contact's address, or sending its change there, charges what leaves the wallet (D19);
  - a prepare that fails after its charge refunds the `Masternode` budget (review r1 F3);
  - in a PSBT, an output to a contact's address gets no derivation record and an input from one is not signable
    (D19, review r1 F2);
  - `vote` signs and stores one vote per masternode, and returns per-masternode refusals, an attached key that is
    not the list's included;
  - the key choice: keychain, then attached, an unreadable keychain falling through (D20) or reporting
    `signer_unavailable` (D23);
  - a vote another wallet is handing off is `in_flight` (D13);
  - a lock between two votes ends the batch with the rest refused;
  - the vote fence kind is admitted only under a live voting lease, and refused unleased (D5).
- **dw-app:** the `governance_vote` command and the grant purposes in the contract test.

Where they are: `dw-vault/tests/masternode_signers.rs` and the scope tests in `signer.rs`;
`dw-engine/src/platform/lease/session_tests.rs` (`mg05_*`), `platform/lease/tests.rs`
(`the_fence_refuses_an_unleased_vote`), `send/flow_tests.rs` (`psbt_a_contacts_address_is_not_the_wallets`),
`governance/voting.rs` (`the_vaults_vote_digest_is_the_governance_digest`, the key choice) and
`governance/voting_tests.rs`. A lock between two votes is tested at the hand-off: signing runs before the first
hand-off, so a lock mid-batch reaches the rest there, as `cancelled`.

## D-rows

| D | Decision | Why |
|---|---|---|
| D1 | `MasternodeVoting` is no `VaultSigner`. The `VoteSigner` computes a funding vote's digest itself and signs only that. Its one caller is the engine's `vote`, which takes the collateral from the list, the proposal from the store and the time from the clock. | The acceptance, structurally (§3). A digest scope at the voting paths would sign a sighash for coins sent to a voting key's address. On testnet the digest is a double SHA-256 of 84 bytes a caller mostly picks; with the fields the engine supplies, no transaction's sighash preimage is among them (review r1 L10). |
| D2 | `Governance` and `MasternodeOp` follow the `Spend` credential column. `QuickUnlock` issues `Governance` at value 0 and `MasternodeOp` at `max_duffs`. | Both sign; neither reveals a secret. A vote moves no funds, as a `SignMessage` signature doesn't. The UI still ends every signing flow with the passphrase field where the host requires it (UX-SPEC §4.25). |
| D3 | A `Governance` grant is bound to the acting wallet, and its vote signer may use any attached `voting` key. | Attached keys are vault-wide (MG-01 D11): a tracked masternode belongs to no wallet. The keychain keys are the wallet's. |
| D4 | The cap is `TxDraft`'s outflow (foreign outputs plus the fee), charged at `prepare` from the `masternode` budget. It bounds what any draft under the lease takes; it does not check that the draft is a ProTx (MG-04's builders are its callers). A `MasternodeOp{0}` grant leases, but nothing prepares under it. | That is the net debit plus fees the row asks for: a `FundNew` collateral pays the wallet and is not counted, and the fee is. It is the measure `Spend` and quick unlock already use. |
| D5 | A vote is a row-less fence artifact (`ArtifactKind::Vote`) with no charge and no journal row, handed off when it enters the store. Its authority is a live voting lease; the fence refuses one handed off unleased. | I2 and I5 then cover votes: none enters the store after a lock returns. A journal row would protect nothing, because a lost vote is cast again with no harm. Refusing the unleased case makes the authority structural, as for a row-less asset lock (review r1 F5). |
| D6 | `vote` returns `Vec<VoteCast>`, one per masternode with its own refusal; argument, sync and grant errors fail the whole call. mg-engine `Contract-Version` 4 → 5. | Core's `vote-many` reports per masternode. One rate-limited masternode must not hide nine relayed votes. `vote` was a stub that nothing bound, so no client breaks. |
| D7 | A batch with no connected peer succeeds, with `peers: 0`. | The votes are kept and announced on the next connection (MG-02 D24). An error would hide which votes were taken. |
| D8 | Flows `voting`, `masternode_registration` and `key_change` now. "Proposal", "Credit withdrawal", "Shared masternode" and "Dissolution" come with MG-06, MG-13 and MG-04b. | A flow with no caller would be dead surface; each row adds its flow with its grant. |
| D9 | No data-key rotation exists, so the `mnkey/` ids need no re-keying (MG-01 §7). | The data key is made only with a new vault (`create`, `recover_with_mnemonic`), whose records start empty. A future rotation must re-key them; the vault test that pins the id derivation fails first. |
| D10 | BL-246: `current_votes` parses its outpoint as Core's `getcurrentvotes` does: the txid by `ParseHashV` (64 hex characters, any case; else `invalid_argument`) and the index by `LocaleIndependentAtoi<uint32_t>`. Outpoints then compare as values. The null outpoint (a zero txid, index `u32::MAX`, however written) is no filter, as `GetCurrentVotes` takes it. | Folded in, as a few lines. `atoi_u32` moves from the console into `dw-governance` so that both share it. |
| D11 | dw-app's "authenticate every payment" setting covers `MasternodeOp` and `Governance`. | A masternode transaction moves duffs out of the wallet, as `Spend` does. A vote moves none, but MG-00 asks for "N votes, one passphrase", and a vote cast near the cutoff can't be changed for an hour (`too_often`); with the setting on, a renderer cannot cast votes silently (review r1 M1). Quick unlock still issues it at value 0 (D2). |
| D12 | Lease errors reach `vote`'s caller as `GovernanceError::Platform(PlatformError)`, with the `platform.*` code (mg-engine §4). The console maps it to `RPC_MISC_ERROR`, dw-app to the platform error it carries. | The lease is E0-04's, so its errors are too; folding them into `Engine` would lose the code a host already handles for every other flow. |
| D13 | `VoteCast.rejected` adds `cancelled`, `signer_unavailable` and `in_flight` to the reasons of §5. | A lock mid-batch, a vault failure, and the fence's `Deferred` (the same vote in flight or being resolved, e.g. two wallets casting one attached key's vote in the same second) are per-vote outcomes; failing the call would hide the votes already taken. `in_flight` is not the store's `duplicate` (review r1 L5). |
| D14 | A `TxDraft` prepared under a voting lease is `send.grant_invalid`, not `needs_grant`. | The lease has neither a `Spend` nor a `Masternode` budget, so no grant could make it sign a transaction: `needs_grant` would invite a host to add one. It is DP3-01's rule for a lease of the wrong kind. |
| D15 | dw-ffi's frozen `GrantPurpose` shows a `MasternodeOp` grant as `Spend{max_duffs}` and a `Governance` grant as `SignMessage`. | The UniFFI enum is frozen (E0-13), and a host there cannot request the new purposes. The nearest variant states what the grant does. |
| D16 | A vote's fence admission needs the dispatch journal's backend, as every leased artifact's does; without it the vote is `cancelled`. A taken vote ends `Sent` and emits `DispatchResolved`, as other artifacts do. | One admission rule for every kind. The journal is open whenever a session is (E0-04 §6.4), so this refuses only in a broken session. |
| D17 | MG-02's `stub` helper is removed with its last caller; `GovernanceError::NotImplemented` stays. | MG-06's calls arrive as stubs again and need the variant; a helper with no caller is dead code. |
| D18 | dw-app's `MAX_LEASE_BUDGETS` is 6, the number of `BudgetPurpose` variants. | A lease view's budget list is bounded by the purposes; the DTO cap test pins it. |
| D19 | `wallet_owns` leaves out contact-owned accounts (the library's `is_contact_owned`: a DashPay contact's external account). A payment to a contact's address, or change sent there, is foreign to every cap: `Spend`, `Masternode`, the PSBT cap, and the address book's "mine". The PSBT derivation lookups and `sign_message` use the same helper (`own_address_info`): an output to a contact gets no derivation record of this wallet, and an input from one is not counted signable. | Review r1 H1: those addresses are derived from the contact's xpub and listed among the wallet's accounts, so a draft paying one counted as a self-payment, and a `MasternodeOp{fee}` grant could empty the wallet to a contact. DP3-01 D3 worked around it for contact payments only. Tested by `mg05_a_contacts_address_is_charged_as_foreign` and `psbt_a_contacts_address_is_not_the_wallets`. A derivation record on a contact output named this wallet's fingerprint with the contact's key, which a co-signer could read as change (review r1 F2). |
| D20 | Attached-key voting needs a wallet with a seed (a vote signer is the wallet's), and `voting_masternodes` lists keychain keys only; a host votes with an attached key by naming a tracked masternode (MG-03's list). A keychain the vault cannot read, other than locked, falls through to the attached key. | MG-00 asks that `MasternodeVoting` cover attached keys, which it does for any wallet with a seed. A watch-only wallet signs nothing (MG-00 D37). Listing attached-key masternodes in `voting_masternodes` is MG-09's UI call (review r1 L3). |
| D21 | Attaching a key needs the full-scope data key and no grant (MG-01's open question to MG-05); using it to vote needs a `Governance` grant. | Storing a secret the user typed signs nothing and reveals nothing; every use is behind a grant (review r1 L8). |
| D22 | Accepted, not changed: secp256k1 0.33 takes a secret's bytes by value, so one unerased copy passes through its constructor; `with_attached_key` erases the key on drop, panics included. | The crate has no by-reference constructor; `masternode_key_id` already shared the pattern (review r1 L9). |
| D23 | When the keychain could not be read (a vault error other than a lock or a missing seed) and no attached key has the list's id, the vote is `signer_unavailable`, not `no_voting_key`. | The keychain may hold the key; `no_voting_key` would tell the user the wallet lacks it (review r1 F4). A wallet without a seed has no keychain keys, so its `no_voting_key` stands. |
