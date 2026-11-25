// "Sign in to an app": refusals said by code only (connect.failed's detail from a fixed list), the
// passphrase every time, and the flow over the fixture backend's IPC: the sheet, the primary key
// that stays, re-quotes, Deny, Approve to done, a send that failed and its retry, a request that
// changed on Platform, and mainnet off.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppError } from "../api/generated/AppError";
import type { ConnectFailureName } from "../api/generated/ConnectFailureName";
import type { LockStatus } from "../api/generated/LockStatus";
import { IpcBackend } from "../api/tauri";
import { CONNECT_LINKS, type ConnectOutcome } from "../fixture/connect";
import { installFixture } from "../fixture/index";
import { FIXTURE_WALLET_ID } from "../fixture/scenarios";
import { t } from "../i18n";
import { needsPassphrase } from "../tools/grant";
import { bindingText, ConnectFlow, connectErrorText, creditsDash, criticalText, expiryDays, FAILURE_KEYS, failureDetail, limitCredits, limitText, quotedLimit, validLimit } from "./connect";
import type { ConnectBinding } from "../api/generated/ConnectBinding";

const err = (code: AppError["code"], data: AppError["data"] = null, message = "a message the engine wrote"): AppError => ({ code, message, data });

describe("connect.failed's detail", () => {
  const NAMES: Record<ConnectFailureName, true> = { keyConflict: true, notConfirmed: true, noResponseKey: true, nothingToSend: true, scopeChanged: true };
  const generic = failureDetail(null);

  it("maps every failure name to its own text", () => {
    const texts = (Object.keys(NAMES) as ConnectFailureName[]).map((failure) => failureDetail({ kind: "connectFailure", failure }));
    for (const text of texts) expect(text).not.toBe(generic);
    expect(new Set(texts).size).toBe(texts.length);
  });

  it("falls back to a generic detail for anything else", () => {
    expect(generic).toBe("something went wrong on the way.");
    for (const data of [undefined, "keyConflict", { kind: "connectFailure", failure: "toString" }, { kind: "connectFailure" }, { kind: "credits", failure: "keyConflict" }])
      expect(failureDetail(data)).toBe(generic);
  });

  it("is said without the error's own text", () => {
    const text = connectErrorText(err("connect.failed", { kind: "connectFailure", failure: "notConfirmed" }), "testnet");
    expect(text).toBe("Sign-in didn't finish: Dash Platform didn't confirm the new keys in time. They may still be added; if you try again, Dash Wallet checks your identity first.");
    expect(connectErrorText(err("connect.failed"), "testnet")).toBe("Sign-in didn't finish: something went wrong on the way.");
  });

  it("adds what is on the identity only where the engine proved it, never after notConfirmed", () => {
    const added = "The keys were added to your identity.";
    const suffix = (failure: string) => {
      const text = connectErrorText(err("connect.failed", { kind: "connectFailure", failure } as AppError["data"]), "testnet");
      return text.endsWith(added) ? "added" : null;
    };
    const pinned = { keyConflict: null, notConfirmed: null, noResponseKey: null, nothingToSend: null, scopeChanged: "added", unknown: null } as const;
    expect(FAILURE_KEYS).toEqual(pinned);
    for (const [failure, keys] of Object.entries(pinned)) expect([failure, suffix(failure)]).toEqual([failure, keys]);
    expect(connectErrorText(err("connect.failed", { kind: "connectFailure", failure: "scopeChanged" }), "testnet")).toBe(
      "Sign-in didn't finish: the app changed its contracts or keys on Dash Platform after your keys were added. The keys were added to your identity.",
    );
  });
});

describe("refusals", () => {
  it("say each code's own text, never the message", () => {
    expect(connectErrorText(err("connect.stale"), "testnet")).toBe("This sign-in request has expired. Ask the app for a new one.");
    expect(connectErrorText(err("connect.owner"), "testnet")).toBe("Dash Wallet refused this request: the app asked for keys covering contracts it doesn't own.");
    expect(connectErrorText(err("connect.network", { kind: "connectNetwork", requested: "mainnet" }), "testnet")).toBe(
      "This request is for Mainnet, but this wallet uses Testnet.",
    );
    expect(connectErrorText(err("connect.network"), "testnet")).toBe("Couldn't check this request with Platform. Try again.");
    expect(connectErrorText(err("connect.credits", { kind: "credits", needed: "120000000", available: "4000000" }), "testnet")).toBe(
      "Your identity needs 116,000,000 more in credits to add these keys. Top it up, then try again.",
    );
    expect(connectErrorText(err("platform.unavailable"), "testnet")).toBe("Couldn't check this request with Platform. Try again.");
    expect(connectErrorText(new Error("boom"), "testnet")).toBe("Couldn't check this request with Platform. Try again.");
  });
});

describe("the grant", () => {
  const lock: LockStatus = { state: "unlocked", encrypted: true, quickUnlockEnrolled: false, failedAttempts: 0, retryAfterSecs: null, walletsWithSecrets: [] };
  const purpose = { kind: "appConnect", maxCredits: "1", request: "00".repeat(20), plan: "00".repeat(32) } as const;

  it("asks for the passphrase even while the vault is unlocked and payments need none", () => {
    expect(needsPassphrase(purpose, lock, false)).toBe(true);
    expect(needsPassphrase({ kind: "signMessage" }, lock, false)).toBe(false);
    expect(needsPassphrase(purpose, { ...lock, encrypted: false, state: "unencrypted" }, false)).toBe(false);
  });
});

describe("helpers", () => {
  it("reads the limit in Dash and the end as whole days", () => {
    expect(["1", " 0.05 ", "0,00000001"].map(limitCredits)).toEqual([100_000_000_000n, 5_000_000_000n, 1_000n]);
    const max = "500000000000";
    expect(["", "0", "0.000000001", "1.123456789", "-3", "1e9", "abc"].map((l) => validLimit(l, max))).toEqual([false, false, false, false, false, false, false]);
    expect(limitCredits("1.123456789")).toBeNull();
    expect(limitText("5000000000")).toBe("0.05");
    expect(["5", "5.00000001", "6"].map((l) => validLimit(l, max))).toEqual([true, false, false]);
    expect(["2", "2.00000001"].map((l) => validLimit(l, "200000000000"))).toEqual([true, false]);
    expect(t("dashpay.connect.limitInvalid", { max: limitText(max) })).toBe("Enter an amount of Dash more than 0 and at most 5, like 0.5.");
    expect(expiryDays({ blockTimeMs: 0, terms: { fundingLimit: "1", expiresAtMs: 30 * 86_400_000 } })).toBe(30);
    expect(expiryDays({ blockTimeMs: 0, terms: { fundingLimit: "1", expiresAtMs: 1 } })).toBe(1);
  });

  it("shows the limit the quote writes, every digit kept", () => {
    expect([0n, 1n, 100_000_000_000n, 5_000_000_000n, 123_456_789_012n].map(creditsDash)).toEqual(["0", "0.00000000001", "1", "0.05", "1.23456789012"]);
    const quote = { grant: { kind: "appConnect", maxCredits: "1", request: "", plan: "" }, fee: "1", balance: "1", budgets: ["30000000000", "1000000000"] } as const;
    expect(quotedLimit({ ...quote, budgets: [...quote.budgets] })).toBe("0.31");
  });

  it("warns under a critical key, and names tokens where it can reach them", () => {
    const group = { group: "g", owner: "o", admins: [], contracts: [], documentTypes: [], tokens: [] };
    const scopes: ConnectBinding["scope"][] = [
      { kind: "group", group },
      { kind: "contract", contract: "c", tokens: true },
      { kind: "contract", contract: "c", tokens: false },
      { kind: "documentType", contract: "c", documentType: "post" },
      { kind: "profile" },
    ];
    const line = (level: ConnectBinding["level"]) => scopes.map((scope) => criticalText({ index: 0, primary: true, level, scope }));
    const tokens =
      "This key has critical access: it can do anything these contracts let your identity do, including sending the app's tokens your identity holds. The funding limit doesn't cap tokens.";
    const noTokens = "This key has critical access: within what it covers, it can do anything your identity can.";
    expect(line("critical")).toEqual([tokens, tokens, noTokens, noTokens, noTokens]);
    expect(line("high")).toEqual([null, null, null, null, null]);
  });

  it("pins the ruled group sentence, {admins, select}, as its two keys", () => {
    const group = { group: "g", owner: "o", admins: [] as string[], contracts: ["a", "b"], documentTypes: [], tokens: [] };
    const row = (admins: string[]) => bindingText({ index: 2, primary: false, level: "high", scope: { kind: "group", group: { ...group, admins } } });
    expect(row([])).toBe("A key for the app's contracts (2). The app's owner can add contracts to this group later, and this key will work for those too.");
    expect(row(["x"])).toBe("A key for the app's contracts (2). The app's owner and admins can add contracts to this group later, and this key will work for those too.");
  });
});

describe("the flow over the fixture", () => {
  beforeEach(() => {
    vi.stubGlobal("window", globalThis);
    vi.stubGlobal("sessionStorage", { getItem: () => null, setItem: () => undefined, removeItem: () => undefined });
    vi.stubGlobal("location", { reload: () => undefined });
  });
  afterEach(() => {
    clearMocks();
    vi.unstubAllGlobals();
  });

  const settle = () => new Promise((r) => setTimeout(r, 0));

  async function open(outcome: ConnectOutcome, returnsTo: string | null = "yappr.example") {
    const fixture = installFixture({ scenario: "dashpay-registered" });
    const linkId = CONNECT_LINKS[outcome];
    fixture.world.links.push({ kind: "appConnect", linkId, returnsTo });
    const backend = new IpcBackend("fixture");
    await backend.takeLinks(fixture.world.launch.network.dirName);
    const closed = vi.fn();
    const flow = new ConnectFlow(backend, FIXTURE_WALLET_ID, linkId, returnsTo, closed);
    await flow.load();
    await settle();
    return { fixture, flow, closed, backend, link: () => fixture.world.connect![linkId] };
  }

  async function approve(flow: ConnectFlow) {
    const done = flow.approve();
    await vi.waitFor(() => expect(flow.grants.pending).not.toBeNull());
    expect(await flow.grants.submit("demo")).toBe(true);
    await done;
  }

  it("shows the sheet; the primary key can't be left out; changes quote again", async () => {
    const { flow } = await open("ok");
    expect(flow.phase).toBe("sheet");
    expect(flow.sheet).toMatchObject({ label: "Yappr", encryption: true });
    const [primary, profile] = flow.sheet!.bindings;
    const fee = flow.quote!.fee;
    flow.toggle(primary);
    expect(flow.selected(primary)).toBe(true);
    flow.toggle(profile);
    expect(flow.selected(profile)).toBe(false);
    await settle();
    expect(flow.choice()!.selection).toEqual([0, 2]);
    expect(BigInt(flow.quote!.fee)).toBeLessThan(BigInt(fee));
    flow.setLimit("0");
    await settle();
    expect([flow.choice(), flow.quote]).toEqual([null, null]);
  });

  it("Deny answers the request; Return goes back, then the sheet closes", async () => {
    const { flow, closed, link } = await open("ok");
    await flow.deny();
    expect(flow.phase).toBe("denied");
    expect(link().answer).toBe("declined");
    await flow.returnToApp();
    expect(link()).toMatchObject({ returned: true, dismissed: true });
    expect(closed).toHaveBeenCalledOnce();
  });

  it("Deny on a request with no return link closes straight away when no approval was attempted", async () => {
    const { flow, closed, link } = await open("ok", null);
    await flow.deny();
    expect(flow.attempted).toBeNull();
    expect(link().answer).toBe("declined");
    expect(flow.closed).toBe(true);
    expect(closed).toHaveBeenCalledOnce();
  });

  it("Deny on a request with no return link shows the denied state when an approval was attempted", async () => {
    const { flow, closed, link } = await open("notConfirmed", null);
    await approve(flow);
    expect(flow.attempted).not.toBeNull();
    await flow.deny();
    expect(flow.phase).toBe("denied");
    expect(flow.closed).toBe(false);
    expect(closed).not.toHaveBeenCalled();
    expect(link().answer).toBe("declined");
    flow.close();
    expect(closed).toHaveBeenCalledOnce();
  });

  it("Deny on a reopened sheet with no return link shows the denied state after an earlier approval", async () => {
    const { flow } = await open("notConfirmed", null);
    await approve(flow);
    const closed = vi.fn();
    const again = new ConnectFlow(new IpcBackend("fixture"), FIXTURE_WALLET_ID, flow.linkId, null, closed);
    await again.load();
    await again.deny();
    expect(again.phase).toBe("denied");
    expect(again.attempted).not.toBeNull();
    expect(again.closed).toBe(false);
    expect(closed).not.toHaveBeenCalled();
  });

  it("Approve asks for the passphrase every time, then signs in", async () => {
    const { fixture, flow, link } = await open("ok");
    await approve(flow);
    expect(flow.phase).toBe("done");
    expect(flow.done!.keysAdded).toEqual([5, 6, 7]);
    expect(link().answer).toBe("ok");
    expect(fixture.world.grants.filter((g) => g.grant.purpose.kind === "appConnect").map((g) => g.used)).toEqual([true]);
  });

  it("a cancelled passphrase leaves the sheet as it was", async () => {
    const { flow, link } = await open("ok");
    const done = flow.approve();
    await vi.waitFor(() => expect(flow.grants.pending).not.toBeNull());
    flow.grants.cancel();
    await done;
    expect([flow.phase, flow.busy, link().started]).toEqual(["sheet", false, false]);
  });

  it("a send that failed can be retried", async () => {
    const { flow, link } = await open("sendFailed");
    await approve(flow);
    expect(flow.phase).toBe("sendFailed");
    expect(flow.closable).toBe(true);
    expect(t("dashpay.connect.sendFailed")).toBe(
      "The keys were added to your identity, but the app may not have received them yet. Retry sending, or return to the app and start again.",
    );
    expect(t("dashpay.connect.app.unverified")).toBe(
      "Dash Wallet can't check which app sent this request. Approve it only if you just started signing in on that app's own website.",
    );
    await flow.retrySend();
    expect(flow.phase).toBe("done");
    expect(link().answer).toBe("ok");
  });

  it("a request that changed on Platform is read again and says so", async () => {
    const { flow } = await open("planChanged");
    expect(flow.sheet!.bindings).toHaveLength(3);
    await approve(flow);
    await settle();
    expect(flow.phase).toBe("sheet");
    expect(flow.planChanged).toBe(true);
    expect(flow.sheet!.bindings).toHaveLength(2);
    await approve(flow);
    expect(flow.phase).toBe("done");
  });

  it("an approval whose fresh quote read anything other than the sheet showed reads the request again", async () => {
    const { flow, backend, link } = await open("ok");
    const shown = flow.quote!;
    const real = backend.appConnectQuote;
    const quote = vi.spyOn(backend, "appConnectQuote").mockImplementationOnce(async (...args) => {
      const fresh = await real(...args);
      return { ...fresh, grant: { ...fresh.grant, plan: "b".repeat(64) } } as typeof fresh;
    });
    await flow.approve();
    await settle();
    expect([flow.phase, flow.planChanged, flow.grants.pending, link().started]).toEqual(["sheet", true, null, false]);
    await vi.waitFor(() => expect(flow.quote).toEqual(shown));
    quote.mockRestore();
    await approve(flow);
    expect(flow.phase).toBe("done");
  });

  it("keys not confirmed in time leave the request open, and approving again finishes it", async () => {
    const { flow } = await open("notConfirmed");
    const approved = flow.choice();
    await approve(flow);
    expect(flow.phase).toBe("sheet");
    expect(connectErrorText(flow.quoteError, "testnet")).toContain("Dash Wallet checks your identity first.");
    expect(flow.attempted).toEqual(approved);
    flow.setLimit("0.02");
    flow.toggle(flow.sheet!.bindings[1]!);
    expect([flow.limit, flow.choice()]).toEqual(["0.05", approved]);
    await approve(flow);
    expect(flow.phase).toBe("done");
  });

  it("a sheet opened again after an approval that didn't finish keeps that approval's choice", async () => {
    const { flow } = await open("notConfirmed");
    expect(flow.sheet!.earlierApproval).toBeNull();
    flow.setLimit("0.02");
    flow.toggle(flow.sheet!.bindings[1]!);
    await vi.waitFor(() => expect(flow.quote).not.toBeNull());
    const approved = flow.choice();
    await approve(flow);
    const again = new ConnectFlow(new IpcBackend("fixture"), FIXTURE_WALLET_ID, flow.linkId, flow.returnsTo, vi.fn());
    await again.load();
    expect([again.sheet!.earlierApproval, again.attempted, again.choice()]).toEqual([approved, approved, approved]);
    expect([again.editable, again.limit, again.selected(again.sheet!.bindings[1]!)]).toEqual([false, "0.02", false]);
    await again.deny();
    expect(again.phase).toBe("denied");
    expect(again.attempted).toEqual(approved);
  });

  it("a Deny after an approval that didn't finish doesn't say nothing was added", async () => {
    const { flow } = await open("notConfirmed");
    await approve(flow);
    await flow.deny();
    expect(flow.phase).toBe("denied");
    expect(flow.attempted).not.toBeNull();
    expect(t("dashpay.connect.deniedAfterApprove")).toBe(
      "You declined this sign-in request. Keys from your earlier approval may already be on your identity, but the app hasn't received them, so it can't use them.",
    );
    expect(t("dashpay.connect.denied")).toBe("You declined this sign-in request. Nothing was added to your identity.");
  });

  it("mainnet off shows no sheet; a refusal shows its code's text", async () => {
    expect((await open("mainnetOff")).flow.phase).toBe("mainnetOff");
    const stale = (await open("stale")).flow;
    expect(stale.phase).toBe("refused");
    expect(connectErrorText(stale.error, "testnet")).toBe("This sign-in request has expired. Ask the app for a new one.");
    const failed = (await open("failed")).flow;
    await approve(failed);
    expect([failed.phase, connectErrorText(failed.error, "testnet")]).toEqual(["refused", "Sign-in didn't finish: a key for this app is already on your identity, with different limits."]);
  });

  it("a credit shortfall keeps the sheet open, Approve off", async () => {
    const { flow } = await open("credits");
    expect([flow.phase, flow.quote]).toEqual(["sheet", null]);
    expect(connectErrorText(flow.quoteError, "testnet")).toMatch(/^Your identity needs [\d,]+ more in credits/);
  });
});
