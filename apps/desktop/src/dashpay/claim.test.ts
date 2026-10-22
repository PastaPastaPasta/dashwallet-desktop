// The claim screen's view model (DP5-03), by the design's states C1–C12 and D-rows.

import { describe, expect, it } from "vitest";
import type { InvitationInviter } from "../api/generated/InvitationInviter";
import type { InvitationStatus } from "../api/generated/InvitationStatus";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import { appError } from "../onboarding/testWorld";
import { CLAIM_COPY, claimErrorText, claimModel, claimRow, inviterCard, recipientLines, type ClaimInputs } from "./claim";
import { REG_COPY, requestNote, screenModel, type ScreenInputs } from "./registration";

const LINK = "1".padStart(32, "0");
const BOB = "8GopLQQPjKr2gXjNEt9G8k2PzRZrdbBqQBNTCXV2DYiN";
const amount = (duffs: number) => `${duffs / 1e8} tDASH`;

const inviter = (i: Partial<InvitationInviter> = {}): InvitationInviter => ({ identity: BOB, username: "bob", displayName: "Bob Builder", avatarUrl: null, ...i });
const valid = (v: Partial<Extract<InvitationStatus, { kind: "valid" }>> = {}): InvitationStatus => ({
  kind: "valid",
  inviter: inviter(),
  fundingDuffs: 5_000_000,
  contestedAllowed: false,
  expiresAt: null,
  ...v,
});
const inputs = (i: Partial<ClaimInputs> = {}): ClaimInputs => ({ entry: "join", hasIdentity: false, status: valid(), statusError: null, network: "testnet", ...i });

const row = (r: Partial<RegistrationStatus> = {}): RegistrationStatus => ({
  draft: "claim-1",
  phase: "keysPrepared",
  label: "carol",
  temporaryLabel: null,
  identity: null,
  txid: null,
  waiting: null,
  holdsKey: false,
  running: true,
  fundsCommitted: false,
  contestEndsAt: null,
  failure: null,
  nameFailure: null,
  profileFailure: null,
  invitation: { linkId: LINK, inviter: BOB, requestFailure: null },
  createdAt: 0,
  updatedAt: 0,
  ...r,
});
const screen: ScreenInputs = { contestPending: false, contestEndsAt: null, date: String, amount, discreet: false };

describe("C1, C2 (D5, F7): the claim's own row comes before the status", () => {
  it("finds the row of this link, in progress or done, not one failed for good", () => {
    expect(claimRow([row()], LINK)?.draft).toBe("claim-1");
    expect(claimRow([row({ phase: "done" })], LINK)?.draft).toBe("claim-1");
    expect(claimRow([row({ invitation: null })], LINK)).toBeUndefined();
    expect(claimRow([row({ invitation: { linkId: "2".padStart(32, "0"), inviter: null, requestFailure: null } })], LINK)).toBeUndefined();
    const failed = row({ phase: "failed", failure: { phase: "keysPrepared", code: "invitation.claimed", retryable: false, needed: null, available: null } });
    expect(claimRow([failed], LINK)).toBeUndefined();
    const retry = row({ phase: "failed", failure: { phase: "keysPrepared", code: "platform.unavailable", retryable: true, needed: null, available: null } });
    expect(claimRow([retry], LINK)?.draft).toBe("claim-1");
    expect(claimRow(undefined, LINK)).toBeUndefined();
  });

  it("C1: the progress screen steps say what a claim does (D10)", () => {
    expect(screenModel(row({ phase: "draft" }), screen).step).toEqual({ n: 1, text: "Checking the invitation" });
    expect(screenModel(row(), screen).step).toEqual({ n: 2, text: "Creating identity" });
    expect(screenModel(row({ phase: "nameRequested", identity: BOB }), screen).step?.n).toBe(3);
  });

  it("C2 (F2, D8): Done says whether the request went out, and nothing when none was asked", () => {
    const done = (invitation: RegistrationStatus["invitation"]) => row({ phase: "done", identity: BOB, invitation });
    expect(screenModel(done({ linkId: LINK, inviter: BOB, requestFailure: null }), screen).lines).toEqual([REG_COPY.done.message, REG_COPY.done.requestSent]);
    expect(screenModel(done({ linkId: LINK, inviter: BOB, requestFailure: "contact.ineligible" }), screen).lines).toEqual([
      REG_COPY.done.message,
      REG_COPY.done.requestRefused,
    ]);
    expect(screenModel(done({ linkId: LINK, inviter: null, requestFailure: null }), screen).lines).toEqual([REG_COPY.done.message]);
    expect(requestNote(row({ invitation: null, phase: "done" }))).toBeNull();
    expect(requestNote(row())).toBeNull();
  });
});

describe("C3–C9: the invitation before a claim", () => {
  it("C3: a wallet with an identity is told so, before any read", () => {
    const m = claimModel(inputs({ hasIdentity: true, status: undefined }));
    expect(m).toMatchObject({ state: "hasIdentity", lines: [CLAIM_COPY.hasIdentity] });
    expect(m.actions.map((a) => a.kind)).toEqual(["dismiss", "notNow"]);
  });

  it("C4, C5: checking, then a failed read with its reason and Try again", () => {
    expect(claimModel(inputs({ status: undefined }))).toMatchObject({ state: "checking", busy: true, lines: [CLAIM_COPY.checking] });
    const failed = claimModel(inputs({ status: undefined, statusError: appError("platform.unavailable") }));
    expect(failed.state).toBe("checkFailed");
    expect(failed.lines).toEqual([CLAIM_COPY.checkFailed, "Could not reach the Dash network. Try again."]);
    expect(failed.actions.map((a) => a.kind)).toEqual(["retry", "notNow"]);
  });

  it("C6, C7: claimed offers Dismiss; expired only Not now", () => {
    const claimed = claimModel(inputs({ status: { kind: "claimed" } }));
    expect(claimed).toMatchObject({ state: "claimed", lines: [CLAIM_COPY.claimed.title, CLAIM_COPY.claimed.text] });
    expect(claimed.actions.map((a) => a.kind)).toEqual(["dismiss"]);
    const expired = claimModel(inputs({ status: { kind: "expired" } }));
    expect(expired.state).toBe("expired");
    expect(expired.actions.map((a) => a.kind)).toEqual(["notNow"]);
  });

  it("C8: each invalid reason in its words; an unknown funding can be checked again", () => {
    const invalid = (reason: "malformed" | "wrongNetwork" | "assetLockNotFound") => claimModel(inputs({ status: { kind: "invalid", reason } }));
    expect(invalid("malformed").lines).toEqual([CLAIM_COPY.invalid.malformed]);
    expect(invalid("wrongNetwork").lines).toEqual(["This invitation is for another Dash network than testnet"]);
    expect(invalid("assetLockNotFound").lines).toEqual([CLAIM_COPY.invalid.assetLockNotFound]);
    expect(invalid("assetLockNotFound").actions.map((a) => a.kind)).toEqual(["retry", "dismiss"]);
    expect(invalid("malformed").actions.map((a) => a.kind)).toEqual(["dismiss"]);
  });

  it("C9 (D4, F7): the inviter card never says verified, only what DPNS proves", () => {
    const m = claimModel(inputs());
    expect(m.state).toBe("invited");
    expect(m.actions.map((a) => a.kind)).toEqual(["accept", "notNow", "dismiss"]);
    expect(m.lines).toEqual([CLAIM_COPY.invited.paid]);
    expect(m.inviter).toEqual({
      name: "Bob Builder",
      username: "bob",
      identity: BOB,
      avatarUrl: null,
      line: "Dash Wallet checked that @bob is a DashPay username. It cannot check who sent you this link.",
    });
    const all = JSON.stringify(m).toLowerCase();
    expect(all).not.toMatch(/verified|trusted/);
    expect(inviterCard(inviter({ username: null }))?.line).toBe(CLAIM_COPY.invited.unnamed);
    expect(claimModel(inputs({ status: valid({ inviter: null }) })).inviter).toBeNull();
  });

  it("D2: the welcome entry's title", () => {
    expect(claimModel(inputs({ entry: "welcome" })).title).toBe("You have been invited to DashPay");
    expect(claimModel(inputs()).title).toBe("DashPay invitation");
  });
});

describe("C11 (Opus-h r1 n3, D6): the request's recipient before Claim", () => {
  it("names the recipient, what the request carries, and the identity whole", () => {
    const r = recipientLines(inviter());
    expect(r?.identity).toBe(BOB);
    expect(r?.text).toMatch(/^A contact request will go to @bob\./);
    expect(r?.text).toMatch(/extended public key/);
    expect(recipientLines(inviter({ username: null }))?.text).toContain(`@${BOB}`);
    expect(recipientLines(null)).toBeNull();
  });
});

describe("C12: the claim's refusals", () => {
  it("says each invitation refusal in the claim's words; the rest as the wizard does", () => {
    expect(claimErrorText(appError("invitation.claimed"), amount)).toBe(`${CLAIM_COPY.claimed.title}. ${CLAIM_COPY.claimed.text}`);
    expect(claimErrorText(appError("invitation.expired"), amount)).toMatch(/no longer saved/);
    expect(claimErrorText(appError("invitation.invalid"), amount)).toBe(CLAIM_COPY.invalid.malformed);
    expect(claimErrorText(appError("invitation.already_has_identity"), amount)).toBe(CLAIM_COPY.hasIdentity);
    expect(claimErrorText(appError("name.unavailable_for_invite"), amount)).toBe(CLAIM_COPY.name.contestedRefused);
    expect(claimErrorText(appError("registration.funding_insufficient"), amount)).toBe("This invitation does not cover this username.");
    expect(claimErrorText(appError("name.taken"), amount)).toBe("Username taken. Choose another one.");
  });
});
