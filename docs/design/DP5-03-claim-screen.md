# DP5-03: Invitation claim screen (welcome and Join entries)

ROADMAP DP5-03 (UI, T0 and GL): the screen on DP5-02's claim ([DASHPAY](DASHPAY.md) F18: "OS scheme handler, 'Paste
invitation link' (welcome and Join), vault stash, preview, invite-limited names, preselected request to the
inviter"). It also closes DP5-02's UI follow-ups F2 and F7 and Opus-h r1 n3, and two engine follow-ups, F5 and F11,
which get their own review. The claim is DP5-02's `start_registration` with `Invitation` funding; there is no second
claim path. The screen is a mode of DP1-09's registration wizard on the Join DashPay page, and reuses its name step,
grant prompt, progress and result screens.

| Part | Where |
|---|---|
| View model: the claim screen's states and copy (C1–C12), the recipient lines, the step copy of a claim row | `apps/desktop/src/dashpay/claim.ts`, tests `claim.test.ts` |
| Wizard: the invitation mode, the claim's start and resume grants | `src/dashpay/wizard.ts` (additive), `wizard.test.ts` |
| Queries (`invitation_status` by link id) | `src/dashpay/queries.ts` (additive) |
| Screens | `src/screens/dashpay/Claim.tsx`; `JoinDashPay.tsx` mounts it; `Registration.tsx` takes the claim's step copy |
| Entries | `src/app/shellBridge.ts` (the route carries the link id), `src/app/savedLinksOffer.ts` (welcome), `src/app/ui.ts` |
| Fixture: invitations, the claim run, scenarios | `src/fixture/{links,registration,handlers,scenarios}.ts` |
| Playwright (both entries, every state, resume after a relaunch, axe light and dark, Chromium and WebKit) | `e2e/claim.spec.ts`; baselines `e2e/visual.spec.ts` |
| Engine: the opt-out and the row's invitation fields (UI needs) | `dw-engine/src/platform/{registration,registration_flow,registration_rows}.rs`, `dw-app/src/api/identity/registration.rs` |
| Engine: F5 (discard refusal, stash holds) | `registration.rs` (`discard`), `dw-vault/src/vault/links.rs` (holds), `invitations.rs` |
| Engine: F11 (duplicate-key refusal) | `registration_flow.rs` (`claim_refusal`, the claim step) |
| Contract | [m4 §2.2, §2.9, §7 item 64](../contracts/m4-dashpay-engine.md), Contract-Version 37 |

## UX (UX-SPEC has no claim screen)

The mobile wallets are the reference (iOS's invitation flow: "You have been invited", the inviter's card, the name,
then the usual creation progress; Android's automatic request to the inviter). The desktop page is the Join DashPay
page (`page-joinDashPay`), width `formMaxWidth`, a `card-l1` section `claim` with `data-state` naming the state.

**Entries.** Both open the Join page on the claim screen for one stashed link (`ui.invitation = {linkId, entry}`):

- **welcome**: onboarding finishes and the link stash holds an invitation (opened before any wallet existed, DP5-01).
  The newest invitation opens at once instead of the "Links you opened" sheet; any other saved link is offered at the
  next launch or unlock (D2). The header says "You have been invited to DashPay"; Not now goes Home.
- **Join**: an invitation link opened while a wallet exists (OS link, second instance, Open URI), the sheet's Open,
  or the Join page's "Have an invitation?" paste (D14). Not now goes back to the Join page.

**States** (the view model's `claimState`, in this order; the first that holds wins):

| # | Input | State | Shows | Actions |
|---|---|---|---|---|
| C1 | A registration row of the wallet whose `invitation.linkId` is this link, not done | `inProgress` | DP1-09's progress screens (R1–R8) for the row, with the claim's steps: "(1/3) Checking the invitation", "(2/3) Creating identity", "(3/3) Registering username" (D10) | the row's (Continue, Unlock, …) |
| C2 | That row, done | `done` | "Hello @x, your account is ready" + the request line: "A contact request was sent to your inviter." / "The contact request to your inviter could not be sent. You can add them from Contacts." (F2, D8) / none (no inviter, or opted out) | Done |
| C3 | The wallet has an identity (no such row) | `hasIdentity` | "This wallet already has a DashPay identity. An invitation creates a first identity; it can be used with another wallet." | Dismiss invitation, Not now |
| C4 | `invitation_status` loading | `checking` | "Checking the invitation…" (busy) | Not now |
| C5 | The read failed (`platform.*`) | `checkFailed` | "Could not check the invitation. Your wallet works as usual." + reason | Try again, Not now |
| C6 | `Claimed` | `claimed` | "This invitation has already been used" + "Each invitation creates one identity. Ask for a new one." | Dismiss invitation |
| C7 | `Expired` | `expired` | "This invitation is no longer saved" + "Open the invitation link again." | Not now |
| C8 | `Invalid{malformed}` / `{wrongNetwork}` / `{assetLockNotFound}` | `invalid` | "This invitation link is not valid" / "This invitation is for another Dash network" (+ the window's network) / "The network does not know this invitation's funding yet. Try again later." | Dismiss (Try again for the last) |
| C9 | `Valid`, step 0 | `invited` | The inviter card (D4), "Paid by the invitation: nothing is paid from this wallet.", for a contested-capable one nothing else | Accept invitation, Not now, Dismiss invitation |
| C10 | `Valid`, step 1 | `name` | DP1-09's name step; an invitation that cannot buy a contested name refuses one with "This invitation cannot pay for a username the network votes on. Use a number 2–9, or 20 characters or more." | Continue, Back |
| C11 | `Valid`, step 2 | `confirm` | "You have chosen @x…" (DP1-09's), the recipient lines (n3, D6), the request check (F2, D7), "I accept" for a contested label | Claim (the grant, then the start), Back |
| C12 | A start or a resume refused | the step it was on | the refusal's line (`claimErrorText`): `invitation.claimed` → C6's line; `.expired`, `.invalid`, `.already_has_identity`, `name.unavailable_for_invite`, `registration.funding_insufficient` ("This invitation does not cover this username.") | as the step |

## Decisions

| # | Decision | Why |
|---|---|---|
| D1 | The claim is a wizard mode, `{kind: "invitation", linkId, contestedAllowed}`, on the Join page; its name step, grant prompt (U-05's `GrantModel`), progress and result screens are DP1-09's components. `claim.ts` holds only what differs: the states before the start, the copy, the recipient lines and the claim's step text. | The brief: reuse DP1-09's progress, resume and name step; one state machine for Home, the chip and the wizard (DP1-09 D11). |
| D2 | Welcome entry: after onboarding, the offer reads the stash; with an invitation in it, the newest opens on the claim screen and the sheet is not shown this time (its other links wait for the next launch or unlock, D11 of DP5-01). With none, the sheet is offered as before. | iOS opens the invitation right after setup; two modal offers at once would bury it. |
| D3 | `ui.invitation` becomes `{linkId, entry} | null` (was a boolean, DP5-01 D20); the route `invitation{linkId}` sets it and opens the Join page; leaving the claim (Not now, Done, Dismiss) clears it. The Join page's note about invitations "not available yet" goes. | The screen claims a link by its stash id; the window never holds the text. |
| D4 | F7: the inviter is never shown as verified. The card leads with "@username" from DPNS, the display name from the inviter's DashPay profile beside it (secondary), and the avatar, with "Dash Wallet checked that @bob is a DashPay username. It cannot check who sent you this link." No check mark, no "verified" word. Without an inviter: "This invitation does not name who sent it." | DPNS proves only that the name maps to an identity; a link's `du` can name any user (Opus-h r1 n3). The engine already drops the link's own name and avatar (DP5-02 D6). |
| D5 | F7: the row is read before the status. A row of the wallet whose `invitation.linkId` is this link shows C1/C2 whatever `invitation_status` says (it reads `Claimed` while our own claim runs, and `Expired` once the link is wiped after the identity). | `invitation_status` is one-way and cannot tell our claim from another's. |
| D6 | Opus-h r1 n3: step 2 names the request's recipient before Claim: "A contact request will go to @bob (identity `<base58>`). It carries your new identity's extended public key for payments between you, so @bob can see the payments you make to each other." The identity id is shown whole in mono text. | The user should know whom the claim contacts and what it shares; the identity is the only proved handle. |
| D7 | F2 opt-out: step 2's check "Send a contact request to @bob" (checked, as the mobile wallets preselect it). Unchecked, the start sends `Invitation{link_id, request_inviter: false}`: the engine stores no inviter in the row's funding and plans no request credits, so no request step runs. Engine field, m4 item 64. | D16 of DP5-02 sent it always; the brief allows a minimal engine flag in the contract. Storing no inviter reuses the row's "no inviter" path, so no new phase or flow branch. |
| D8 | F2 note: `RegistrationStatus.invitation` (`{link_id, inviter, request_failure}`) reports a claim row: its link id (D5), the identity asked (`None` without an inviter or when opted out) and, on a `Done` row, the code the request was refused with for good (DP5-02 D15). The code is stored in the row's `funding` JSON (`request_error`) when the request step settles, through a new `RegistrationChange::funding`; no schema migration. | A host cannot tell "sent" from "refused" otherwise. The funding encoding is versioned JSON whose v1 reader ignores unknown fields; sibling MG-04b holds the next appdb migration (V2026102201), so a column would collide. |
| D9 | The claim's grants: the start uses the invitation quote's (`max_duffs` 0). A resume (Continue after a relaunch) takes the engine's `registration_resume_grant` for the row (D20). | The engine refuses a grant with duffs on an invitation row (D8 there). Superseded in review: the first cut quoted the invitation again while the row had no identity, which reads `claimed` once the row's own claim landed and its answer was lost (Opus r1 H1). |
| D10 | A claim row's steps (Home's card, the chip, the progress screen): Draft is "Checking the invitation", KeysPrepared (the claim call) "Creating identity", later phases "Registering username". `stepOf` takes the row's kind. | "Processing payment" is false for a claim: nothing is paid. |
| D11 | F5 (a): `discard_registration` refuses an invitation row that has its identity and is not `Done` ("the claim's identity exists; it finishes its name first"), and one whose identity step may have released signatures (its call step's marker stands or reads `Sent`, `LeaseTable::call_step`), unless a refusal proved the claim did not land: the row failed for good with `invitation.already_has_identity` (Platform refused its keys, D13) or `invitation.claimed` (another claim consumed the lock). Judged twice, as the funds-committed refusal, before and inside the delete's job. Refusal is `invalid_argument` as DP1-02's other discard refusals. "Choose another username" on such a row that can run again says the claim is kept and may be on the network (`CLAIM_COPY.kept`); on one stopped for good it goes back to the invitation's screen, as a discarded claim does (the row stays, and `claimRow` passes over it) (Opus r2 1). | Such a discard leaves an identity on Platform with no name step to finish it (DP5-02 F5). A `Cancelled` call (nothing released) supersedes its marker (`NotSent`), so an untried or refused-before-release claim still discards. Review (Opus r1 M1): `invitation.expired` or `invalid` after a release proves nothing about the claim (the link can go from the disk while the claim is in flight), so they no longer free it. |
| D12 | F5 (b, c): an open claim row **holds** its stash entry: a marker file `links/<id>.hold` (0600, empty) next to the entry. A held entry does not expire (its 30-day TTL is not applied), is never evicted at `MAX_STASHED_LINKS`, is never the duplicate an unlock collapses (DP5-01 D9), and is not forgotten (`forget_stashed_link` refuses it, `InvalidArgument`); a full stash evicts the oldest unheld payment or contact, then the oldest unheld invitation, and refuses the new link ("the link stash is full") when every entry is held. The engine holds the link when the start writes the row and again at each claim step. When a row is discarded or fails for good, at any step, the engine lets go of the hold unless another open claim row of any wallet names the link (the stash is the network's); the claim's wipe lets go, then forgets. At each bring-up the engine lets go of every hold no open claim row names (`retain_stashed_link_holds`): rows that ended while the process was killed, or whose wallet was removed. A row this build cannot run (DEC-171 (3)) whose funding names a link is open: a newer build's claim keeps its hold (Opus r2 9). Remove or Dismiss of a held link says "This invitation is being claimed. Finish that claim first." (`removeErrorText`), and the fixture refuses it too (Opus r2 4). Taking an entry deletes its hold, and the sweep deletes a hold without an entry. The entry's sealed fields and times are not touched, so no reseal and no key is needed: a hold works locked. | Holding the times in the entry would need a reseal (the AAD binds both times, DP5-01 D5), which needs the data key. Review (Opus r1 M1–M3, L4): a dismissal or a dedupe could delete a held entry, and a wallet's ended claim could let go of another wallet's. A re-hold at a claim step of an entry past its 30 days that no load has swept yet keeps it: the claim still owns it. A writer in the vault directory can plant holds; the same writer can do more (DP5-01 D19). |
| D13 | F11: map Platform's duplicate-key refusal of the claim to `invitation.already_has_identity`, not retryable, link kept, rather than running a discovery before the retry. At the pin, `rs-drive-abci/.../identity_create/state/v0/mod.rs:86-120` validates the transition's keys with `validate_unique_identity_public_key_hashes_not_in_state` and, when one is in state, returns `PartiallyUseAssetLockAction` charging `unique_key_already_present` plus the processing fee, with the validator's error; `validate_unique_identity_public_key_hashes_in_state/v1/mod.rs:54-58` (protocol v7 and later, `drive_abci_validation_versions/v7.rs:23`) gives `StateError::DuplicatedIdentityPublicKeyIdStateError`, v0 (earlier protocols) `BasicError::DuplicatedIdentityPublicKeyIdBasicError`. The engine reads it from the SDK error as platform-wallet's `consensus_error_of` does (`rs-platform-wallet/src/error.rs:1236`: `StateTransitionBroadcastError.cause`, `Protocol(ConsensusError)`, `NoAvailableAddressesToRetry`), after D26's own-identity check. | The claim's keys are the seed's keys at the row's index (DP1-01), so a key in state means another identity of this seed holds the index: the same verdict D26 gives before the call. Mapping is immediate and needs no network; a discovery before the retry would cost a pass per resume and still end in the same failure. Our own landed claim meets "consumed" (the outpoint check comes before state validation), never this refusal, and is adopted as before. |
| D14 | The Join page offers "Have an invitation?" (DP1-09 F3): a sheet with a paste field that stashes the text (`stash_invitation`, refused text says "This is not a DashPay invitation link.") and opens the claim screen for its id. The text is cleared from the field once stashed. Onboarding's own "Paste invitation link" is F1. | F18 asks for paste on Join; the stash keeps the bearer text out of the renderer after this one hand-over (a paste was in the renderer anyway). |
| D15 | A contested label on an invitation without `contestedAllowed` is refused on step 1, as DP1-10's instant username refuses one, with its own line; the engine refuses it too (`name.unavailable_for_invite`). | Invite-limited names (F18). |
| D16 | Dismiss invitation (C3, C6, C8, C9) forgets the stashed link (`forget_stashed_link`, works locked) and leaves the claim screen; Not now leaves it and keeps the link (Links you opened offers it again). | DP5-01's Remove and Later, on the screen itself. |
| D17 | The fixture models invitations by stash id (`World.invitations`: each link's `InvitationStatus`; a stashed invitation with none reads valid, inviter @bob), the claim run on the registration run (`advance`: draft → keysPrepared → identityRegistered, wiping the link, → … → done with the request step), the opt-out, a refused request (`DashPayWorld.requestRefusal`, the code the row's `requestFailure` takes), and the engine's refusals (D11, D12 are engine-only and tested there). Scenarios: `invitation-valid`, `invitation-claimed`, `invitation-invalid`, `no-wallet-invitation` (no wallet, the link stashed: the welcome entry), `invitation-claiming` (a claim row parked after a relaunch). | The flows to prove are the UI's; the engine's own rules have Rust tests. |
| D18 | Load flakes (agentbox load 80–120 at the time), each passed alone: `dashpay.spec.ts` "Home card creatingPaused dark" (Chromium; the chip read `registering` within 5 s) failed in a 6-worker run of the affected specs and passed alone twice. After review r1 (load 76–97), `claim.spec.ts` "DP5-03 claim screen light: 0 serious axe issues" (WebKit; `registration-next` still disabled after 5 s, the name check pending) failed in a 2-worker run and passed alone. | Brief rule: rerun alone before blaming the code. The card's state comes from a fixture row this task does not touch; `stepOf`'s claim flag is false for it. |
| D19 | Visual baselines: `visual.spec.ts` gains `dashpay-claim-{invited,claimed,confirm}` (light, dark), and the Join page's `dashpay-join`, `-shortfall`, `-waiting` are re-blessed for the "Have an invitation?" button. `profile-withdraw` fails at the base too (3d0b2b5b changed the withdraw copy without re-blessing) and is left to its owner. | Visual baselines are opt-in (`DW_VISUAL=1`; CI's job is `continue-on-error`), not in the gate. |
| D20 | Engine `registration_resume_grant(draft)` (m4 §2.2, facade `DashPay::registration_resume_grant`): a claim row's resume grant from the row alone, `max_duffs` 0 and `max_credits` the steps it has left: the identity's create without an identity, the name (and a temporary name) before `NameRegistered`, the initial profile, and with an inviter the request before `Done`. A Core row is `invalid_argument`. A row with no step left that costs credits (`Done`; `NameRegistered` or later with no inviter and no profile) gets a zero grant: none of those steps signs with an identity key. The fixture computes the same sums for what it models (no temporary names or profiles, D17). | Opus r1 H1: the invitation's status reads `claimed` once the row's own claim landed, so a host cannot size a resume from it; this was F3. |
| D21 | UI review fixes (Opus r1): the Join page's failed claim row says each invitation code in the claim's words (`failureText`, L1); the claim screen reads the registration rows before the status and shows the rows' error first (L2), and a failed refetch of rows already read keeps the screen (Opus r2 6); a paste refusal other than `invitation.invalid` says its own error (L6), and the fixture's `stash_invitation` refuses other text with `invitation.invalid` as the engine does (it said `invalid_argument`); the request's recipient without a username is the identity id without `@`, and the inviter card leads with the proved `@username`, the profile's display name beside it (nits). | The status is judged against the row (F7), so it must not run on rows that failed to load. |
| D22 | Contract-Version stays 37 for this task: review r1's `registration_resume_grant` was blessed at 37 by setting §3's recorded mark back to the base's 36 first. | Version 37 has not left this branch; one version per change, as items 56 and 63. |
| D23 | Gate load flakes (load 35–180): the gate's Playwright step failed 7 WebKit tests in specs this task does not touch, each on a 30 s timeout: `dashpay-info.spec.ts` D3 Sync now, `onboarding.spec.ts` axe ×3, `governance.spec.ts` create light and dark, `names.spec.ts` "Vote goes off at the end time itself". Rerun alone, those four specs on WebKit passed but for the last three; those passed alone with one worker (load 178). Every other gate step passed. The gate's first run also failed two `dw-app` tests on this task's own gaps (the `registration_resume_grant` capability grant, a coverage row for `Vault::retain_stashed_link_holds`), fixed in 8d0c2ec1. | Brief rule: rerun alone before blaming the code; none of the failing specs reads a claim row or the claim screen. |
| D24 | Opus-h r1 m1: F11 closed for a fresh start too. (a) Engine: `start_registration` refuses a claim, of any link, `invitation.already_has_identity` while the wallet keeps a claim row failed for good with that code (`claim_found_identity`), read in the start's ledger job before a row is written or the grant is used; the quote reads no rows and still answers. (b) UI: such a row is the wallet's identity on the claim screen (C3: Dismiss, Not now; `claimHasIdentity`), and Choose another username keeps it and goes back to the invitation's screen, which reads C3. (c) Engine: `PwLib::claim_invitation` reads `IdentityAssetLockStateTransitionReplayError` for the claim's own outpoint as `DuplicateKeys` (`is_claim_replay`). At the pin `35e96ca1`, rs-drive-abci `verify_is_not_spent/v0/mod.rs:47-79` raises it only for a lock partially consumed whose used tags hold the transition's signable bytes, after the fully consumed check; only an IdentityCreate Platform refused and charged writes a tag (`identity_create/advanced_structure/v0/mod.rs:50-140`: identifier, key structure, proof of possession; `state/v0/mod.rs:85-121`, `state/v1/mod.rs:111`: keys in state). So a replay is the identical claim refused before, which did not land (a landed claim consumes the whole lock and meets `consumed` first), and of those refusals only keys in state can meet a claim whose identifier, keys and signatures the library builds. | The row gate over kicking `discover_identities` on `DuplicateKeys`: a discovery needs an `IdentityScan` grant the claim's run does not hold, so it stays DP5-02 F11's open half (F5). The row is the guard: D11 still lets it be discarded (the refusal disproves a landing), and claims start again after that; the window no longer discards it (b), and no other host path discards a row. |

## Engine and contract (m4 item 64, Contract-Version 37)

- `RegistrationFunding::Invitation{link_id, request_inviter}`: `false` asks no one (D7).
- `RegistrationStatus.invitation: Option<RegistrationInvitation{link_id, inviter, request_failure}>` (D8).
- `discard_registration` refuses the claim rows of D11.
- `registration_resume_grant(draft)`: a claim row's resume grant (D20).
- The stash holds the entries of open claim rows against its TTL and its eviction (D12); a full stash of held
  entries refuses a new link.
- The claim's duplicate-key refusal fails the row `invitation.already_has_identity`, not retryable (D13).
- No new §4 code, so no Swift `engineCodes` change; the contract text changes, so the Swift T0 suite runs.

## Tests and evidence

- vitest: every state C1–C12 (`claim.test.ts`), the wizard's claim start and resume grants (`wizard.test.ts`), the
  entries (`shellBridge.test.ts`, `savedLinks.test.ts`), the fixture's claim (`fixture/registration.test.ts`;
  `fixture.test.ts` pins the list of fixture files, so no new one).
- Playwright `e2e/claim.spec.ts`: both entries; valid → name → confirm (recipient, opt-out) → progress → done; the
  request refused for good; already claimed; our own claim in flight reads in progress; expired, invalid, wallet with an
  identity; resume after a relaunch; axe light and dark; Chromium and WebKit. Baselines in `visual.spec.ts`.
- Rust: F5 (discard refusal by phase and by the identity step's marker; holds against the TTL, the eviction and the
  sweep; release on discard and on a failure for good) and F11 (the classifier on each error shape; the step's
  verdict), with mutants (record `/work/scratch/dw-dp5-03/mut/MUTANTS.md`).
- GL: the real app (Tauri, `agent-gui`) on a fresh profile: a syntactically valid invitation link (made up, funding
  nobody) opened before a wallet exists, onboarding, the welcome entry's claim screen, and a second link opened in the
  wallet (Join entry). No real claim or invitation. Screenshots under `/work/scratch/dw-dp5-03/gl/`: with no Platform on
  the run, the welcome entry reads `checkFailed` (Try again, Not now); a forwarded mainnet invitation reads "another Dash
  network" and a bad txid "not valid", both checked locally; Dismiss forgets the shown link and returns to the Join page.

## Reviews

- Opus r1 (code-review-validator): REQUIRES CHANGES (H1, M1–M3, L1–L7, nits). H1, M1–M3, L1, L2, L4, L6 and the nits
  are fixed (D11, D12, D20, D21); L3, L5, L7 are F4–F6.
- Opus r2: ACCEPT WITH NITS. 1, 4–10 fixed (D11, D12, D20, D21); 2 and 3 are F8 and F9.
- Opus-h r1 (`/work/scratch/dw-dp5-03-review/DP5-03-r1-opus.md`): ACCEPT WITH NITS. m1 fixed (D24); n1 in F5; n2 in
  F4; n3 a comment on `removeErrorText` (a narrower code would be a §4 change); n4 in DP5-02 F8.

## Follow-ups

- F1: "Paste invitation link" on onboarding's welcome screen (F18's welcome paste); the welcome entry here is the
  replayed link only.
- F2: DP5-02 F8, F9, F10 stay open there.
- F3: (done as D20.)
- F4 (Opus r1 L3, Opus-h r1 n2): the claim screen shows the inviter of the current status; a row started under
  another inviter's link (a re-stash) is not compared with it. Nor is the recipient step 2 shows (n3) bound to the one
  the start asks: `invitation_plan` resolves the link's `du` again. The start should carry the confirmed inviter
  identity and refuse a mismatch (`invitation.invalid` or `invalid_argument`), recording the confirmed one.
- F5 (Opus r1 L5): the classifier keeps the `BasicError` arm (protocols before v7), unreachable on current Platform;
  a discovery on a duplicate-key refusal (to adopt another device's identity at once) is left to DP5-02 F11's
  discovery half. Until it runs, D24's kept row refuses the next claim.
- F6 (Opus r1 L7): Links you opened still lists a held invitation; hiding or labelling it needs a `StashedLinkInfo`
  field (a DP5-01 contract change).
- F7: the second discard judgement (inside the delete's job) is reached only by a race and has no test of its own.
- F8 (Opus r2 2): a claim's wipe forgets its link even when another wallet's claim of the same link is open; that claim
  then fails `invitation.expired` where `invitation.claimed` is the truth. Fix: on a link gone, check the claim's own
  identity id on Platform, or keep the entry while another open row names it.
- F9 (Opus r2 3): a settle reads the open rows, then lets go; a start of another wallet's claim of the same link in
  between keeps no hold until its claim step. Fix: one session lock for the hold's read-and-release and the start's
  hold, or re-hold every open row's link at the bring-up.
- F10 (D24): a Core registration is not refused by a kept `already_has_identity` claim row. If its index is the one the
  claim used, Platform refuses its keys too, from the wallet's own lock; the discovery (DP5-02 F11) would cover both.
