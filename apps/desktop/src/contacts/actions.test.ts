// ContactActions (DP2-09 D3–D7, D10, D16): each write sizes its grant with `grant_request`, asks
// the vault through the grant model, says its outcome, and holds one action at a time.

import { describe, expect, it, vi } from "vitest";
import type { GrantAction } from "../api/generated/GrantAction";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { GrantModel } from "../tools/grant";
import { fakeVault } from "../tools/testApi";
import { actionError, ContactActions, type ActionApi, type Refresh } from "./actions";

const ME = "4EfA9Jrvv3nnCFdSf7fad59851iiTRZ6Wcu6YVJ4iSeF";
const GRACE = "GrAC3eQhZmKGwKu4R7PDuBkaxWDcXuspNwGM8RHm9ZsZ";

const until = async (cond: () => boolean) => {
  for (let i = 0; i < 100 && !cond(); i++) await new Promise((r) => setTimeout(r, 0));
  expect(cond()).toBe(true);
};

function setup(overrides: Partial<ActionApi> = {}) {
  const { api: vault, calls } = fakeVault();
  const api: ActionApi = {
    grantRequest: vi.fn(async (_w: string, _i: string, a: GrantAction) => ({ maxDuffs: 0, maxCredits: a.kind === "acceptRequest" ? "150000000" : "300000000" })),
    dashpaySendRequest: vi.fn(async () => ({ kind: "pending" as const, request: "1" })),
    dashpayAcceptRequest: vi.fn(async () => ({ kind: "established" as const, request: "1" })),
    dashpayIgnoreContact: vi.fn(async () => null),
    dashpayUnignoreContact: vi.fn(async () => null),
    dashpaySetPrivateDetails: vi.fn(async () => "published" as const),
    dashpayEnableKeys: vi.fn(async () => null),
    ...overrides,
  };
  const refreshed: Refresh[][] = [];
  const grants = new GrantModel(vault);
  const actions = new ContactActions(api, grants, WALLET_A, ME, (r) => refreshed.push(r));
  /** Answers the passphrase prompt the action raised. */
  const authorize = async () => {
    await until(() => grants.pending !== null);
    expect(await grants.submit("pw")).toBe(true);
  };
  return { api, calls, grants, actions, refreshed, authorize };
}

describe("ContactActions", () => {
  it("IOS-073 sends a request under a sendRequest grant, with the scan it answers", async () => {
    const { api, calls, actions, refreshed, authorize } = setup();
    const done = actions.send(GRACE, "Grace Hopper", "scan-1");
    expect(actions.isRunning("send", GRACE)).toBe(true);
    await authorize();
    expect(await done).toEqual({ kind: "send", contact: GRACE, ok: true, text: "Contact request sent to Grace Hopper." });
    expect(api.grantRequest).toHaveBeenCalledWith(WALLET_A, ME, { kind: "sendRequest" });
    expect(calls.authorize[0].purpose).toEqual({ kind: "platformOp", maxDuffs: 0, maxCredits: "300000000" });
    expect(api.dashpaySendRequest).toHaveBeenCalledWith(WALLET_A, ME, GRACE, "scan-1", "grant-1");
    // The contacts and the cached search show the new request (DP2-08 D5).
    expect(refreshed).toEqual([["contacts", "search"]]);
  });

  it("IOS-073 a send that meets their request says they are contacts", async () => {
    const { actions, authorize } = setup({ dashpaySendRequest: vi.fn(async () => ({ kind: "established" as const, request: "2" })) });
    const done = actions.send(GRACE, "Grace", null);
    await authorize();
    expect(await done).toMatchObject({ ok: true, text: "You and Grace are now contacts." });
  });

  it("DP2-08 D5 says the send's refusals in the user's words", async () => {
    const cases: [ReturnType<typeof appError>, string][] = [
      [appError("contact.request_pending"), "A contact request to Grace is already being sent."],
      [appError("contact.scan_expired"), "This QR code has expired. Ask for a new one."],
      [appError("contact.self"), "This is you."],
      [appError("contact.ineligible", { kind: "ineligible", reason: "alreadyContact" }), "Already in your contacts."],
      [appError("contact.ineligible", { kind: "ineligible", reason: "contactCannotReceive" }), "This user cannot receive contact requests yet."],
      [appError("contact.ineligible", { kind: "ineligible", reason: "noDashPayKeys" }), "Enable DashPay first."],
    ];
    for (const [e, text] of cases) {
      const { actions, authorize } = setup({
        dashpaySendRequest: vi.fn(async () => {
          throw e;
        }),
      });
      const done = actions.send(GRACE, "Grace", null);
      await authorize();
      expect(await done).toMatchObject({ ok: false, text });
    }
    expect(actionError(appError("contact.request_pending"))).toBe("A contact request to them is already being sent.");
  });

  it("IOS-075 accepts under a platformOp grant sized by grant_request", async () => {
    const { api, calls, actions, refreshed, authorize } = setup();
    const done = actions.accept(GRACE, "Grace Hopper");
    expect(actions.isRunning("accept", GRACE)).toBe(true);
    await authorize();
    expect(await done).toEqual({ kind: "accept", contact: GRACE, ok: true, text: "You and Grace Hopper are now contacts." });
    expect(calls.authorize[0].purpose).toEqual({ kind: "platformOp", maxDuffs: 0, maxCredits: "150000000" });
    expect(api.dashpayAcceptRequest).toHaveBeenCalledWith(WALLET_A, ME, GRACE, "grant-1");
    expect(refreshed).toEqual([["contacts"]]);
    expect(actions.running).toBeNull();
  });

  it("a cancelled passphrase says nothing and sends nothing", async () => {
    const { api, grants, actions } = setup();
    const done = actions.accept(GRACE, "Grace");
    await until(() => grants.pending !== null);
    actions.cancel();
    expect(await done).toBeNull();
    expect(actions.outcome).toBeNull();
    expect(api.dashpayAcceptRequest).not.toHaveBeenCalled();
  });

  it("D4 says a refusal in the user's words and revokes the unused grant", async () => {
    const { calls, actions, authorize } = setup({
      dashpayAcceptRequest: vi.fn(async () => {
        throw appError("platform.insufficient_credits", { kind: "credits", needed: "150000000", available: "4000000" });
      }),
    });
    const done = actions.accept(GRACE, "Grace");
    await authorize();
    expect(await done).toMatchObject({ ok: false, text: "Not enough credits. Top up your credits and try again." });
    expect(calls.revoked).toEqual(["grant-1"]);
  });

  it("L2 an accept that finds them contacts already succeeds, and every outcome re-reads", async () => {
    const { actions, refreshed, authorize } = setup({
      dashpayAcceptRequest: vi.fn(async () => {
        throw appError("contact.already_contact");
      }),
    });
    const done = actions.accept(GRACE, "Grace");
    await authorize();
    expect(await done).toMatchObject({ ok: true, text: "You and Grace are now contacts." });
    expect(refreshed).toEqual([["contacts"]]);
  });

  it("runs one action at a time", async () => {
    const { actions, authorize } = setup();
    const first = actions.accept(GRACE, "Grace");
    expect(await actions.ignore(GRACE)).toBeNull();
    await authorize();
    expect((await first)?.ok).toBe(true);
  });

  it("D5, D6 ignores without a grant; unignore re-reads the network search", async () => {
    const { calls, actions, refreshed } = setup();
    expect((await actions.ignore(GRACE))?.text).toBe("Request ignored. You can find it under Hidden.");
    expect((await actions.unignore(GRACE))?.text).toBe("The request is back in Contact Requests.");
    expect(calls.authorize).toEqual([]);
    expect(refreshed).toEqual([["contacts"], ["contacts", "search"]]);
  });

  it("IOS-074 D7 publishes the private details and revokes the grant whatever the engine used", async () => {
    const { api, calls, actions, authorize } = setup({ dashpaySetPrivateDetails: vi.fn(async () => "deferredUntilTwoContacts" as const) });
    const details = { alias: "Gracie", note: null, hidden: false };
    const done = actions.saveDetails(GRACE, details);
    await authorize();
    expect((await done)?.text).toBe("Saved. It is published once you have two contacts.");
    expect(api.dashpaySetPrivateDetails).toHaveBeenCalledWith(WALLET_A, ME, GRACE, details, "grant-1");
    expect(calls.revoked).toEqual(["grant-1"]);
  });

  it("IOS-072 D10 enables the keys and re-reads the identities", async () => {
    const { api, calls, actions, refreshed, authorize } = setup();
    const done = actions.enable();
    await authorize();
    expect((await done)?.text).toBe("DashPay is enabled.");
    expect(calls.authorize[0].purpose).toEqual({ kind: "platformOp", maxDuffs: 0, maxCredits: "300000000" });
    expect(api.dashpayEnableKeys).toHaveBeenCalledWith(WALLET_A, ME, "grant-1");
    expect(refreshed).toEqual([["identities"]]);
    // The engine leaves the grant unused when the keys were there: never left live.
    expect(calls.revoked).toEqual(["grant-1"]);
  });

  it("names the refusals D4 lists", () => {
    expect(actionError(appError("contact.ineligible", { kind: "ineligible", reason: "noDashPayKeys" }))).toBe("Enable DashPay first.");
    expect(actionError(appError("contact.already_contact"))).toBe("You are already contacts.");
    expect(actionError(appError("platform.timeout"))).toBe("Could not reach Dash Platform. Try again.");
    expect(actionError(appError("contact.details_too_long", { kind: "size", bytes: 2100, max: 2031 }))).toBe("The alias and note are too long.");
    expect(actionError(appError("identity.not_found", null, "identity not found"))).toBe("identity not found");
  });
});
