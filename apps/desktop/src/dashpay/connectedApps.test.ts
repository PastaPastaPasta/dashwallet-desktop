import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IpcBackend } from "../api/tauri";
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
    const purpose = { kind: "appDisconnect", maxCredits: "22000000", request: "1a2b".repeat(10) } as const;
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
    expect(refreshed()).toBe(1);
    expect(fixture.world.grants.filter((g) => g.grant.purpose.kind === "appDisconnect").map((g) => g.used)).toEqual([true]);

    const after = await backend.appConnectList(FIXTURE_WALLET_ID, IDENTITY.alice);
    expect(after.apps.map((a) => a.label)).toEqual(["Dash Forum"]);
  });

  it("shows plain errors and keeps the row when quote or disconnect fails", async () => {
    const { fixture, backend, model } = setup("dashpay-connected-apps");
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

    // Not confirmed at disconnect time: shows plain error and keeps the row in the list.
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
    const stillThere = await backend.appConnectList(FIXTURE_WALLET_ID, IDENTITY.alice);
    expect(stillThere.apps.map((a) => a.label)).toEqual(["Yappr", "Dash Forum"]);
  });

  it("maps generic and provably-unsent disconnect errors accurately", () => {
    expect(disconnectErrorText({ code: "platform.unavailable", message: "", data: null })).toBe(
      "Dash Wallet couldn't disconnect this app. Try again.",
    );
    expect(disconnectErrorText({ code: "platform.grant_exceeded", message: "", data: null })).toBe(
      "Dash Wallet couldn't disconnect this app. Try again. Nothing was changed.",
    );
    expect(disconnectErrorText({ code: "connect.mainnet_off", message: "", data: null })).toBe(
      "This version can sign in to apps on test networks only.",
    );
  });
});
