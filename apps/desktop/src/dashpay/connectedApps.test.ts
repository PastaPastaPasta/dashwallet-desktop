import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IpcBackend } from "../api/tauri";
import { CONNECT_LINKS } from "../fixture/connect";
import { IDENTITY } from "../fixture/dashpay";
import { installFixture } from "../fixture/index";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID } from "../fixture/scenarios";
import { needsPassphrase } from "../tools/grant";
import { ConnectedAppsModel, connectedKeyCritical, connectedKeyText, disconnectErrorText } from "./connectedApps";

function setup(scenario: "dashpay-connected-apps" | "dashpay-connected-apps-empty" = "dashpay-connected-apps") {
  const fixture = installFixture({ scenario });
  const backend = new IpcBackend("fixture", () => fixture.now, fixture.locale);
  let refreshed = 0;
  const model = new ConnectedAppsModel(backend, FIXTURE_WALLET_ID, () => {
    refreshed++;
  });
  return { fixture, backend, model, refreshed: () => refreshed };
}

beforeEach(() => vi.stubGlobal("window", globalThis));
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("Connected apps list and empty state", () => {
  it("lists approved apps with claimed name, App ID, return host, key scopes, limit, expiry and approval date", async () => {
    const { backend } = setup("dashpay-connected-apps");
    const res = await backend.appConnectList(FIXTURE_WALLET_ID, IDENTITY.alice);
    expect(res.identity).toBe(IDENTITY.alice);
    expect(res.apps).toHaveLength(2);
    const [yappr, forum] = res.apps;
    expect(yappr.label).toBe("Yappr");
    expect(yappr.returnsTo).toBe("yappr.example");
    expect(yappr.keys).toHaveLength(2);
    expect(connectedKeyText(yappr.keys[0])).toContain("owner and admins can add contracts");
    expect(connectedKeyCritical(yappr.keys[0])).toContain("doesn't cap tokens");
    expect(connectedKeyText(yappr.keys[1])).toBe("A key that can edit your DashPay profile");
    expect(connectedKeyCritical(yappr.keys[1])).toBeNull();
    expect(forum.label).toBe("Dash Forum");
    expect(forum.returnsTo).toBeNull();
    expect(connectedKeyText(forum.keys[0])).toBe("A key for “post” documents");
  });

  it("returns an empty list when no apps are connected", async () => {
    const { backend } = setup("dashpay-connected-apps-empty");
    const res = await backend.appConnectList(FIXTURE_WALLET_ID, IDENTITY.alice);
    expect(res.apps).toEqual([]);
  });

  it("records a connected app when sendFailed happens after keys are added and recovers on retry", async () => {
    const fixture = installFixture({ scenario: "dashpay-registered" });
    const linkId = CONNECT_LINKS.sendFailed;
    fixture.world.links.push({ kind: "appConnect", linkId, returnsTo: "yappr.example" });
    const backend = new IpcBackend("fixture", () => fixture.now, fixture.locale);
    await backend.takeLinks(fixture.world.launch.network.dirName);
    const sheet = await backend.appConnectSheet(FIXTURE_WALLET_ID, linkId);
    const choice = {
      identity: sheet.identities[0].identity,
      selection: sheet.bindings.map((b) => b.index),
      terms: sheet.terms,
    };
    const quote = await backend.appConnectQuote(FIXTURE_WALLET_ID, linkId, choice);
    const grant = await backend.authorize(quote.grant, FIXTURE_WALLET_ID, {
      kind: "passphrase",
      passphrase: FIXTURE_PASSPHRASE,
    });
    await expect(backend.appConnectApprove(FIXTURE_WALLET_ID, linkId, choice, grant.id)).rejects.toMatchObject({
      code: "connect.send_failed",
    });
    const afterFail = await backend.appConnectList(FIXTURE_WALLET_ID, sheet.identities[0].identity);
    expect(afterFail.apps.some((a) => a.request === sheet.request)).toBe(true);
    await backend.appConnectRetrySend(FIXTURE_WALLET_ID, linkId);
    const afterRetry = await backend.appConnectList(FIXTURE_WALLET_ID, sheet.identities[0].identity);
    expect(afterRetry.apps.some((a) => a.request === sheet.request)).toBe(true);
  });
});

describe("Disconnect confirm with fee, passphrase, success and errors", () => {
  it("always requires the passphrase on an encrypted vault, even while unlocked", () => {
    const unlocked = {
      state: "unlocked" as const,
      encrypted: true,
      quickUnlockEnrolled: true,
      failedAttempts: 0,
      retryAfterSecs: null,
      walletsWithSecrets: [FIXTURE_WALLET_ID],
    };
    const purpose = { kind: "appDisconnect", maxCredits: "22000000", identity: "22".repeat(32), request: "1a2b".repeat(10) } as const;
    expect(needsPassphrase(purpose, unlocked, false)).toBe(true);
    expect(needsPassphrase(purpose, { ...unlocked, encrypted: false }, false)).toBe(false);
  });

  it("quotes the live Platform credit fee, asks for the passphrase, disconnects and refreshes the list", async () => {
    const { fixture, backend, model, refreshed } = setup("dashpay-connected-apps");
    const { apps } = await backend.appConnectList(FIXTURE_WALLET_ID, IDENTITY.alice);
    const yappr = apps[0];

    await model.openDisconnect(yappr);
    expect(model.target?.request).toBe(yappr.request);
    expect(model.quoting).toBe(false);
    expect(model.quoteError).toBeNull();
    expect(model.quote?.fee).toBe("22000000");
    expect(model.quote?.grant.kind).toBe("appDisconnect");

    const done = model.confirmDisconnect();
    await vi.waitFor(() => expect(model.grants.pending).not.toBeNull());
    // Wrong passphrase stays on the prompt and refuses.
    expect(await model.grants.submit("wrong-pass")).toBe(false);
    expect(model.grants.pending).not.toBeNull();
    // Correct passphrase completes disconnect.
    expect(await model.grants.submit(FIXTURE_PASSPHRASE)).toBe(true);
    await done;

    expect(model.target).toBeNull();
    expect(model.disconnecting).toBe(false);
    expect(model.disconnectError).toBeNull();
    expect(model.disconnectedLabel).toBe("Yappr");
    expect(model.lastDone?.keysDisabled).toEqual([6, 7]);
    expect(model.lastDone?.responseMayHaveLanded).toBe(false);
    expect(refreshed()).toBe(1);
    expect(fixture.world.grants.filter((g) => g.grant.purpose.kind === "appDisconnect").map((g) => g.used)).toEqual([true]);

    const after = await backend.appConnectList(FIXTURE_WALLET_ID, IDENTITY.alice);
    expect(after.apps.map((a) => a.label)).toEqual(["Dash Forum"]);
  });

  it("records responseMayHaveLanded when an in-flight sign-in response may still reach the app", async () => {
    const { fixture, backend, model } = setup("dashpay-connected-apps");
    const { apps } = await backend.appConnectList(FIXTURE_WALLET_ID, IDENTITY.alice);
    const yappr = apps[0];

    await model.openDisconnect(yappr);
    fixture.world.disconnectOutcome = "responseInFlight";
    const done = model.confirmDisconnect();
    await vi.waitFor(() => expect(model.grants.pending).not.toBeNull());
    await model.grants.submit(FIXTURE_PASSPHRASE);
    await done;

    expect(model.disconnectedLabel).toBe("Yappr");
    expect(model.lastDone?.responseMayHaveLanded).toBe(true);
  });

  it("carries prior-conflict charge disclosure on re-quoted Disconnect", async () => {
    const { fixture, backend, model } = setup("dashpay-connected-apps");
    const { apps } = await backend.appConnectList(FIXTURE_WALLET_ID, IDENTITY.alice);
    const yappr = apps[0];

    fixture.world.disconnectOutcome = "priorChargeKnown";
    await model.openDisconnect(yappr);
    expect(model.quote?.priorAttemptConflict).toBe(true);
    expect(model.quote?.priorAttemptDebit).toBe("500000");

    fixture.world.disconnectOutcome = "priorChargePossible";
    await model.openDisconnect(yappr);
    expect(model.quote?.priorAttemptConflict).toBe(true);
    expect(model.quote?.priorAttemptDebit).toBeNull();
  });

  it("shows plain errors, refreshes on notConfirmed/pendingFeeTooHigh, and auto-requotes on grant_exceeded, feeTooHigh and disconnectConflict", async () => {
    const { fixture, backend, model, refreshed } = setup("dashpay-connected-apps");
    const { apps } = await backend.appConnectList(FIXTURE_WALLET_ID, IDENTITY.alice);
    const yappr = apps[0];

    // Unverified key refusal at quote time.
    fixture.world.disconnectOutcome = "unverifiedKey";
    await model.openDisconnect(yappr);
    expect(model.quote).toBeNull();
    expect(disconnectErrorText(model.quoteError)).toContain("couldn't confirm which keys on Dash Platform belong to this app");

    // Fee too high refusal at quote time.
    fixture.world.disconnectOutcome = "feeTooHigh";
    await model.openDisconnect(yappr);
    expect(model.quote).toBeNull();
    expect(disconnectErrorText(model.quoteError)).toContain("higher than Dash Wallet's limit");

    // Low credits refusal at quote time.
    fixture.world.disconnectOutcome = "credits";
    await model.openDisconnect(yappr);
    expect(model.quote).toBeNull();
    expect(disconnectErrorText(model.quoteError)).toContain("more in credits to disable these keys");

    // Not confirmed at disconnect time: shows plain error, refreshes list, and keeps the row in the list.
    fixture.world.disconnectOutcome = "ok";
    await model.openDisconnect(yappr);
    expect(model.quote).not.toBeNull();
    fixture.world.disconnectOutcome = "notConfirmed";
    const run = model.confirmDisconnect();
    await vi.waitFor(() => expect(model.grants.pending).not.toBeNull());
    await model.grants.submit(FIXTURE_PASSPHRASE);
    await run;

    expect(model.disconnectError).not.toBeNull();
    expect(disconnectErrorText(model.disconnectError)).toContain("Dash Wallet checks first, so you don't pay twice.");
    expect(refreshed()).toBe(1);
    const stillThere = await backend.appConnectList(FIXTURE_WALLET_ID, IDENTITY.alice);
    expect(stillThere.apps.map((a) => a.label)).toEqual(["Yappr", "Dash Forum"]);

    // Grant exceeded at disconnect time: auto-requotes for the user to review the new fee and never claims nothing was changed.
    const quoteSpy = vi.spyOn(backend, "appConnectDisconnectQuote");
    fixture.world.disconnectOutcome = "grantExceeded";
    const grantRun = model.confirmDisconnect();
    await vi.waitFor(() => expect(model.grants.pending).not.toBeNull());
    await model.grants.submit(FIXTURE_PASSPHRASE);
    await grantRun;

    expect(quoteSpy).toHaveBeenCalledTimes(1);
    expect(model.quote).not.toBeNull();
    expect(model.quoteError).toBeNull();
    expect(disconnectErrorText(model.disconnectError)).toContain("fee changed while disconnecting");
    expect(disconnectErrorText(model.disconnectError).toLowerCase()).not.toContain("nothing was changed");

    // Disconnect conflict at disconnect time: refreshes list and auto-requotes so user starts a fresh re-quoted Disconnect.
    fixture.world.disconnectOutcome = "disconnectConflict";
    const conflictRun = model.confirmDisconnect();
    await vi.waitFor(() => expect(model.grants.pending).not.toBeNull());
    await model.grants.submit(FIXTURE_PASSPHRASE);
    await conflictRun;

    expect(quoteSpy).toHaveBeenCalledTimes(2);
    expect(model.quote).not.toBeNull();
    expect(refreshed()).toBe(2);
    expect(disconnectErrorText(model.disconnectError)).toContain("identity changed on Dash Platform");
    expect(disconnectErrorText(model.disconnectError).toLowerCase()).not.toContain("nothing was changed");

    // Fee too high at disconnect time: auto-requotes (which fails with feeTooHigh -> clears stale quote so confirm stays disabled).
    fixture.world.disconnectOutcome = "feeTooHigh";
    const feeRun = model.confirmDisconnect();
    await vi.waitFor(() => expect(model.grants.pending).not.toBeNull());
    await model.grants.submit(FIXTURE_PASSPHRASE);
    await feeRun;

    expect(quoteSpy).toHaveBeenCalledTimes(3);
    expect(model.quote).toBeNull();
    expect(model.quoteError).not.toBeNull();

    // Pending fee too high at disconnect time: refreshes list and does not say nothing was changed.
    fixture.world.disconnectOutcome = "ok";
    await model.openDisconnect(yappr);
    fixture.world.disconnectOutcome = "pendingFeeTooHigh";
    const pendingFeeRun = model.confirmDisconnect();
    await vi.waitFor(() => expect(model.grants.pending).not.toBeNull());
    await model.grants.submit(FIXTURE_PASSPHRASE);
    await pendingFeeRun;

    expect(refreshed()).toBe(3);
    expect(disconnectErrorText(model.disconnectError)).toContain("earlier attempt may still be in flight");
    expect(disconnectErrorText(model.disconnectError).toLowerCase()).not.toContain("nothing was changed");
  });

  it("maps generic, fee-change, and provably-unsent disconnect errors accurately", () => {
    expect(disconnectErrorText({ code: "platform.unavailable", message: "", data: null })).toBe(
      "Dash Wallet couldn't disconnect this app. Try again.",
    );
    expect(disconnectErrorText({ code: "platform.grant_exceeded", message: "", data: null })).toBe(
      "The Dash Platform fee changed while disconnecting, so review the updated fee and try again.",
    );
    expect(disconnectErrorText({ code: "platform.grant_invalid", message: "", data: null })).toBe(
      "Dash Wallet couldn't disconnect this app. Try again. Nothing was changed.",
    );
    expect(disconnectErrorText({ code: "connect.failed", message: "", data: { kind: "connectFailure", failure: "disconnectConflict" } })).toContain(
      "identity changed on Dash Platform",
    );
    expect(disconnectErrorText({ code: "connect.failed", message: "", data: { kind: "connectFailure", failure: "pendingFeeTooHigh" } })).toContain(
      "earlier attempt may still be in flight",
    );
    expect(disconnectErrorText({ code: "connect.failed", message: "", data: { kind: "connectFailure", failure: "pendingWrite" } })).toContain(
      "hasn't settled on Dash Platform yet",
    );
    expect(disconnectErrorText({ code: "connect.mainnet_off", message: "", data: null })).toBe(
      "This version can sign in to apps on test networks only.",
    );
  });
});
