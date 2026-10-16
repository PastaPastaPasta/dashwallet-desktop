import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import * as cmd from "./generated/commands";
import type { AppError, CommandArgs, CommandName, CommandResults } from "./generated/commands";
import type { LockStatus } from "./generated/LockStatus";

// The generated wrappers against a typed fixture backend: each handler takes
// the command's `CommandArgs` and returns its `CommandResults`, so a binding
// change that the fixtures do not follow fails tsc as well as the test.
type Fixture = { [K in CommandName]?: (args: CommandArgs[K]) => CommandResults[K] };

const WALLET = "ab".repeat(32);
const LOCK: LockStatus = {
  state: "locked",
  encrypted: true,
  quickUnlockEnrolled: false,
  failedAttempts: 0,
  retryAfterSecs: null,
  walletsWithSecrets: [WALLET],
};

/** One representative command per facade (and the free functions). */
const fixture: Fixture = {
  // Engine
  open_network: ({ network, settings }) => {
    expect(settings).toBeNull();
    return { network: { kind: "regtest", dirName: network ?? "regtest", unitName: "tDASH" }, lock: LOCK };
  },
  // Vault
  create_vault: ({ passphrase }) => ({ ...LOCK, state: passphrase === null ? "unencrypted" : "unlocked" }),
  // NetworkSession
  create_wallet: ({ wordCount }) => ({ walletId: WALLET, mnemonic: Array(wordCount).fill("abandon").join(" ") }),
  // TxDraft (by handle)
  new_tx_draft: ({ walletId }) => (walletId === WALLET ? 7 : 0),
  draft_set_recipients: ({ draft, recipients }) => {
    expect([draft, recipients.length]).toEqual([7, 1]);
    return null;
  },
  // PreparedTx (by handle)
  prepared_summary: ({ prepared }) => ({
    txid: "cd".repeat(32),
    fee: 226,
    feeRatePerKb: 1000,
    sizeBytes: 226,
    inputs: [],
    outputs: [],
    totalSent: prepared,
    totalDebit: prepared + 226,
    externalSent: prepared,
  }),
  // DashPay
  identities: ({ walletId }) => [
    {
      identity: "4EfA9Jrvv3nnCFdSf7fad59851iiTRZ6Wcu6YVJ4iSeF",
      index: 0,
      names: ["alice"],
      mainName: "alice",
      namesUpdating: false,
      isMain: walletId === WALLET,
      balance: "123456789012345678",
      balanceAsOf: 1_760_000_000_000,
      balanceStale: false,
      hasDashpayKeys: true,
      profile: null,
      unverified: false,
      creditLevel: "ok",
    },
  ],
  // Free functions
  generate_mnemonic: ({ wordCount, language }) => `${language}:${wordCount}`,
  check_username: ({ label }) => ({ valid: true, normalized: label.toLowerCase(), contested: false, rules: [] }),
};

const calls: [string, unknown][] = [];

beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  calls.length = 0;
  mockIPC((name, payload) => {
    calls.push([name, payload]);
    if (name === "lock_status") {
      const e: AppError = { code: "network_not_open", message: "no network is open", data: null };
      throw e;
    }
    const handler = fixture[name as CommandName] as ((a: unknown) => unknown) | undefined;
    if (!handler) throw new Error(`no fixture for ${name}`);
    return handler(payload);
  });
});

afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("generated commands", () => {
  it("reach one command per facade with camelCase arguments", async () => {
    expect((await cmd.openNetwork("regtest", null)).network.dirName).toBe("regtest");
    expect((await cmd.createVault("pw")).state).toBe("unlocked");
    expect((await cmd.createVault(null)).state).toBe("unencrypted");
    expect((await cmd.createWallet(12)).mnemonic.split(" ")).toHaveLength(12);
    const draft = await cmd.newTxDraft(WALLET);
    await cmd.draftSetRecipients(draft, [
      { address: "yXg3", amount: 100_000, subtractFeeFromAmount: false, label: null, message: null },
    ]);
    expect((await cmd.preparedSummary(1000)).totalDebit).toBe(1226);
    // Credits cross as decimal strings: above 2^53, a number would round.
    expect((await cmd.identities(WALLET))[0].balance).toBe("123456789012345678");
    expect(await cmd.generateMnemonic(24, "english")).toBe("english:24");
    expect((await cmd.checkUsername("Alice")).normalized).toBe("alice");

    // Every argument is sent, `null` included.
    expect(calls[0]).toEqual(["open_network", { network: "regtest", settings: null }]);
    expect(calls[2]).toEqual(["create_vault", { passphrase: null }]);
    expect(calls).toContainEqual(["new_tx_draft", { walletId: WALLET }]);
    expect(calls).toContainEqual(["generate_mnemonic", { wordCount: 24, language: "english" }]);
  });

  it("reject with an AppError whose code is in the union", async () => {
    const e = (await cmd.lockStatus().then(
      () => expect.unreachable(),
      (e: unknown) => e,
    )) as AppError;
    expect(e.code).toBe("network_not_open");
    expect(cmd.ERROR_CODES).toContain(e.code);
  });

  it("export a wrapper for every command and no other", () => {
    const camel = (s: string) => s.replace(/_(.)/g, (_, c: string) => c.toUpperCase());
    const wrappers = Object.entries(cmd)
      .filter(([, v]) => typeof v === "function")
      .map(([k]) => k)
      .sort();
    expect(wrappers).toEqual(Object.keys(cmd.COMMANDS).map(camel).sort());
    expect(new Set(Object.values(cmd.COMMANDS))).toEqual(
      new Set(["app", "network", "vault", "wallets", "chain", "history", "backup", "send", "coinjoin", "identity", "social", "console", "units", "shell", "files", "scan", "selftest"]),
    );
  });
});
