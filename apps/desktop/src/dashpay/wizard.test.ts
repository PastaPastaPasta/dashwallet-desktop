// The registration wizard's flow (DP1-09), by parity ID: IOS-067 (Confirm & pay under the quote's
// grant), IOS-068 (Continue, Choose another username, Finish transfers) and the design's D6–D8,
// D10, D12.

import { describe, expect, it, vi } from "vitest";
import { hasCode } from "../api/backend";
import type { RegistrationQuote } from "../api/generated/RegistrationQuote";
import type { RegistrationRequest } from "../api/generated/RegistrationRequest";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { fakeVault } from "../tools/testApi";
import { CLAIM_COPY, CLAIM_KEPT, claimErrorText } from "./claim";
import { QUOTE_LABEL } from "./status";
import { coreRequest, invitationRequest, RegistrationWizard, resetWizards, wizardFor } from "./wizard";

const quote = (label: string): RegistrationQuote => ({
  contested: label.length < 6,
  lockDuffs: 3_000_000,
  feeDuffs: 2_260,
  totalDuffs: 3_002_260,
  remainingCredits: "1000000000",
  availableDuffs: 10e8,
  grant: { maxDuffs: 3_002_261, maxCredits: "2000000000" },
});

const row = (r: Partial<RegistrationStatus> = {}): RegistrationStatus => ({
  draft: "reg-1",
  phase: "fundingSent",
  label: "alice-77",
  temporaryLabel: null,
  identity: null,
  txid: "f0".repeat(32),
  waiting: "authorize",
  holdsKey: false,
  running: false,
  fundsCommitted: true,
  contestEndsAt: null,
  failure: null,
  nameFailure: null,
  profileFailure: null,
  invitation: null,
  createdAt: 0,
  updatedAt: 0,
  ...r,
});

function setup(lock = { encrypted: false }) {
  const calls: string[] = [];
  const vault = fakeVault(lock);
  let rows: RegistrationStatus[] = [];
  const api = {
    ...vault.api,
    registrationQuote: vi.fn(async (_: string, req: RegistrationRequest) => (calls.push(`quote ${req.label}`), quote(req.label))),
    startRegistration: vi.fn(async (_: string, req: RegistrationRequest, grant: string) => (calls.push(`start ${req.label} ${grant}`), "reg-9")),
    startRegistrationPastUnknownFunding: vi.fn(async (_: string, req: RegistrationRequest, grant: string, past: string[]) => {
      calls.push(`start ${req.label} ${grant} past ${past.join(",")}`);
      return "reg-10";
    }),
    resumeRegistration: vi.fn(async (_: string, draft: string, grant: string | null) => (calls.push(`resume ${draft} ${grant}`), null)),
    discardRegistration: vi.fn(async (_: string, draft: string) => (calls.push(`discard ${draft}`), null)),
    finishAssetLocks: vi.fn(async (_: string, grant: string) => (calls.push(`finish ${grant}`), { resumed: 1, completed: 0, stillPending: 0 })),
    registrations: vi.fn(async () => rows),
    grantRequest: vi.fn(async () => ({ maxDuffs: 0, maxCredits: "2000000000" })),
    registrationResumeGrant: vi.fn(async (_: string, draft: string) => (calls.push(`resume grant ${draft}`), { maxDuffs: 0, maxCredits: "3500000000" })),
    registerName: vi.fn(async (_: string, identity: string, label: string, grant: string) => {
      calls.push(`name ${identity} ${label} ${grant}`);
      return { kind: "registered" as const };
    }),
  };
  const wiz = new RegistrationWizard(api, WALLET_A);
  return { wiz, api, calls, vault, setRows: (r: RegistrationStatus[]) => (rows = r) };
}

/** The wizard on step 2 for `label`. */
function atConfirm(wiz: RegistrationWizard, label: string) {
  wiz.begin({ kind: "core" });
  wiz.setLabel(label);
  wiz.next();
}

describe("IOS-067 Confirm & pay", () => {
  it("starts once under a platformOp grant of the quote's caps, then shows the row's progress", async () => {
    const { wiz, calls, vault } = setup();
    atConfirm(wiz, "alice-77");
    await wiz.confirm(quote("alice-77").grant);
    expect(vault.calls.authorize[0]).toMatchObject({ purpose: { kind: "platformOp", maxDuffs: 3_002_261, maxCredits: "2000000000" }, walletId: WALLET_A });
    expect(calls).toEqual(["start alice-77 grant-1"]);
    expect(wiz).toMatchObject({ step: null, draft: "reg-9", running: null, error: null });
  });

  it("D8: a second Confirm while the first runs starts nothing", async () => {
    const { wiz, calls } = setup();
    atConfirm(wiz, "alice-77");
    await Promise.all([wiz.confirm(quote("x").grant), wiz.confirm(quote("x").grant)]);
    expect(calls).toEqual(["start alice-77 grant-1"]);
  });

  it("a cancelled passphrase starts nothing and stays on step 2", async () => {
    const { wiz, calls } = setup({ encrypted: true });
    atConfirm(wiz, "alice-77");
    const running = wiz.confirm(quote("x").grant);
    await vi.waitFor(() => expect(wiz.grants.pending).not.toBeNull());
    wiz.grants.cancel();
    await running;
    expect(calls).toEqual([]);
    expect(wiz).toMatchObject({ step: "confirm", draft: null, error: null });
  });

  it("a refusal is kept for the screen, and its unused grant is revoked", async () => {
    const { wiz, api, vault } = setup();
    api.startRegistration.mockRejectedValueOnce(appError("registration.funding_insufficient", { kind: "duffs", needed: 3_002_260, available: 1 }));
    atConfirm(wiz, "alice-77");
    await wiz.confirm(quote("x").grant);
    expect(wiz.error).toMatchObject({ code: "registration.funding_insufficient" });
    expect(wiz.step).toBe("confirm");
    expect(vault.calls.revoked).toEqual(["grant-1"]);
  });

  it("registration.in_progress shows the registration that goes on instead of starting a second one", async () => {
    const { wiz, api, setRows } = setup();
    setRows([row({ draft: "reg-1", waiting: "instantSend" })]);
    api.startRegistration.mockRejectedValueOnce(appError("registration.in_progress", { kind: "draft", draft: "reg-1" }));
    atConfirm(wiz, "alice-77");
    await wiz.confirm(quote("x").grant);
    expect(wiz).toMatchObject({ step: null, draft: "reg-1", error: null, wedge: null });
  });

  it("D27: a start refused by a row stopped for good shows that row and says why", async () => {
    const { wiz, api, setRows } = setup();
    setRows([row({ draft: "reg-1", phase: "failed", waiting: null, failure: { code: "registration.islock_timeout", phase: "fundingSent", retryable: false, needed: null, available: null } })]);
    api.startRegistration.mockRejectedValueOnce(appError("registration.in_progress", { kind: "draft", draft: "reg-1" }));
    atConfirm(wiz, "alice-77");
    await wiz.confirm(quote("x").grant);
    expect(wiz).toMatchObject({ step: null, draft: "reg-1", error: { code: "registration.in_progress" } });
  });

  it("the wedge: behind a funding that is unknown, it asks, then starts past it (m4 §2.2)", async () => {
    const { wiz, api, calls, setRows } = setup();
    setRows([row({ draft: "reg-1", waiting: "fundingUnknown" })]);
    api.startRegistration.mockRejectedValueOnce(appError("registration.in_progress", { kind: "draft", draft: "reg-1" }));
    atConfirm(wiz, "alice-77");
    await wiz.confirm(quote("x").grant);
    expect(wiz).toMatchObject({ step: "confirm", wedge: ["reg-1"], draft: null });
    await wiz.confirmWedge(quote("x").grant);
    expect(calls).toEqual(["start alice-77 grant-2 past reg-1"]);
    expect(wiz).toMatchObject({ step: null, draft: "reg-10" });
  });

  it("the wedge behind two unknown fundings: the second start passes both (review M4)", async () => {
    const { wiz, api, calls, setRows } = setup();
    setRows([row({ draft: "reg-1", waiting: "fundingUnknown" }), row({ draft: "reg-2", waiting: "fundingUnknown" })]);
    api.startRegistration.mockRejectedValueOnce(appError("registration.in_progress", { kind: "draft", draft: "reg-2" }));
    api.startRegistrationPastUnknownFunding.mockRejectedValueOnce(appError("registration.in_progress", { kind: "draft", draft: "reg-1" }));
    atConfirm(wiz, "alice-77");
    await wiz.confirm(quote("x").grant);
    await wiz.confirmWedge(quote("x").grant);
    expect(wiz.wedge).toEqual(["reg-2", "reg-1"]);
    await wiz.confirmWedge(quote("x").grant);
    expect(calls.at(-1)).toBe("start alice-77 grant-3 past reg-2,reg-1");
    expect(wiz.draft).toBe("reg-10");
  });

  it("review M3: step 2 takes the trimmed label", () => {
    const { wiz } = setup();
    wiz.begin({ kind: "core" });
    wiz.setLabel("  alice-77 ");
    wiz.next();
    expect(wiz.label).toBe("alice-77");
  });

  it("D6: from credits, the name's own grant, then its outcome", async () => {
    const { wiz, api, calls, vault } = setup();
    wiz.begin({ kind: "credits", identity: "ID" });
    wiz.setLabel("bob-77");
    wiz.next();
    await wiz.registerName("ID");
    expect(api.grantRequest).toHaveBeenCalledWith(WALLET_A, "ID", { kind: "registerName", label: "bob-77" });
    expect(vault.calls.authorize[0].purpose).toEqual({ kind: "platformOp", maxDuffs: 0, maxCredits: "2000000000" });
    expect(calls).toEqual(["name ID bob-77 grant-1"]);
    expect(wiz).toMatchObject({ step: null, named: { label: "bob-77", result: { kind: "registered" } } });
  });
});

describe("IOS-068 the row's actions", () => {
  it("D12 Continue: quotes the row's label again for the grant, then resumes the same draft (no second start)", async () => {
    const { wiz, api, calls } = setup();
    await wiz.resume(row());
    expect(api.registrationQuote).toHaveBeenCalledWith(WALLET_A, coreRequest("alice-77"));
    expect(calls).toEqual(["quote alice-77", "resume reg-1 grant-1"]);
    expect(api.startRegistration).not.toHaveBeenCalled();
    expect(wiz.draft).toBe("reg-1");
  });

  it("D7: Choose another username discards an unfunded row and opens step 1; a funded row is never discarded", async () => {
    const { wiz, calls } = setup();
    await wiz.another(row({ fundsCommitted: true }));
    expect(calls).toEqual([]);
    await wiz.another(row({ fundsCommitted: false, phase: "failed" }));
    expect(calls).toEqual(["discard reg-1"]);
    expect(wiz).toMatchObject({ step: "name", mode: { kind: "core" }, label: "" });
  });

  it("D27: from a row stopped for good with its funds committed, step 1 opens and nothing is discarded", async () => {
    const { wiz, calls } = setup();
    await wiz.another(row({ phase: "failed", fundsCommitted: true, failure: { code: "registration.islock_timeout", phase: "fundingSent", retryable: false, needed: null, available: null } }));
    expect(calls).toEqual([]);
    expect(wiz).toMatchObject({ step: "name", mode: { kind: "core" }, draft: null });
  });

  it("R10: after a refused name, another one is paid from the identity's credits", async () => {
    const { wiz, calls } = setup();
    await wiz.another(row({ phase: "done", identity: "ID", fundsCommitted: true, nameFailure: "name.taken" }));
    expect(calls).toEqual([]);
    expect(wiz.mode).toEqual({ kind: "credits", identity: "ID" });
    // Without an identity the next name would need a second lock: nothing opens.
    wiz.close();
    await wiz.another(row({ phase: "done", identity: null, nameFailure: "name.taken" }));
    expect(wiz.step).toBeNull();
  });

  it("D10 Finish transfers: one duff and the quote's credits, and its report", async () => {
    const { wiz, calls, vault } = setup();
    await wiz.finish();
    expect(vault.calls.authorize[0].purpose).toEqual({ kind: "platformOp", maxDuffs: 1, maxCredits: "2000000000" });
    expect(calls).toEqual([`quote ${QUOTE_LABEL}`, "finish grant-1"]);
    expect(wiz.finished).toEqual({ resumed: 1, completed: 0, stillPending: 0 });
  });

  it("D12: a row the page shows is adopted, so its result stays until Done; never over an open step", () => {
    const { wiz } = setup();
    wiz.adopt("reg-1");
    expect(wiz.draft).toBe("reg-1");
    wiz.adopt("reg-2");
    expect(wiz.draft).toBe("reg-1");
    wiz.close();
    wiz.begin({ kind: "core" });
    wiz.adopt("reg-3");
    expect(wiz.draft).toBeNull();
  });

  it("D1: one wizard per wallet while the app runs; a relaunch starts empty", () => {
    const { api } = setup();
    resetWizards();
    const a = wizardFor(api, WALLET_A);
    a.begin({ kind: "core" });
    expect(wizardFor(api, WALLET_A)).toBe(a);
    expect(wizardFor(api, "b2".repeat(32))).not.toBe(a);
    resetWizards();
    expect(wizardFor(api, WALLET_A).step).toBeNull();
  });
});

describe("DP5-03 the claim (invitation mode)", () => {
  const LINK = "1".padStart(32, "0");
  const BOB = { identity: "8GopLQQPjKr2gXjNEt9G8k2PzRZrdbBqQBNTCXV2DYiN", username: "bob", displayName: null, avatarUrl: null };
  const claimRow = (r: Partial<RegistrationStatus> = {}) =>
    row({ draft: "claim-1", phase: "keysPrepared", txid: null, fundsCommitted: false, invitation: { linkId: LINK, inviter: BOB.identity, requestFailure: null }, ...r });

  it("D7: Claim starts the invitation's request, asking the inviter unless unchecked", async () => {
    const { wiz, api } = setup();
    wiz.begin({ kind: "invitation", linkId: LINK, contestedAllowed: false, inviter: BOB });
    wiz.setLabel("carol-77 ");
    wiz.next();
    expect(wiz.request()).toEqual(invitationRequest("carol-77", LINK, true));
    wiz.setRequestInviter(false);
    expect(wiz.request()).toEqual(invitationRequest("carol-77", LINK, false));
    await wiz.confirm({ maxDuffs: 0, maxCredits: "3500000000" });
    expect(api.startRegistration).toHaveBeenCalledWith(WALLET_A, invitationRequest("carol-77", LINK, false), "grant-1");
    expect(wiz.draft).toBe("reg-9");
    // A new claim checks the request again; without an inviter there is no one to ask.
    wiz.begin({ kind: "invitation", linkId: LINK, contestedAllowed: false, inviter: null });
    wiz.setLabel("carol-77");
    expect(wiz.requestInviter).toBe(true);
    expect(wiz.request().funding).toEqual({ kind: "invitation", linkId: LINK, requestInviter: false });
  });

  it("D9, D20: a claim row resumes under the row's own grant, never its invitation's quote", async () => {
    const { wiz, api, vault } = setup();
    await wiz.resume(claimRow());
    expect(api.registrationResumeGrant).toHaveBeenCalledWith(WALLET_A, "claim-1");
    expect(api.registrationQuote).not.toHaveBeenCalled();
    expect(api.grantRequest).not.toHaveBeenCalled();
    expect(vault.calls.authorize[0]).toMatchObject({ purpose: { kind: "platformOp", maxDuffs: 0, maxCredits: "3500000000" } });
    expect(api.resumeRegistration).toHaveBeenCalledWith(WALLET_A, "claim-1", "grant-1");
  });

  it("Choose another username on a failed claim discards it and goes back to the invitation", async () => {
    const { wiz, calls } = setup();
    await wiz.another(claimRow({ phase: "failed", failure: { phase: "keysPrepared", code: "platform.unavailable", retryable: true, needed: null, available: null } }));
    expect(calls).toEqual(["discard claim-1"]);
    expect(wiz).toMatchObject({ step: null, draft: null });
  });

  it("D11: Choose another username on a claim that may have landed keeps it, and says so", async () => {
    const { wiz, api } = setup();
    const failed = claimRow({ phase: "failed", failure: { phase: "keysPrepared", code: "platform.unavailable", retryable: true, needed: null, available: null } });
    api.discardRegistration.mockRejectedValueOnce(appError("invalid_argument"));
    await wiz.another(failed);
    expect(wiz.error).toBe(CLAIM_KEPT);
    expect(claimErrorText(wiz.error, String)).toBe(CLAIM_COPY.kept);
    // Another refusal of a claim's discard, and any of a Core row's, reads as it is.
    api.discardRegistration.mockRejectedValueOnce(appError("platform.unavailable"));
    await wiz.another(failed);
    expect(hasCode(wiz.error, "platform.unavailable")).toBe(true);
    api.discardRegistration.mockRejectedValueOnce(appError("invalid_argument"));
    await wiz.another(row({ phase: "failed", fundsCommitted: false, failure: { phase: "draft", code: "platform.unavailable", retryable: true, needed: null, available: null } }));
    expect(hasCode(wiz.error, "invalid_argument")).toBe(true);
  });

  it("D11 (Opus r2 1): a stopped claim the engine keeps goes back to its invitation, as a discarded one", async () => {
    const { wiz, api } = setup();
    wiz.adopt("claim-1");
    const stopped = claimRow({ phase: "failed", failure: { phase: "keysPrepared", code: "invitation.expired", retryable: false, needed: null, available: null } });
    api.discardRegistration.mockRejectedValueOnce(appError("invalid_argument"));
    await wiz.another(stopped);
    expect(api.discardRegistration).toHaveBeenCalledWith(WALLET_A, "claim-1");
    expect(wiz).toMatchObject({ step: null, draft: null, error: null });
  });
});
