import { clearMocks } from "@tauri-apps/api/mocks";
import { listen } from "@tauri-apps/api/event";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { isAppError } from "../api/backend";
import * as cmd from "../api/generated/commands";
import type { AppEvent } from "../api/generated/commands";
import type { HistoryRequest } from "../api/generated/HistoryRequest";
import type { RegistrationFunding } from "../api/generated/RegistrationFunding";
import type { UserRelation } from "../api/generated/UserRelation";
import type { ChannelStateName } from "../api/generated/ChannelStateName";
import { IpcBackend } from "../api/tauri";
import { fixtureSentinels, IGNORED } from "../../scripts/fixture-sentinels.mjs";
import { ERROR_CLASSES } from "./errors";
import { IDENTITY } from "./dashpay";
import { fixtureOptionsFromUrl, FIXTURE_SENTINEL, installFixture, type FixtureOptions } from "./index";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID, SCENARIO_NAMES } from "./scenarios";

const ALL: HistoryRequest = { categories: [], text: null, sort: "newestFirst", cursor: null, limit: 500 };

function backend(opts: FixtureOptions) {
  const fixture = installFixture(opts);
  return { fixture, b: new IpcBackend("fixture", () => fixture.now, fixture.locale) };
}

beforeEach(() => vi.stubGlobal("window", globalThis));
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("fixture backend", () => {
  // vite.config.ts fails a production build holding a module with this marker, wherever it
  // lives: a fixture module copied or renamed out of src/fixture/ keeps it. One whose marker
  // was also removed is found by its literals (scripts/check-bundle.mjs, sentinels.test.ts),
  // which are derived from the modules here. The exact list of files, of every type, makes
  // moving one out or adding one the sentinels do not read (Sol r3: a `.mts`) fail here, and
  // each module must give the check at least one sentinel of its own.
  it("every fixture module carries the production-build marker and has sentinels", async () => {
    const dir = fileURLToPath(new URL(".", import.meta.url));
    const files = (readdirSync(dir, { recursive: true }) as string[])
      .filter((f) => statSync(join(dir, f)).isFile())
      .map((f) => f.split(sep).join("/"))
      .filter((f) => !IGNORED.test(f));
    expect(files.sort()).toEqual([
      "accept.test.ts",
      "addContact.test.ts",
      "contacts.test.ts",
      "dashpay.ts",
      "errors.ts",
      "fixture.test.ts",
      "handlers.ts",
      "history.test.ts",
      "historyContacts.test.ts",
      "index.ts",
      "links.ts",
      "notifications.test.ts",
      "pane.test.ts",
      "payContact.test.ts",
      "registration.test.ts",
      "registration.ts",
      "scan.ts",
      "scenarios.ts",
      "send.test.ts",
      "send.ts",
      "sentinels.test.ts",
      "settings.test.ts",
      "settings.ts",
      "tools.test.ts",
      "tools.ts",
      "topup.test.ts",
      "topup.ts",
      "vault.ts",
      "vectors/accept.json",
      "vectors/history.json",
      "vectors/notifications.json",
      "wallet.ts",
    ]);
    // Only `*.test.ts` is a test (vite.config.ts `test.include`): a `*.test.mts` is a module.
    const modules = files.filter((f) => /\.[cm]?[jt]sx?$/.test(f) && !f.endsWith(".test.ts"));
    expect(modules).toEqual(["dashpay.ts", "errors.ts", "handlers.ts", "index.ts", "links.ts", "registration.ts", "scan.ts", "scenarios.ts", "send.ts", "settings.ts", "tools.ts", "topup.ts", "vault.ts", "wallet.ts"]);
    const { bySource } = await fixtureSentinels(fileURLToPath(new URL("../..", import.meta.url)));
    for (const f of modules) {
      expect(readFileSync(join(dir, f), "utf8").startsWith("// dw-fixture-module"), f).toBe(true);
      expect(bySource.get(`src/fixture/${f}`)?.length ?? 0, f).toBeGreaterThan(0);
    }
    // Test data has no marker, so it is found by its sentinels alone (Sol r4: a `?raw` copy).
    for (const f of files.filter((f) => f.startsWith("vectors/"))) expect(bySource.get(`src/fixture/${f}`)?.length ?? 0, f).toBeGreaterThan(0);
    // The vectors are test data: no module imports them, so no fixture copy can carry them.
    for (const f of modules) expect(readFileSync(join(dir, f), "utf8"), f).not.toMatch(/(import|export|require)[^;]*vectors\//);
  });

  it("is deterministic and newest first", async () => {
    const a = await backend({ scenario: "funded" }).b.historyPage(FIXTURE_WALLET_ID, ALL);
    const b = await backend({ scenario: "funded" }).b.historyPage(FIXTURE_WALLET_ID, ALL);
    expect(a).toEqual(b);
    const times = a.rows.map((r) => r.timestamp ?? 0);
    expect([...times].sort((x, y) => y - x)).toEqual(times);
    expect(a.rows[0].amount).toBe(20_000_000);
  });

  it("pages with a cursor and filters by category", async () => {
    const { b } = backend({ scenario: "large" });
    const first = await b.historyPage(FIXTURE_WALLET_ID, { ...ALL, limit: 200 });
    expect(first.rows).toHaveLength(200);
    expect(first.totalMatching).toBe(10_003);
    const second = await b.historyPage(FIXTURE_WALLET_ID, { ...ALL, cursor: first.nextCursor, limit: 200 });
    expect(second.rows[0].txid).not.toBe(first.rows[0].txid);
    const sent = await b.historyPage(FIXTURE_WALLET_ID, { ...ALL, categories: ["sent"], limit: 50 });
    expect(sent.rows.every((r) => r.category === "sent")).toBe(true);
  });

  it("unlocks only with the fixture passphrase", async () => {
    const { b, fixture } = backend({ scenario: "locked" });
    expect((await b.lockStatus()).state).toBe("locked");
    await expect(b.unlock("nope")).rejects.toMatchObject({ code: "vault.wrong_passphrase", data: { kind: "wrongPassphrase", failedAttempts: 1 } });
    expect((await b.unlock(fixture.world.passphrase)).state).toBe("unlocked");
  });

  it("opens every scenario through the generated commands", async () => {
    for (const scenario of SCENARIO_NAMES) {
      const { b } = backend({ scenario });
      const launch = await b.launchInfo();
      expect(launch.engineVersion, scenario).toBe("fixture");
      expect((await b.openNetwork()).network.dirName, scenario).toBe("testnet");
      const wallets = await b.listWallets();
      const count = scenario.startsWith("no-wallet") ? 0 : scenario.endsWith("two-wallets") ? 2 : 1;
      expect(wallets.length, scenario).toBe(count);
      await b.syncStatus();
      clearMocks();
    }
  });

  it("covers every DashPay relation and channel state, in the right sections", async () => {
    backend({ scenario: "dashpay-registered" });
    const list = await cmd.dashpayContacts(FIXTURE_WALLET_ID, IDENTITY.alice, { sections: [], sort: "displayName", text: null });
    const contacts = list.sections.flatMap((s) => s.contacts.map((c) => ({ ...c, section: s.section })));
    const directory = await cmd.searchUsers(FIXTURE_WALLET_ID, "", 100);
    const relations = new Set<UserRelation>(directory.map((u) => u.relation));
    const channels = new Set<ChannelStateName>(contacts.map((c) => c.channel));
    // Records over the generated unions: a new relation or channel state fails to compile here.
    const allRelations: Record<UserRelation, true> = { contact: true, ignored: true, isSelf: true, pendingIncoming: true, pendingOutgoing: true, stranger: true };
    const allChannels: Record<ChannelStateName, true> = { broken: true, notEstablished: true, ready: true, setupPending: true };
    expect([...relations].sort()).toEqual(Object.keys(allRelations).sort());
    expect([...channels].sort()).toEqual(Object.keys(allChannels).sort());
    expect(contacts.filter((c) => c.section === "requests").every((c) => c.relation === "pendingIncoming")).toBe(true);
    expect(contacts.filter((c) => c.section === "pending").every((c) => c.relation === "pendingOutgoing")).toBe(true);
    expect(contacts.filter((c) => c.section === "hidden").map((c) => c.username).sort()).toEqual(["ivan", "judy"]);
    // The bell waits on the requests section (notifications.test.ts replays the engine's bell).
    const bell = await cmd.dashpayEvents(FIXTURE_WALLET_ID, IDENTITY.alice, null, 10);
    expect(bell.pending.map((c) => c.contact)).toEqual(contacts.filter((c) => c.section === "requests").map((c) => c.contact));
    expect((await cmd.dashpayStatus(FIXTURE_WALLET_ID)).kind).toBe("ready");
  });

  it("accepting a request moves it to contacts and signals the change", async () => {
    backend({ scenario: "dashpay-registered" });
    const seen: AppEvent[] = [];
    await listen<AppEvent>(cmd.ENGINE_EVENT, (e) => seen.push(e.payload));
    const grace = (await cmd.resolveUser(FIXTURE_WALLET_ID, "grace"))!;
    // The engine redeems an authorised platformOp grant (accept.test.ts replays its outcomes).
    const grant = await cmd.authorize({ kind: "platformOp", maxDuffs: 0, maxCredits: "150000000" }, FIXTURE_WALLET_ID, {
      kind: "passphrase",
      passphrase: FIXTURE_PASSPHRASE,
    });
    expect(await cmd.dashpayAcceptRequest(FIXTURE_WALLET_ID, IDENTITY.alice, grace.identity, grant.id)).toMatchObject({ kind: "established" });
    expect((await cmd.dashpayContact(FIXTURE_WALLET_ID, IDENTITY.alice, grace.identity))?.summary).toMatchObject({ relation: "contact", channel: "setupPending" });
    expect(seen).toContainEqual(expect.objectContaining({ kind: "platform", change: { domain: "contacts", identity: IDENTITY.alice } }));
  });

  it("has the DashPay states before registration", async () => {
    backend({ scenario: "dashpay-no-identity" });
    expect(await cmd.dashpayStatus(FIXTURE_WALLET_ID)).toEqual({ kind: "noIdentity", reason: null });
    expect(await cmd.identities(FIXTURE_WALLET_ID)).toEqual([]);
    clearMocks();
    backend({ scenario: "dashpay-registering" });
    expect(await cmd.dashpayStatus(FIXTURE_WALLET_ID)).toEqual({ kind: "registering", draft: "reg-1" });
    expect((await cmd.registrations(FIXTURE_WALLET_ID))[0]).toMatchObject({ phase: "fundingSent", waiting: "instantSend", fundsCommitted: true });
  });

  it("quotes a registration as the engine plans it (DP1-08 D6)", async () => {
    backend({ scenario: "dashpay-no-identity" });
    const quote = (label: string, funding: RegistrationFunding = { kind: "coreBalance" }) =>
      cmd.registrationQuote(FIXTURE_WALLET_ID, { label, temporaryLabel: null, funding, initialProfile: null });
    expect(await quote("dashpaydesktopquote2")).toMatchObject({ contested: false, lockDuffs: 3_000_000, totalDuffs: 3_002_260 });
    expect(await quote("alice")).toMatchObject({ contested: true, lockDuffs: 25_000_000 });
    await expect(quote("-alice")).rejects.toMatchObject({ code: "name.invalid" });
    await expect(quote("alice", { kind: "invitation", linkId: "1" })).rejects.toMatchObject({ code: "platform.not_implemented" });
  });

  it("rejects a command it does not implement with a typed AppError", async () => {
    backend({ scenario: "funded" });
    const e = await cmd.coinjoinStatus(FIXTURE_WALLET_ID).catch((x: unknown) => x);
    expect(isAppError(e)).toBe(true);
    expect(e).toMatchObject({ code: "platform.not_implemented", data: { kind: "call", call: "coinjoin_status" } });
  });

  it("injects a sample of every ErrorData class", async () => {
    for (const cls of ERROR_CLASSES) {
      const { b } = backend({ scenario: "funded", failures: { list_wallets: cls } });
      const e = await b.listWallets().catch((x: unknown) => x);
      expect(isAppError(e), cls).toBe(true);
      expect((e as cmd.AppError).data?.kind ?? "none").toBe(cls);
      // Only the failing command fails.
      expect((await b.lockStatus()).state).toBe("unlocked");
      clearMocks();
    }
  });

  it("emits the engine's events when the vault locks", async () => {
    const { b } = backend({ scenario: "funded" });
    const seen: AppEvent["kind"][] = [];
    const stop = await b.onEvent((e) => seen.push(e.kind));
    const outcome = await b.lock();
    expect(outcome.status.state).toBe("locked");
    expect(seen).toEqual(["vaultLockState", "lockProgress"]);
    expect(await b.lastLockReport()).toEqual(outcome);
    stop();
  });

  it("selects scenarios, failures and delays from the URL", () => {
    expect(fixtureOptionsFromUrl("")).toMatchObject({ scenario: "funded", theme: null, failures: {} });
    expect(fixtureOptionsFromUrl("?scenario=locked&theme=dark")).toMatchObject({ scenario: "locked", theme: "dark" });
    expect(fixtureOptionsFromUrl("?scenario=error-duffs").failures).toEqual({ list_wallets: "duffs" });
    expect(fixtureOptionsFromUrl("?fail=balance:rpc&fail=sync_status:none&delay=50")).toMatchObject({
      failures: { balance: "rpc", sync_status: "none" },
      delayMs: 50,
    });
    expect(() => fixtureOptionsFromUrl("?scenario=nope")).toThrow("unknown fixture scenario");
    expect(() => fixtureOptionsFromUrl("?scenario=error-nope")).toThrow("unknown error class");
    expect(() => fixtureOptionsFromUrl("?fail=balance")).toThrow("expected <command>:<error class>");
  });

  it("carries the sentinel the production bundle check looks for", () => {
    expect(installFixture({ scenario: "funded" }).sentinel).toBe(FIXTURE_SENTINEL);
  });
});
