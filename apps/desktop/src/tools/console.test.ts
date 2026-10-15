// Tools ▸ Console and the grant it asks for, by parity ID (the Swift `ToolsViewModelTests`, ported).

import { describe, expect, it, vi } from "vitest";
import type { ConsoleOutput } from "../api/generated/ConsoleOutput";
import type { GrantPurposeSpec } from "../api/generated/GrantPurposeSpec";
import type { LockStatus } from "../api/generated/LockStatus";
import { appError, status, summary, WALLET_A, WALLET_B } from "../onboarding/testWorld";
import { ConsoleModel, ConsoleText, FONT_SIZES, HISTORY_LIMIT } from "./console";
import { GrantModel, needsPassphrase } from "./grant";
import { fakeVault } from "./testApi";

const SPEND = { kind: "spend", maxDuffs: 100_000_000 } as const;
const PAYEE = "yXdjeVZqmWp4ZcVJ2Hh8hcFhUzFE8wD1Cd";
const TXID = "09".repeat(32);

function fakeConsole() {
  const executed: [string | null, string, string | null][] = [];
  const api = {
    consoleCommands: vi.fn(async () =>
      ["getbalance", "getblockcount", "getnetworkinfo", "sendtoaddress", "walletpassphrase"].map((name) => ({ name, category: "", sensitive: false, available: true, usage: "" })),
    ),
    consoleRedact: vi.fn(async (line: string) => {
      if (line.split('"').length % 2 === 0) throw appError("console.parse_error");
      return line.startsWith("walletpassphrase") ? "walletpassphrase(…)" : line;
    }),
    consoleExecute: vi.fn(async (walletId: string | null, line: string, grantId: string | null): Promise<ConsoleOutput> => {
      executed.push([walletId, line, grantId]);
      const out = (text: string): ConsoleOutput => ({ text, isJson: false, filtered: line });
      if (line === "getblock abc") throw appError("console.rpc_error", { kind: "rpc", rpcCode: -5, rpcMessage: "Block not found" });
      if (line === "getmempoolinfo") throw appError("console.not_available");
      if (line === "getbalance" && walletId === null) throw appError("console.wallet_required");
      if (line.startsWith("sendtoaddress")) {
        if (grantId === null) throw appError("console.authorization_required", { kind: "authorize", purpose: SPEND, walletId });
        if (grantId === "stale") throw appError("vault.grant_invalid");
        return out(TXID);
      }
      return out(line.startsWith("getblockcount") ? "1200000" : "null");
    }),
  };
  return { api, executed };
}

function setup(lock: Partial<LockStatus> = {}) {
  const vault = fakeVault(lock);
  const c = fakeConsole();
  const grants = new GrantModel(vault.api);
  const model = new ConsoleModel(c.api, grants);
  return { model, grants, vault, c };
}

/** Waits for the console to ask for the passphrase. */
async function untilPrompt(grants: GrantModel) {
  await vi.waitFor(() => expect(grants.pending).not.toBeNull());
}

describe("Console (QT-145)", () => {
  it("QT145_welcomeHasTheAntiScamWarningAndClearKeepsIt", async () => {
    const { model } = setup();
    expect(model.entries.map((e) => e.kind)).toEqual(["welcome", "warning"]);
    expect(model.entries[1].text).toMatch(/^WARNING: Scammers have been active/);
    expect(model.entries[0].text).toContain("Ctrl+L to clear screen");
    await model.run("getblockcount");
    model.clear();
    expect(model.entries.map((e) => e.kind)).toEqual(["welcome", "warning"]);
  });

  it("QT145_sensitiveLinesAreRedactedInEchoAndHistory", async () => {
    const { model } = setup();
    expect(await model.run('walletpassphrase "hunter2" 60')).toBe(true);
    expect(model.history).toEqual(["walletpassphrase(…)"]);
    expect(model.entries.some((e) => e.kind === "command" && e.text === "walletpassphrase(…)")).toBe(true);
    expect(model.entries.some((e) => e.text.includes("hunter2"))).toBe(false);
  });

  it("QT145_historyKeepsFiftyAndBrowsesWithArrows", async () => {
    const { model } = setup();
    for (let n = 0; n < 55; n++) await model.run(`getblockcount ${n}`);
    expect(model.history).toHaveLength(HISTORY_LIMIT);
    expect(model.historyUp()).toBe("getblockcount 54");
    expect(model.historyUp()).toBe("getblockcount 53");
    expect(model.historyDown()).toBe("getblockcount 54");
    expect(model.historyDown()).toBe("");
    expect(model.historyDown()).toBeNull();
  });

  it("QT145_parseErrorsAreNotKeptAndRPCErrorsPrintCoreLine", async () => {
    const { model, c } = setup();
    expect(await model.run('getblock "abc')).toBe(false);
    expect(model.error).toBe(ConsoleText.invalidCommandLine);
    expect(model.history).toEqual([]);
    expect(c.executed).toEqual([]);
    await model.run("getblock abc");
    expect(model.error).toBeNull();
    expect(model.entries.at(-1)).toMatchObject({ kind: "error", text: "Block not found (code -5)" });
    await model.run("getmempoolinfo");
    expect(model.entries.at(-1)?.text).toBe("Not available in SPV mode.");
  });

  it("QT145_authorizationRequiredAsksAndRunsTheLineAgain", async () => {
    const { model, grants, vault, c } = setup({ encrypted: true, state: "unlocked" });
    await model.load([summary(WALLET_A, "Main")]);
    const running = model.run(`sendtoaddress ${PAYEE} 1`);
    await untilPrompt(grants);
    expect(grants.pending).toEqual({ purpose: SPEND, walletId: WALLET_A });
    // A wrong passphrase keeps the line waiting; the lock model counts it.
    expect(await grants.submit("nope")).toBe(false);
    expect(grants.lock.message).toMatch(/incorrect/);
    expect(await grants.submit("pw")).toBe(true);
    await running;
    expect(grants.pending).toBeNull();
    expect(model.entries.at(-1)?.text).toBe(TXID);
    expect(c.executed.at(-1)).toEqual([WALLET_A, `sendtoaddress ${PAYEE} 1`, "grant-1"]);
    expect(vault.calls.authorize.at(-1)?.credential).toEqual({ kind: "passphrase", passphrase: "pw" });
    expect(vault.calls.revoked).toEqual([]);
  });

  it("QT145_unencryptedVaultAuthorizesWithoutAPrompt", async () => {
    const { model, grants, vault } = setup({ encrypted: false });
    await model.load([summary(WALLET_A, "Main")]);
    await model.run(`sendtoaddress ${PAYEE} 1`);
    expect(grants.pending).toBeNull();
    expect(model.entries.at(-1)?.text).toBe(TXID);
    expect(vault.calls.authorize.map((a) => a.credential)).toEqual([{ kind: "unencrypted" }]);
  });

  it("QT145_cancelledAuthorizationRunsNothing", async () => {
    const { model, grants, c } = setup({ encrypted: true });
    await model.load([summary(WALLET_A, "Main")]);
    const running = model.run(`sendtoaddress ${PAYEE} 1`);
    await untilPrompt(grants);
    grants.cancel();
    await running;
    expect(model.entries.at(-1)).toMatchObject({ kind: "error", text: ConsoleText.authorizationCancelled });
    expect(c.executed).toHaveLength(1);
  });

  it("QT145_aRefusedGrantIsNotAskedForAgainAndIsRevoked", async () => {
    const { model, vault, c } = setup({ encrypted: false });
    vault.api.authorize.mockResolvedValueOnce({ id: "stale", purpose: SPEND, walletId: WALLET_A, expiresAt: 0, singleUse: true });
    await model.load([summary(WALLET_A, "Main")]);
    await model.run(`sendtoaddress ${PAYEE} 1`);
    expect(model.entries.at(-1)?.text).toBe(ConsoleText.grantInvalid);
    expect(c.executed).toHaveLength(2);
    expect(vault.calls.revoked).toEqual(["stale"]);
  });

  it("QT145_walletSelectorWithTwoWalletsAndFontSize", async () => {
    const { model } = setup();
    await model.load([summary(WALLET_A, "Main"), summary(WALLET_B, "Savings")]);
    expect(model.showsWalletSelector).toBe(true);
    expect(model.walletId).toBe(WALLET_A);
    model.selectWallet(WALLET_B);
    expect(model.entries.at(-1)?.text).toBe('Executing command using "Savings" wallet');
    model.selectWallet(null);
    expect(model.entries.at(-1)?.text).toBe("Executing command without any wallet");
    await model.run("getbalance");
    expect(model.entries.at(-1)?.text).toBe(ConsoleText.walletRequired);
    for (let i = 0; i < 50; i++) model.setFontSize(model.fontSize + 1);
    expect(model.fontSize).toBe(FONT_SIZES.max);
    model.setFontSize(0);
    expect(model.fontSize).toBe(FONT_SIZES.min);
    expect(model.completions("get")).toEqual(["getbalance", "getblockcount", "getnetworkinfo"]);
    expect(model.completions("help")).toEqual(["help", "help-console"]);
  });

  it("one wallet: no selector, and that wallet runs the line", async () => {
    const { model, c } = setup();
    await model.load([summary(WALLET_A, "Main")]);
    expect(model.showsWalletSelector).toBe(false);
    await model.run("getbalance");
    expect(c.executed[0][0]).toBe(WALLET_A);
  });
});

describe("Grants (U-05 D7)", () => {
  it("asks for the passphrase as the vault and the payment policy say", () => {
    const sign = { kind: "signMessage" } as GrantPurposeSpec;
    expect(needsPassphrase(SPEND, status({ encrypted: false }), true)).toBe(false);
    expect(needsPassphrase(SPEND, status(), true)).toBe(true);
    expect(needsPassphrase(SPEND, status(), false)).toBe(false);
    expect(needsPassphrase(SPEND, status({ state: "locked" }), false)).toBe(true);
    expect(needsPassphrase(sign, status(), true)).toBe(false);
    expect(needsPassphrase(sign, status({ state: "unlockedMixingOnly" }), true)).toBe(true);
  });

  it("falls back to the passphrase when the vault asks for it", async () => {
    const { api } = fakeVault();
    api.paymentAuthentication.mockResolvedValue(false);
    api.authorize.mockRejectedValueOnce(appError("vault.credential_required"));
    const grants = new GrantModel(api);
    const asked = grants.obtain(SPEND, WALLET_A);
    await untilPrompt(grants);
    await grants.submit("pw");
    expect((await asked)?.id).toBe("grant-1");
  });

  it("other refusals reach the caller", async () => {
    const { api } = fakeVault({ encrypted: false });
    api.authorize.mockRejectedValueOnce(appError("wallet_not_found"));
    await expect(new GrantModel(api).obtain(SPEND, WALLET_A)).rejects.toMatchObject({ code: "wallet_not_found" });
  });
});
