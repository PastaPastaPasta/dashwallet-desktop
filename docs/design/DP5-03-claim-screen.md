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
| D4 | F7: the inviter is never shown as verified. The card shows the avatar and display name from the inviter's DashPay profile and "@username" from DPNS, with "Dash Wallet checked that @bob is a DashPay username. It cannot check who sent you this link." No check mark, no "verified" word. Without an inviter: "This invitation does not name who sent it." | DPNS proves only that the name maps to an identity; a link's `du` can name any user (Opus-h r1 n3). The engine already drops the link's own name and avatar (DP5-02 D6). |
| D5 | F7: the row is read before the status. A row of the wallet whose `invitation.linkId` is this link shows C1/C2 whatever `invitation_status` says (it reads `Claimed` while our own claim runs, and `Expired` once the link is wiped after the identity). | `invitation_status` is one-way and cannot tell our claim from another's. |
| D6 | Opus-h r1 n3: step 2 names the request's recipient before Claim: "A contact request will go to @bob (identity `<base58>`). It carries your new identity's extended public key for payments between you, so @bob can see the payments you make to each other." The identity id is shown whole in mono text. | The user should know whom the claim contacts and what it shares; the identity is the only proved handle. |
| D7 | F2 opt-out: step 2's check "Send a contact request to @bob" (checked, as the mobile wallets preselect it). Unchecked, the start sends `Invitation{link_id, request_inviter: false}`: the engine stores no inviter in the row's funding and plans no request credits, so no request step runs. Engine field, m4 item 64. | D16 of DP5-02 sent it always; the brief allows a minimal engine flag in the contract. Storing no inviter reuses the row's "no inviter" path, so no new phase or flow branch. |
| D8 | F2 note: `RegistrationStatus.invitation` (`{link_id, inviter, request_failure}`) reports a claim row: its link id (D5), the identity asked (`None` without an inviter or when opted out) and, on a `Done` row, the code the request was refused with for good (DP5-02 D15). The code is stored in the row's `funding` JSON (`request_error`) when the request step settles, through a new `RegistrationChange::funding`; no schema migration. | A host cannot tell "sent" from "refused" otherwise. The funding encoding is versioned JSON whose v1 reader ignores unknown fields; sibling MG-04b holds the next appdb migration (V2026102201), so a column would collide. |
| D9 | The claim's grants: the start uses the invitation quote's (`max_duffs` 0). A resume (Continue after a relaunch) quotes the invitation again while the row has no identity; once it has one (the link is wiped, and a wallet with an identity has no invitation quote, DP5-02 §3 step 1) the grant is `max_duffs` 0 and `max_credits` the sum of `grant_request(registerName{label})` and, with an inviter, `grant_request(sendRequest)`. | The engine refuses a grant with duffs on an invitation row (D8 there); `grant_request` is the engine's own sizing (m4: a host never sizes credits from the cost table). No engine call is added. |
| D10 | A claim row's steps (Home's card, the chip, the progress screen): Draft is "Checking the invitation", KeysPrepared (the claim call) "Creating identity", later phases "Registering username". `stepOf` takes the row's kind. | "Processing payment" is false for a claim: nothing is paid. |
| D11 | F5 (a): `discard_registration` refuses an invitation row that has its identity and is not `Done` ("the claim's identity exists; it finishes its name first"), and one whose identity step may have released signatures (its call step's marker stands or reads `Sent`, `LeaseTable::call_step`), unless it failed for good. Judged twice, as the funds-committed refusal, before and inside the delete's job. Refusal is `invalid_argument` as DP1-02's other discard refusals. | Such a discard leaves an identity on Platform with no name step to finish it (DP5-02 F5). A `Cancelled` call (nothing released) supersedes its marker (`NotSent`), so an untried or refused-before-release claim still discards. A failed-for-good claim (`claimed`, `expired`, `invalid`, `already_has_identity`) made no identity of ours, by the checks that failed it. |
| D12 | F5 (b, c): an open claim row **holds** its stash entry: a marker file `links/<id>.hold` (0600, empty) next to the entry. A held entry does not expire (its 30-day TTL is not applied) and is never evicted at `MAX_STASHED_LINKS`; a full stash evicts the oldest unheld payment or contact, then the oldest unheld invitation, and refuses the new link (`InvalidArgument`, "the link stash is full") when every entry is held. The engine holds the link when the start writes the row and again at each claim step, and releases it when the row is discarded or its claim fails for good; deleting the entry (the claim's wipe, a dismissal) deletes its hold, and the sweep deletes a hold without an entry. The entry's sealed fields and times are not touched, so no reseal and no key is needed: a hold works locked. | Holding the times in the entry would need a reseal (the AAD binds both times, DP5-01 D5), which needs the data key. A hold left by a row deleted out of band (a removed wallet) keeps the link until the user removes it under Links you opened: bounded by the open claim rows, which DP5-02 D7 limits to one per wallet without an identity. A writer in the vault directory can plant holds; the same writer can do more (DP5-01 D19). |
| D13 | F11: map Platform's duplicate-key refusal of the claim to `invitation.already_has_identity`, not retryable, link kept, rather than running a discovery before the retry. At the pin, `rs-drive-abci/.../identity_create/state/v0/mod.rs:86-120` validates the transition's keys with `validate_unique_identity_public_key_hashes_not_in_state` and, when one is in state, returns `PartiallyUseAssetLockAction` charging `unique_key_already_present` plus the processing fee, with the validator's error; `validate_unique_identity_public_key_hashes_in_state/v1/mod.rs:54-58` (protocol v7 and later, `drive_abci_validation_versions/v7.rs:23`) gives `StateError::DuplicatedIdentityPublicKeyIdStateError`, v0 (earlier protocols) `BasicError::DuplicatedIdentityPublicKeyIdBasicError`. The engine reads it from the SDK error as platform-wallet's `consensus_error_of` does (`rs-platform-wallet/src/error.rs:1236`: `StateTransitionBroadcastError.cause`, `Protocol(ConsensusError)`, `NoAvailableAddressesToRetry`), after D26's own-identity check. | The claim's keys are the seed's keys at the row's index (DP1-01), so a key in state means another identity of this seed holds the index: the same verdict D26 gives before the call. Mapping is immediate and needs no network; a discovery before the retry would cost a pass per resume and still end in the same failure. Our own landed claim meets "consumed" (the outpoint check comes before state validation), never this refusal, and is adopted as before. |
| D14 | The Join page offers "Have an invitation?" (DP1-09 F3): a sheet with a paste field that stashes the text (`stash_invitation`, refused text says "This is not a DashPay invitation link.") and opens the claim screen for its id. The text is cleared from the field once stashed. Onboarding's own "Paste invitation link" is F1. | F18 asks for paste on Join; the stash keeps the bearer text out of the renderer after this one hand-over (a paste was in the renderer anyway). |
| D15 | A contested label on an invitation without `contestedAllowed` is refused on step 1, as DP1-10's instant username refuses one, with its own line; the engine refuses it too (`name.unavailable_for_invite`). | Invite-limited names (F18). |
| D16 | Dismiss invitation (C3, C6, C8, C9) forgets the stashed link (`forget_stashed_link`, works locked) and leaves the claim screen; Not now leaves it and keeps the link (Links you opened offers it again). | DP5-01's Remove and Later, on the screen itself. |
| D17 | The fixture models invitations by stash id (`DashPayWorld.invitations`: status, inviter, funding), the claim run on the registration run (`advance`: draft → keysPrepared → identityRegistered, wiping the link, → … → done with the request step), the opt-out, a refused request (`failRequest`), and the engine's refusals (D11, D12 are engine-only and tested there). Scenarios: `invitation-valid`, `invitation-claimed`, `invitation-invalid`, `invitation-welcome` (no wallet, the link stashed), `invitation-claiming` (a claim row parked after a relaunch). | The flows to prove are the UI's; the engine's own rules have Rust tests. |

## Engine and contract (m4 item 64, Contract-Version 37)

- `RegistrationFunding::Invitation{link_id, request_inviter}`: `false` asks no one (D7).
- `RegistrationStatus.invitation: Option<RegistrationInvitation{link_id, inviter, request_failure}>` (D8).
- `discard_registration` refuses the claim rows of D11.
- The stash holds the entries of open claim rows against its TTL and its eviction (D12); a full stash of held
  entries refuses a new link.
- The claim's duplicate-key refusal fails the row `invitation.already_has_identity`, not retryable (D13).
- No new §4 code, so no Swift `engineCodes` change; the contract text changes, so the Swift T0 suite runs.

## Tests and evidence

- vitest: every state C1–C12 (`claim.test.ts`), the wizard's claim start and resume grants (`wizard.test.ts`), the
  entries (`shellBridge.test.ts`, `savedLinks.test.ts`), the fixture's claim (`fixture/claim.test.ts`).
- Playwright `e2e/claim.spec.ts`: both entries; valid → name → confirm (recipient, opt-out) → progress → done; the
  request refused for good; already claimed; our own claim in flight reads in progress; expired, invalid, wallet with an
  identity; resume after a relaunch; axe light and dark; Chromium and WebKit. Baselines in `visual.spec.ts`.
- Rust: F5 (discard refusal by phase and by the identity step's marker; holds against the TTL, the eviction and the
  sweep; release on discard and on a failure for good) and F11 (the classifier on each error shape; the step's
  verdict), with mutants (record `/work/scratch/dw-dp5-03/mut/MUTANTS.md`).
- GL: the real app (Tauri, `agent-gui`) on a fresh profile: a syntactically valid invitation link (made up, funding
  nobody) opened before a wallet exists, onboarding, the welcome entry's claim screen, and a second link opened in the
  wallet (Join entry). No real claim or invitation. Screenshots under `/work/scratch/dw-dp5-03/gl/`.

## Follow-ups

- F1: "Paste invitation link" on onboarding's welcome screen (F18's welcome paste); the welcome entry here is the
  replayed link only.
- F2: DP5-02 F8, F9, F10 stay open there.
- F3: An engine `resume_grant` on the row would replace D9's composition.
