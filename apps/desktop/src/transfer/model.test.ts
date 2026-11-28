// Transfer's model: which routes exist and when each can start, the route the shortcut opens, the
// picks, the fee line's debounce and one-quote-at-a-time, Max (from the Dash wallet one Core Max quote;
// from Shielded re-quotes, filled only from a quote that took it), and the sheet following its routes'
// flows.

import { afterEach, describe, expect, it, vi } from "vitest";
import type { ShieldedSpendQuote } from "../api/generated/ShieldedSpendQuote";
import type { ShieldOutcome } from "../api/generated/ShieldOutcome";
import { SHIELDED_CODES } from "../api/pending";
import type { PlatformFundQuote } from "../api/generated/PlatformFundQuote";
import type { ShieldQuote } from "../api/generated/ShieldQuote";
import type { FundAmount } from "../api/generated/FundAmount";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { PlatformFundFlow, resetPlatformFunds } from "../platform/fund";
import { MOVE_COPY, PlatformMoveFlow, resetPlatformMoves } from "../platform/move";
import { resetShielded, SHIELDED_COPY, ShieldedFlow } from "../shielded/flow";
import { SHIELDED_SYNC_COPY } from "../shielded/syncing";
import { engineSpendQuote, NOTE_SETS, NOTES_BALANCE } from "../shielded/testNotes";
import { fakeVault } from "../tools/testApi";
import { SHIELDED_SPENDS_COPY } from "../shielded/spends";
import { canSwap, coreMaxNotes, defaultRoute, feeText, heldExitRest, listedPools, mainnetOff, MAX_DERIVE, noRouteText, platformChip, platformToCoreOff, QuoteLine, ROUTES, routeOf, routeWhy, shieldedSpendMax, TRANSFER_COPY, TransferModel, unsupportedText, type MainnetOff, type Pools, type Settled, type Timers } from "./model";
import { showToast } from "../components/toast";

vi.mock("../components/toast", () => ({ showToast: vi.fn() }));

const duffsOf = (a: FundAmount) => (a.kind === "duffs" ? a.duffs : 0);

/** A derive for `QuoteLine.max` in these tests: the whole probe when it was taken, else what a
 * `funding_insufficient` refusal's figures leave room for. */
const fromShortfall = (first: Settled<unknown>, probe: number): number | null => {
  if (first.ok) return probe;
  const e = first.error as { code?: string; data?: { kind?: string; needed?: number | null; available?: number } };
  if (e?.code !== "credits.funding_insufficient" || e.data?.kind !== "duffs" || e.data.needed == null) return null;
  const most = e.data.available! - (e.data.needed - probe);
  return most > 0 ? most : null;
};

/** A Core Max quote of `duffs` (the lock), its coins' sum `duffs + fee`, and what it leaves. */
const coreMaxQuote = (duffs: number, left = 0, arriving = 0) => ({ duffs, feeDuffs: 1_044, totalDuffs: duffs + 1_044, credits: String(duffs * 1000), max: { inputs: 2, leftDuffs: left, arrivingDuffs: arriving } }) as never;

/** Timers run by hand: `tick(ms)` fires those due. */
function manualTimers() {
  let now = 0;
  let next = 0;
  const due = new Map<number, { at: number; f: () => void }>();
  const timers: Timers = {
    set: (f, ms) => {
      due.set(++next, { at: now + ms, f });
      return next;
    },
    clear: (t) => void due.delete(t as number),
  };
  const tick = (ms: number) => {
    now += ms;
    for (const [id, t] of [...due]) if (t.at <= now) (due.delete(id), t.f());
  };
  return { timers, tick, pending: () => due.size };
}

/** A promise answered by hand. */
function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => ((resolve = res), (reject = rej)));
  return { promise, resolve, reject };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

const POOLS: Pools = {
  core: 3_000_000_000,
  shielded: { active: true, syncing: false, duffs: 125_000_000 },
  platform: { duffs: 40_000_000, dust: false, dustText: "dust", block: null, waitingText: MOVE_COPY.waiting.text },
};

function flows() {
  const vault = fakeVault({ encrypted: false });
  const api = {
    ...vault.api,
    shieldFromCore: vi.fn(),
    resumeShield: vi.fn(async (): Promise<ShieldOutcome> => ({ txid: "lock1", state: { kind: "applied", balance: "1" } })),
    shieldedSend: vi.fn(),
    unshieldToCore: vi.fn(),
    unshieldToPlatform: vi.fn(),
    platformAddressesSync: vi.fn(),
    platformFund: vi.fn(),
    resumePlatformFund: vi.fn(),
    platformMoveToCore: vi.fn(),
  };
  return { api, shielded: new ShieldedFlow(api, WALLET_A), fund: new PlatformFundFlow(api, WALLET_A), move: new PlatformMoveFlow(api, WALLET_A) };
}

function model() {
  const f = flows();
  const quotes = { shieldQuote: vi.fn(), unshieldQuote: vi.fn(), unshieldToPlatformQuote: vi.fn(), platformFundQuote: vi.fn() };
  return { t: new TransferModel(f, quotes, WALLET_A, manualTimers().timers), ...f, quotes };
}

afterEach(() => {
  vi.clearAllMocks();
  resetShielded();
  resetPlatformFunds();
  resetPlatformMoves();
});

describe("pending transfers", () => {
  it.each([
    ["broadcast", "Sent"],
    ["queued", "Sending…"],
    ["preparing", "Sending…"],
  ])("closes a %s Shielded withdrawal and shows %s", async (pendingDispatch, label) => {
    const { t, shielded, api } = model();
    api.unshieldToCore.mockResolvedValue({ state: "pending", pendingDispatch, credits: "1000", balance: "2000", to: "fixture", unresolved: null });
    t.start("shieldedToCore");
    shielded.next({ kind: "out", credits: "1000" });

    await shielded.run({ maxDuffs: 0, maxCredits: "1000" });

    expect(api.unshieldToCore).toHaveBeenCalledOnce();
    expect(t.shown).toBe(false);
    expect(showToast).toHaveBeenCalledExactlyOnceWith(label);
  });

  it("closes a broadcast Platform move while keeping its restart hold", async () => {
    const { t, move, api } = model();
    api.platformMoveToCore.mockResolvedValue({ id: "move", credits: "1000", state: "pending", pendingDispatch: "broadcast", expectedDuffs: 1, to: "fixture", feeCredits: null });
    t.start("platformToCore");
    t.review();

    await move.move({ grant: { maxDuffs: 0, maxCredits: "1000" }, to: "fixture", exitOf: null, credits: "1000", feeCredits: "0" });

    expect(t.shown).toBe(false);
    expect(showToast).toHaveBeenCalledExactlyOnceWith("Sent");
    t.start("platformToCore");
    expect(t.begunRoute).toBeNull();
    expect(routeWhy("platformToCore", POOLS, t.flows)).toBe(POOLS.platform!.waitingText);
    expect(api.platformMoveToCore).toHaveBeenCalledOnce();
  });

  it.each([
    ["shielded.spend_refused", true],
    ["shielded.unrecorded_spend", false],
    ["platform.unavailable", false],
    ["platform.broadcast_unknown", false],
  ] as const)("keeps a %s refusal open and only toasts a known failure", async (code, toast) => {
    const { t, shielded, api } = model();
    api.unshieldToCore.mockRejectedValue(appError(code));
    t.start("shieldedToCore");
    shielded.next({ kind: "out", credits: "1000" });

    await shielded.run({ maxDuffs: 0, maxCredits: "1000" });

    expect(t.shown).toBe(true);
    expect(shielded.step).toBe("refused");
    if (toast) expect(showToast).toHaveBeenCalledExactlyOnceWith("Transfer failed");
    else expect(showToast).not.toHaveBeenCalled();
  });

  it("keeps a refused resume available without reporting a failed transfer", async () => {
    const { shielded, api } = model();
    api.resumeShield.mockRejectedValue(appError("platform.unavailable"));

    await shielded.resume("lock1");

    expect(shielded.open).toBe(true);
    expect(shielded.step).toBe("refused");
    expect(showToast).not.toHaveBeenCalled();
  });
});

describe("routes", () => {
  it("are the five with a command; Platform → Shielded has none", () => {
    expect(routeOf("core", "shielded")).toBe("coreToShielded");
    expect(routeOf("core", "platform")).toBe("coreToPlatform");
    expect(routeOf("shielded", "core")).toBe("shieldedToCore");
    expect(routeOf("shielded", "platform")).toBe("shieldedToPlatform");
    expect(routeOf("platform", "core")).toBe("platformToCore");
    expect(routeOf("platform", "shielded")).toBe("unsupported");
    expect(routeOf("core", "core")).toBeNull();
    expect(Object.keys(ROUTES).sort()).toEqual(["coreToPlatform", "coreToShielded", "platformToCore", "shieldedToCore", "shieldedToPlatform"]);
    expect(MAX_DERIVE.shieldedToPlatform).toBe(shieldedSpendMax);
    expect(unsupportedText("platform", "shielded")).toBe(TRANSFER_COPY.unsupported);
    for (const mainnet of [false, true]) {
      expect(unsupportedText("platform", "shielded", mainnet, true)).toBe(SHIELDED_SPENDS_COPY.shieldIn);
      expect(unsupportedText("shielded", "platform", mainnet, true)).toBe(SHIELDED_SPENDS_COPY.unavailable);
      expect(unsupportedText("core", "platform", mainnet, true)).toBe(unsupportedText("core", "platform", mainnet));
      expect(unsupportedText("platform", "shielded", mainnet, false)).toBe(unsupportedText("platform", "shielded", mainnet));
    }
    expect(canSwap("shielded", "platform")).toBe(false);
  });

  it("on mainnet the off routes name no detour through the Dash wallet, whose move to Platform is off there too", () => {
    expect(unsupportedText("shielded", "platform", true)).toBe("This version can move from Shielded to Platform on test networks only.");
    expect(unsupportedText("platform", "shielded", true)).toBe("Moving from Platform to Shielded isn't available in this version.");
    expect(unsupportedText("core", "platform", true)).toBe("This version can add Dash to Platform on test networks only.");
    for (const [from, to] of [["shielded", "platform"], ["platform", "shielded"], ["core", "platform"]] as const) expect(unsupportedText(from, to, true)).not.toMatch(/Dash wallet first/);
    expect(unsupportedText("shielded", "platform")).toBe(TRANSFER_COPY.offOnNetwork);
    expect(unsupportedText("platform", "shielded")).toBe(TRANSFER_COPY.unsupported);
  });

  it("Shielded → Platform and Dash wallet → Platform are off on mainnet, each by its flag; the others are unchanged", () => {
    expect(routeOf("shielded", "platform", true)).toBe("unsupported");
    expect(unsupportedText("shielded", "platform")).toBe(TRANSFER_COPY.offOnNetwork);
    expect(routeOf("core", "platform", true)).toBe("unsupported");
    expect(routeOf("core", "platform")).toBe("coreToPlatform");
    expect(unsupportedText("core", "platform")).toBe("This version can add Dash to Platform on test networks only.");
    expect(routeOf("platform", "shielded", true)).toBe("unsupported");
    for (const [from, to] of [["core", "shielded"], ["shielded", "core"], ["platform", "core"]] as const) expect(routeOf(from, to, true)).toBe(routeOf(from, to));
    expect(canSwap("core", "platform", true)).toBe(true);
    expect(canSwap("platform", "core", true)).toBe(false);
  });

  it("keeps an add to Platform that runs or has an unseen result reachable on mainnet", () => {
    const { t, fund } = model();
    t.setMainnet(true);
    t.pick("from", "core", POOLS);
    t.pick("to", "platform", POOLS);
    expect(t.route).toBe("unsupported");
    fund.step = "done";
    expect(fund.unseen).toBe(true);
    expect(t.route).toBe("coreToPlatform");
    t.setMainnet(false);
    expect(t.route).toBe("coreToPlatform");
  });

  it("Platform → Dash wallet on mainnet: the line in place of Move, the held line while the addresses hold Dash, the exit while earlier payments or moves hold them", () => {
    const fmt = (d: number) => `${d / 1e8} DASH`;
    const on = (mainnet: MainnetOff | undefined): Pools => ({ ...POOLS, platform: { ...POOLS.platform!, mainnet } });
    expect(platformToCoreOff(POOLS)).toBeNull();
    expect(platformChip(POOLS, "from")).toBe(false);
    const empty = mainnetOff("0", 0, fmt);
    expect(platformToCoreOff(on(empty))).toBe("This version can move and send from Platform addresses on test networks only.");
    expect(platformToCoreOff(on(mainnetOff("999", 0, fmt)))).toBe(empty.text);
    expect(platformToCoreOff(on(mainnetOff("40000000000", 0, fmt)))).toBe("0.4 DASH is on your Platform addresses. It's still yours. This version can move it to your Dash wallet on test networks only.");
    expect([platformChip(on(empty), "from"), platformChip(on(empty), "to")]).toEqual([true, true]);
    // One held record: Move stays offered as its exit, and the line names no amount.
    const one = mainnetOff("40000000000", 1, fmt);
    expect(one).toEqual({
      held: true,
      credits: "40000000000",
      text: "A payment or move you made earlier from your Platform balance hasn't finished, and Dash Wallet can't tell yet whether it will go through. You can move what's on those Platform addresses to your Dash wallet instead. Only one of the two can go through, never both. If the earlier one goes through first, its recipient is paid and this move fails. If this move goes through, the earlier one doesn't, so whatever it paid for stays unpaid.",
    });
    expect(one.text).not.toMatch(/\d/);
    expect(platformToCoreOff(on(one))).toBeNull();
    // A record Platform has not answered holds Move: its waiting line, not the off line.
    expect(platformToCoreOff({ ...POOLS, platform: { ...POOLS.platform!, block: "pending", mainnet: empty } })).toBeNull();
    expect([platformChip(on(one), "from"), platformChip(on(one), "to")]).toEqual([false, true]);
    // Several records in one hold: one line with their count, still no amount.
    const several = mainnetOff("40000000000", 2, fmt);
    expect(several.text).toBe(
      "2 payments or moves you made earlier from your Platform balance haven't finished, and Dash Wallet can't tell yet whether they will go through. You can move what's on those Platform addresses to your Dash wallet instead. If one of them goes through first, its recipient is paid and this move fails. If this move goes through, none of them do, so whatever they paid for stays unpaid.",
    );
    expect(several.text.replace(/^2 /, "")).not.toMatch(/\d/);
    // Rest: the credits outside what the exit's quote takes, only when there are some.
    expect(heldExitRest(one, "40000000000", fmt)).toBeNull();
    expect(heldExitRest(one, "39999999500", fmt)).toBeNull();
    expect(heldExitRest(mainnetOff("50000000000", 1, fmt), "40000000000", fmt)).toBe("The rest of your Platform balance, 0.1 DASH, is still yours. This version can move it on test networks only.");
    // Once the hold settles, the line falls back by the balance, with no other notice.
    expect(mainnetOff("10000000000", 0, fmt).text).toBe("0.1 DASH is on your Platform addresses. It's still yours. This version can move it to your Dash wallet on test networks only.");
    expect(mainnetOff("0", 0, fmt).text).toBe(empty.text);
  });

  it("list the pools this dw-app has; Mixed coins is not one", () => {
    expect(listedPools(POOLS)).toEqual(["core", "shielded", "platform"]);
    expect(listedPools({ ...POOLS, shielded: null })).toEqual(["core", "platform"]);
    expect(listedPools({ ...POOLS, shielded: null, platform: null })).toEqual(["core"]);
  });

  it("hold the moves out of Shielded with one reason, and into it with another, where this build cannot spend from it", () => {
    const { shielded, fund, move } = flows();
    const f = { shielded, fund, move };
    const off: Pools = { ...POOLS, shielded: { active: true, syncing: false, duffs: 125_000_000, spendsOff: true } };
    expect(routeWhy("shieldedToCore", off, f)).toBe(SHIELDED_SPENDS_COPY.unavailable);
    expect(routeWhy("shieldedToPlatform", off, f)).toBe(SHIELDED_SPENDS_COPY.unavailable);
    expect(routeWhy("coreToShielded", off, f)).toBe(SHIELDED_SPENDS_COPY.shieldIn);
    expect(routeWhy("coreToPlatform", off, f)).toBeNull();
    const on: Pools = { ...POOLS, shielded: { active: true, syncing: false, duffs: 125_000_000, spendsOff: false } };
    for (const r of ["coreToShielded", "shieldedToCore", "shieldedToPlatform"] as const) expect(routeWhy(r, on, f), r).toBe(routeWhy(r, POOLS, f));
  });

  it("each say why they can't start, with the text the Home card gave", () => {
    const { shielded, fund, move } = flows();
    const f = { shielded, fund, move };
    for (const r of ["coreToShielded", "coreToPlatform", "shieldedToCore", "shieldedToPlatform", "platformToCore"] as const) expect(routeWhy(r, POOLS, f), r).toBeNull();
    const inactive = { ...POOLS, shielded: { active: false, syncing: false, duffs: null } };
    expect(routeWhy("coreToShielded", inactive, f)).toBe(SHIELDED_COPY.card.inactive);
    const syncing = { ...POOLS, shielded: { active: true, syncing: true, duffs: null } };
    expect(routeWhy("coreToShielded", syncing, f)).toBe(SHIELDED_SYNC_COPY.syncing);
    expect(routeWhy("shieldedToCore", syncing, f)).toBe(SHIELDED_SYNC_COPY.syncing);
    // The vault's lock holding the account is named, for Add and Transfer out alike.
    const mixingOnly = { ...POOLS, shielded: { active: true, syncing: true, wait: "mixingOnly" as const, duffs: null } };
    for (const r of ["coreToShielded", "shieldedToCore", "shieldedToPlatform"] as const) expect(routeWhy(r, mixingOnly, f), r).toBe(SHIELDED_SYNC_COPY.wait("mixingOnly"));
    expect(routeWhy("shieldedToCore", { ...POOLS, shielded: { active: true, syncing: false, duffs: 0 } }, f)).toBe(SHIELDED_COPY.card.why.empty);
    expect(routeWhy("shieldedToPlatform", inactive, f)).toBe(SHIELDED_COPY.card.inactive);
    expect(routeWhy("shieldedToPlatform", syncing, f)).toBe(SHIELDED_SYNC_COPY.syncing);
    expect(routeWhy("shieldedToPlatform", { ...POOLS, shielded: { active: true, syncing: false, duffs: 0 } }, f)).toBe(SHIELDED_COPY.card.why.empty);
    expect(routeWhy("coreToPlatform", { ...POOLS, platform: null }, f)).toBe("Platform addresses are not active on this network yet.");
    const platform = POOLS.platform!;
    expect(routeWhy("platformToCore", { ...POOLS, platform: { ...platform, duffs: 0 } }, f)).toBe(TRANSFER_COPY.platformEmpty);
    expect(routeWhy("platformToCore", { ...POOLS, platform: { ...platform, duffs: 0, dust: true } }, f)).toBe("dust");
    expect(routeWhy("platformToCore", { ...POOLS, platform: { ...platform, block: "checking" } }, f)).toBe(MOVE_COPY.check.loading);
    expect(routeWhy("platformToCore", { ...POOLS, platform: { ...platform, block: "failed" } }, f)).toBe(MOVE_COPY.check.failed);
    expect(routeWhy("platformToCore", { ...POOLS, platform: { ...platform, block: "pending" } }, f)).toBe(MOVE_COPY.waiting.text);
  });

  it("hold only Add to Shielded while the wallet's Core sync runs: a shield's funding waits for it", () => {
    const { shielded, fund, move } = flows();
    const f = { shielded, fund, move };
    const coreSyncing = { ...POOLS, coreSyncing: true };
    expect(routeWhy("coreToShielded", coreSyncing, f)).toBe(SHIELDED_SYNC_COPY.syncing);
    for (const r of ["shieldedToCore", "coreToPlatform", "platformToCore"] as const) expect(routeWhy(r, coreSyncing, f), r).toBeNull();
    expect(routeWhy("coreToShielded", { ...coreSyncing, shielded: { active: false, syncing: false, duffs: null } }, f)).toBe(SHIELDED_COPY.card.inactive);
  });

  it("share the Shielded flow: another of its moves running holds the other route, not its own", () => {
    const f = flows();
    f.shielded.begin("out");
    f.shielded.running = true;
    expect(routeWhy("coreToShielded", POOLS, f)).toBe(SHIELDED_COPY.card.why.busy);
    expect(routeWhy("shieldedToPlatform", POOLS, f)).toBe(SHIELDED_COPY.card.why.busy);
    // Its own route opens at its step, even with nothing left in Shielded.
    expect(routeWhy("shieldedToCore", { ...POOLS, shielded: { active: true, syncing: false, duffs: 0 } }, f)).toBeNull();
  });
});

describe("the shortcut before the pools are read", () => {
  const LOADING: Pools = { core: null, shielded: undefined, platform: undefined };

  it("loading is not absent: no default route yet, and each route says it is reading", () => {
    const { t } = model();
    expect(defaultRoute(LOADING)).toBeUndefined();
    // Shielded inactive, Platform still read: Platform may yet be the default.
    expect(defaultRoute({ ...POOLS, shielded: { active: false, syncing: false, duffs: null }, platform: undefined })).toBeUndefined();
    for (const r of ["coreToShielded", "shieldedToCore", "coreToPlatform", "platformToCore"] as const) expect(routeWhy(r, LOADING, t.flows)).toBe(TRANSFER_COPY.loading);
  });

  it("opens waiting, then on the default route once the pools are read", () => {
    const { t, shielded } = model();
    t.start(defaultRoute(LOADING));
    expect([t.shown, t.awaitingDefault, t.begunRoute, shielded.open]).toEqual([true, true, null, false]);
    t.settleDefault(defaultRoute(LOADING));
    expect(t.awaitingDefault).toBe(true);
    t.settleDefault(defaultRoute(POOLS));
    expect([t.awaitingDefault, t.begunRoute, shielded.open, shielded.kind, shielded.step]).toEqual([false, "coreToShielded", true, "add", "form"]);
  });

  it("a sheet closed while waiting does not open when the pools arrive", () => {
    const { t, shielded } = model();
    t.start(undefined);
    t.close();
    t.settleDefault(defaultRoute(POOLS));
    expect([t.shown, shielded.open]).toEqual([false, false]);
  });
});

describe("the default route", () => {
  it("is Dash wallet → Shielded, or → Platform when Shielded is not active", () => {
    expect(defaultRoute(POOLS)).toBe("coreToShielded");
    expect(defaultRoute({ ...POOLS, shielded: { active: false, syncing: false, duffs: null } })).toBe("coreToPlatform");
    expect(defaultRoute({ ...POOLS, shielded: null })).toBe("coreToPlatform");
  });

  it("is → Platform where this build cannot spend from Shielded, and → Shielded, held, without Platform", () => {
    const off = { active: true, syncing: false, duffs: 125_000_000, spendsOff: true };
    expect(defaultRoute({ ...POOLS, shielded: off })).toBe("coreToPlatform");
    expect(defaultRoute({ ...POOLS, shielded: off, platform: null })).toBe("coreToShielded");
    expect(defaultRoute({ ...POOLS, shielded: off, platform: undefined })).toBeUndefined();
  });

  it("is none when neither is there, and the sheet says why", () => {
    expect(defaultRoute({ ...POOLS, shielded: { active: false, syncing: false, duffs: null }, platform: null })).toBeNull();
    expect(noRouteText({ ...POOLS, shielded: { active: false, syncing: false, duffs: null }, platform: null })).toBe(TRANSFER_COPY.none.inactive);
    expect(noRouteText({ ...POOLS, shielded: null, platform: null })).toBe(TRANSFER_COPY.none.absent);
  });
});

describe("the picks", () => {
  it("move the other end to its default when both would be the same pool, and swap only a route that reverses", () => {
    const { t } = model();
    t.start("coreToShielded");
    t.pick("from", "shielded", POOLS);
    expect([t.from, t.to]).toEqual(["shielded", "core"]);
    t.pick("to", "platform", POOLS);
    expect([t.from, t.to, t.route]).toEqual(["shielded", "platform", "shieldedToPlatform"]);
    // Platform → Shielded has no command: no swap.
    t.swap();
    expect([t.from, t.to]).toEqual(["shielded", "platform"]);
    t.pick("from", "core", POOLS);
    expect(t.route).toBe("coreToPlatform");
    t.swap();
    expect(t.route).toBe("platformToCore");
    t.pick("to", "platform", POOLS);
    expect([t.from, t.to]).toEqual(["core", "platform"]);
  });
});

describe("Max", () => {
  it.each(["coreToShielded", "coreToPlatform"] as const)("of %s is one Core Max quote ({kind: all}), filling the lock it quotes: no amount with the fee on top", async (route) => {
    const { t, quotes, shielded, fund } = model();
    const quote = route === "coreToShielded" ? quotes.shieldQuote : quotes.platformFundQuote;
    quote.mockResolvedValue(coreMaxQuote(199_998_956));
    t.start(route);
    const flow = route === "coreToShielded" ? shielded : fund;
    await t.max(route, 3_000_000_000, (d) => flow.setText(String(d)), String);
    expect(quote.mock.calls).toEqual([[WALLET_A, { kind: "all" }]]);
    expect(flow.text).toBe("199998956");
    expect(t.isMax(route)).toBe(true);
    expect(t.lines[route].state).toMatchObject({ kind: "quoted", amount: 199_998_956 });
  });

  it("from the Dash wallet: Continue sends the Core Max while the field holds what Max filled; an edit sends the amount", async () => {
    const { t, quotes, shielded, fund } = model();
    quotes.shieldQuote.mockResolvedValue(coreMaxQuote(199_998_956));
    quotes.platformFundQuote.mockResolvedValue(coreMaxQuote(199_998_956));
    t.start("coreToShielded");
    await t.max("coreToShielded", 3_000_000_000, (d) => shielded.setText(String(d)), String);
    expect(t.isMax("coreToShielded")).toBe(true);
    // The field typed over, even back to the same figure: an amount of its own from then on.
    t.edited("coreToShielded");
    shielded.setText("199998956");
    expect(t.isMax("coreToShielded")).toBe(false);
    t.start("coreToPlatform");
    await t.max("coreToPlatform", 3_000_000_000, (d) => fund.setText(String(d)), String);
    expect(t.isMax("coreToPlatform")).toBe(true);
    fund.setText("1.5");
    expect(t.isMax("coreToPlatform")).toBe(false);
  });

  it("from the Dash wallet: Max's figure stays a Core Max for the balance it read; another balance makes it an amount", async () => {
    const { timers, tick } = manualTimers();
    const shieldQuote = vi.fn(async (_w: string, a: FundAmount) => (a.kind === "all" ? coreMaxQuote(199_998_956) : ({ duffs: a.duffs } as never)));
    const f = flows();
    const t = new TransferModel(f, { shieldQuote, unshieldQuote: vi.fn(), unshieldToPlatformQuote: vi.fn(), platformFundQuote: vi.fn() }, WALLET_A, timers);
    t.start("coreToShielded");
    await t.max("coreToShielded", 3_000_000_000, (d) => f.shielded.setText(String(d)), String);
    // The sheet reads the figure back at the balance Max read: still the Core Max, nothing quoted again.
    t.typed("coreToShielded", 199_998_956, 3_000_000_000);
    tick(400);
    await flush();
    expect(t.isMax("coreToShielded")).toBe(true);
    expect(shieldQuote.mock.calls.map(([, a]) => a)).toEqual([{ kind: "all" }]);
    // The source balance changed: the figure is an amount now, quoted with its fee on top.
    t.typed("coreToShielded", 199_998_956, 2_900_000_000);
    tick(400);
    await flush();
    expect(t.isMax("coreToShielded")).toBe(false);
    expect(shieldQuote.mock.calls.at(-1)).toEqual([WALLET_A, { kind: "duffs", duffs: 199_998_956 }]);
  });

  it("from the Dash wallet: Max's text read as another amount (a unit change) is an amount, not the Core Max", async () => {
    const { t, quotes, shielded } = model();
    quotes.shieldQuote.mockResolvedValue(coreMaxQuote(199_998_956));
    t.start("coreToShielded");
    await t.max("coreToShielded", 3_000_000_000, (d) => shielded.setText(String(d)), String);
    expect(t.isMax("coreToShielded")).toBe(true);
    t.typed("coreToShielded", 19_999_895_600, 3_000_000_000);
    expect(t.isMax("coreToShielded")).toBe(false);
  });

  it("from the Dash wallet: a refused Core Max fills nothing and is marked whole, after one quote", async () => {
    const { t, quotes } = model();
    quotes.shieldQuote.mockRejectedValue(appError(SHIELDED_CODES.tooManyInputs, { kind: "number", value: 60 }));
    const fill = vi.fn();
    await t.max("coreToShielded", 3_000_000_000, fill);
    expect(fill).not.toHaveBeenCalled();
    expect(quotes.shieldQuote).toHaveBeenCalledTimes(1);
    expect(t.lines.coreToShielded.state).toMatchObject({ kind: "refused", whole: true, error: { code: SHIELDED_CODES.tooManyInputs } });
  });

  describe("from the Dash wallet: what a Core Max leaves is said before Confirm, with its amount, only when above 0", () => {
    const fmt = (d: number) => `${d} duffs`;
    const LEFT = "633 duffs stays in your Dash wallet, because Max doesn't include mixed coins, payments from contacts or coins in other accounts.";
    const ARRIVING = "5000 duffs still arriving stays in your Dash wallet. Move it once it confirms.";
    it.each([
      ["nothing left or arriving", { inputs: 2, leftDuffs: 0, arrivingDuffs: 0 }, []],
      ["coins in other accounts only", { inputs: 2, leftDuffs: 633, arrivingDuffs: 0 }, [LEFT]],
      ["coins still arriving only", { inputs: 2, leftDuffs: 0, arrivingDuffs: 5_000 }, [ARRIVING]],
      ["both", { inputs: 2, leftDuffs: 633, arrivingDuffs: 5_000 }, [LEFT, ARRIVING]],
    ] as const)("%s", (_name, max, notes) => {
      expect(coreMaxNotes(max, fmt)).toEqual(notes);
    });
    it("an amount's quote (no Core Max) says nothing", () => {
      expect(coreMaxNotes(null, fmt)).toEqual([]);
    });
  });

  it("of Shielded → Dash wallet is the balance less the fee, in whole duffs", () => {
    const quote = { credits: "125000000000", feeCredits: "20000500", totalCredits: "0", expectedDuffs: 0, grant: { maxDuffs: 0, maxCredits: "0" } } satisfies ShieldedSpendQuote;
    expect(shieldedSpendMax({ ok: true, quote }, 125_000_000)).toBe(124_979_999);
    const short = appError(SHIELDED_CODES.insufficientFunds, { kind: "credits", needed: "125020000000", available: "125000000000" });
    expect(shieldedSpendMax({ ok: false, error: short }, 125_000_000)).toBe(124_980_000);
    expect(shieldedSpendMax({ ok: false, error: appError(SHIELDED_CODES.poolTooSmall) }, 125_000_000)).toBeNull();
  });

  it.each([
    [2, 124_980_000, 2],
    [3, 124_970_000, 3],
    [5, 124_950_000, 3],
  ] as const)("of Shielded → Dash wallet over %i notes derives again from each refusal until a quote takes it", async (n, max, calls) => {
    const { t, quotes } = model();
    quotes.unshieldQuote.mockImplementation(async (_w: string, c: string) => engineSpendQuote(NOTE_SETS[n], c));
    let filled: number | null = null;
    await t.max("shieldedToCore", Number(NOTES_BALANCE / 1000n), (d) => (filled = d));
    expect(filled).toBe(max);
    expect(quotes.unshieldQuote).toHaveBeenCalledTimes(calls);
    expect(t.lines.shieldedToCore.state).toMatchObject({ kind: "quoted", amount: max, quote: { totalCredits: String(NOTES_BALANCE) } });
  });

  it.each([
    [2, 124_980_000, 2],
    [5, 124_950_000, 3],
  ] as const)("of Shielded → Platform over %i notes derives from unshield_to_platform_quote the same way", async (n, max, calls) => {
    const { t, quotes } = model();
    quotes.unshieldToPlatformQuote.mockImplementation(async (_w: string, c: string, to: string | null) => (expect(to).toBeNull(), engineSpendQuote(NOTE_SETS[n], c)));
    let filled: number | null = null;
    await t.max("shieldedToPlatform", Number(NOTES_BALANCE / 1000n), (d) => (filled = d));
    expect(filled).toBe(max);
    expect(quotes.unshieldToPlatformQuote).toHaveBeenCalledTimes(calls);
    expect(quotes.unshieldQuote).not.toHaveBeenCalled();
    expect(t.lines.shieldedToPlatform.state).toMatchObject({ kind: "quoted", amount: max });
  });

  it("re-quotes once and fills only from the quote that took it", async () => {
    const { timers } = manualTimers();
    const fetch = vi.fn(async (d: number) => {
      if (d > 2_899_999_000) throw appError("credits.funding_insufficient", { kind: "duffs", needed: d + 1_000, available: 2_900_000_000 });
      return { duffs: d };
    });
    const line = new QuoteLine(fetch, timers);
    const got = await line.max(3_000_000_000, fromShortfall);
    expect(got).toBe(2_899_999_000);
    expect(fetch.mock.calls.map(([d]) => d)).toEqual([3_000_000_000, 2_899_999_000]);
    expect(line.state).toEqual({ kind: "quoted", amount: 2_899_999_000, quote: { duffs: 2_899_999_000 } });
    // The field then reads that amount: it is already quoted, so it is not read again.
    line.set(2_899_999_000);
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it("keeps the previous quote visible while Max is outstanding", async () => {
    const next = deferred<{ duffs: number }>();
    const { timers, tick } = manualTimers();
    const fetch = vi.fn().mockResolvedValueOnce({ duffs: 5 }).mockReturnValueOnce(next.promise);
    const line = new QuoteLine(fetch, timers);
    line.set(5);
    tick(400);
    await flush();
    const previous = line.state;
    const max = line.max(100, () => 100);
    expect(line.maxing).toBe(true);
    expect(line.state).toEqual(previous);
    next.resolve({ duffs: 100 });
    expect(await max).toBe(100);
  });

  it("keeps Platform Max's quote when the filled amount is parsed", async () => {
    const { t, quotes } = model();
    quotes.platformFundQuote.mockResolvedValue({ credits: "999000", duffs: 1000 });
    await t.max("coreToPlatform", 1000, () => undefined);
    t.typed("coreToPlatform", 1000, 1000);
    expect(t.lines.coreToPlatform.state.kind).toBe("quoted");
    t.typed("coreToPlatform", 900, 1000);
    expect(t.lines.coreToPlatform.state.kind).toBe("idle");
  });

  it("clears Platform Max's quote when spendable funds change", async () => {
    const { t, quotes } = model();
    quotes.platformFundQuote.mockResolvedValue({ credits: "999000", duffs: 1000 });
    await t.max("coreToPlatform", 1000, () => undefined);
    t.typed("coreToPlatform", 1000, 900);
    expect(t.lines.coreToPlatform.state.kind).toBe("idle");
    expect(quotes.platformFundQuote).toHaveBeenCalledTimes(1);
  });

  it("shows the first quote's refusal when it names no shortfall (a pending lock), with no re-quote", async () => {
    const { timers } = manualTimers();
    const fetch = vi.fn(async () => {
      throw appError(SHIELDED_CODES.shieldPending, { kind: "txid", txid: "t" });
    });
    const line = new QuoteLine(fetch, timers);
    expect(await line.max(3_000_000_000, fromShortfall)).toBeNull();
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(line.state).toMatchObject({ kind: "refused", error: { code: SHIELDED_CODES.shieldPending } });
  });

  it("is dropped when the amount is typed while it works", async () => {
    const { timers } = manualTimers();
    const first = deferred<{ duffs: number }>();
    const fetch = vi.fn((d: number) => (d === 3_000_000_000 ? first.promise : Promise.resolve({ duffs: d })));
    const line = new QuoteLine(fetch, timers);
    const max = line.max(3_000_000_000, () => 1_000);
    await flush();
    line.set(5_000);
    first.resolve({ duffs: 3_000_000_000 });
    expect(await max).toBeNull();
    expect(fetch).toHaveBeenCalledTimes(1);
  });
});

describe("the fee line", () => {
  it("reads the quote once the amount has stayed the same for 400 ms", async () => {
    const { timers, tick } = manualTimers();
    const fetch = vi.fn(async (d: number) => ({ duffs: d }));
    const line = new QuoteLine(fetch, timers);
    line.set(1);
    tick(300);
    line.set(12);
    tick(300);
    line.set(123);
    expect(line.state).toEqual({ kind: "quoting" });
    tick(399);
    expect(fetch).not.toHaveBeenCalled();
    tick(1);
    expect(fetch.mock.calls).toEqual([[123]]);
    await flush();
    expect(line.state).toEqual({ kind: "quoted", amount: 123, quote: { duffs: 123 } });
  });

  it("has one quote out at a time: a newer amount waits for it and wins, the older answer is dropped", async () => {
    const { timers, tick } = manualTimers();
    const answers = new Map<number, ReturnType<typeof deferred<{ duffs: number }>>>();
    const fetch = vi.fn((d: number) => {
      const a = deferred<{ duffs: number }>();
      answers.set(d, a);
      return a.promise;
    });
    const line = new QuoteLine(fetch, timers);
    line.set(1);
    tick(400);
    line.set(2);
    tick(400);
    line.set(3);
    tick(400);
    // Only the first is out; 2 was replaced before it could go.
    expect(fetch.mock.calls).toEqual([[1]]);
    answers.get(1)!.resolve({ duffs: 1 });
    await flush();
    expect(line.state).toEqual({ kind: "quoting" });
    expect(fetch.mock.calls).toEqual([[1], [3]]);
    answers.get(3)!.reject(appError("credits.funding_insufficient", { kind: "duffs", needed: 4, available: 2 }));
    await flush();
    expect(line.state).toMatchObject({ kind: "refused", amount: 3 });
  });

  it("quotes nothing while the route can't run, and drops an answer that comes after", async () => {
    const { timers, tick, pending } = manualTimers();
    const a = deferred<{ duffs: number }>();
    const fetch = vi.fn(() => a.promise);
    const line = new QuoteLine(fetch, timers);
    line.set(5);
    tick(400);
    line.set(null);
    expect(pending()).toBe(0);
    a.resolve({ duffs: 5 });
    await flush();
    expect(line.state).toEqual({ kind: "idle" });
    expect(fetch).toHaveBeenCalledTimes(1);
  });
});

describe("a refusal never outlives its inputs", () => {
  it("the same amount is read again when the source balance changes", async () => {
    const { timers, tick } = manualTimers();
    let balance = 100_000;
    const shieldQuote = vi.fn(async (_wallet: string, a: FundAmount) => {
      const d = duffsOf(a);
      if (d > balance) throw appError(SHIELDED_CODES.fundingInsufficient, { kind: "duffs", needed: d + 1_000, available: balance });
      return { duffs: d } as never;
    });
    const t = new TransferModel(flows(), { shieldQuote, unshieldQuote: vi.fn(), unshieldToPlatformQuote: vi.fn(), platformFundQuote: vi.fn() }, WALLET_A, timers);
    t.typed("coreToShielded", 500_000, balance);
    tick(400);
    await flush();
    expect(t.lines.coreToShielded.state).toMatchObject({ kind: "refused", amount: 500_000 });
    // Same balance: nothing read again.
    t.typed("coreToShielded", 500_000, balance);
    tick(400);
    await flush();
    expect(shieldQuote).toHaveBeenCalledTimes(1);
    balance = 3_000_000;
    t.typed("coreToShielded", 500_000, balance);
    expect(t.lines.coreToShielded.state).toEqual({ kind: "quoting" });
    tick(400);
    await flush();
    expect(shieldQuote).toHaveBeenCalledTimes(2);
    expect(t.lines.coreToShielded.state).toMatchObject({ kind: "quoted", amount: 500_000 });
  });

  it("on Dash wallet → Platform a balance change clears the refusal Max left, with no quote", async () => {
    const { timers } = manualTimers();
    const platformFundQuote = vi.fn(async (): Promise<never> => {
      throw appError("credits.fund_pending", { kind: "txid", txid: "t" });
    });
    const t = new TransferModel(flows(), { shieldQuote: vi.fn(), unshieldQuote: vi.fn(), unshieldToPlatformQuote: vi.fn(), platformFundQuote }, WALLET_A, timers);
    t.typed("coreToPlatform", null, 3_000_000_000);
    await t.max("coreToPlatform", 3_000_000_000, vi.fn());
    expect(t.lines.coreToPlatform.state).toMatchObject({ kind: "refused" });
    t.typed("coreToPlatform", null, 2_000_000_000);
    expect(t.lines.coreToPlatform.state).toEqual({ kind: "idle" });
    expect(platformFundQuote).toHaveBeenCalledTimes(1);
  });
});

describe("Dash wallet → Platform quotes only on Continue", () => {
  /** A model whose Platform quote answers a Core Max with a lock of 2_899_998_956 and any amount it covers. */
  function fundModel() {
    const { timers, tick } = manualTimers();
    const platformFundQuote = vi.fn(async (_wallet: string, a: FundAmount) => {
      if (a.kind === "all") return coreMaxQuote(2_899_998_956);
      if (a.duffs > 2_899_999_000) throw appError("credits.funding_insufficient", { kind: "duffs", needed: a.duffs + 1_000, available: 2_900_000_000 });
      return { duffs: a.duffs } as never;
    });
    const shieldQuote = vi.fn(async (_wallet: string, a: FundAmount) => ({ duffs: duffsOf(a) }) as never);
    const t = new TransferModel(flows(), { shieldQuote, unshieldQuote: vi.fn(), unshieldToPlatformQuote: vi.fn(), platformFundQuote }, WALLET_A, timers);
    return { t, tick, platformFundQuote, shieldQuote };
  }

  it("typing an amount makes no platform_fund_quote call; the other routes still quote it", async () => {
    const { t, tick, platformFundQuote, shieldQuote } = fundModel();
    t.typed("coreToPlatform", 5);
    t.typed("coreToPlatform", 50_000_000);
    tick(2_000);
    await flush();
    expect(platformFundQuote).not.toHaveBeenCalled();
    expect(t.lines.coreToPlatform.state).toEqual({ kind: "idle" });
    t.typed("coreToShielded", 100_000);
    tick(400);
    await flush();
    expect(shieldQuote.mock.calls).toEqual([[WALLET_A, { kind: "duffs", duffs: 100_000 }]]);
  });

  it("Max is a click, not typing: one Core Max call, and typing after it quotes nothing", async () => {
    const { t, tick, platformFundQuote } = fundModel();
    const fill = vi.fn();
    await t.max("coreToPlatform", 3_000_000_000, fill);
    expect(platformFundQuote.mock.calls).toEqual([[WALLET_A, { kind: "all" }]]);
    expect(fill).toHaveBeenCalledWith(2_899_998_956);
    t.typed("coreToPlatform", 2_899_998_956);
    t.typed("coreToPlatform", 1_000_000);
    tick(2_000);
    await flush();
    expect(platformFundQuote).toHaveBeenCalledTimes(1);
    expect(t.lines.coreToPlatform.state).toEqual({ kind: "idle" });
  });

  it("each Max click makes exactly one platform_fund_quote call, the Core Max", async () => {
    const { t, platformFundQuote } = fundModel();
    const fill = vi.fn();
    await t.max("coreToPlatform", 3_000_000_000, fill);
    await t.max("coreToPlatform", 3_000_000_000, fill);
    expect(platformFundQuote.mock.calls.map(([, a]) => a)).toEqual([{ kind: "all" }, { kind: "all" }]);
    expect(fill.mock.calls).toEqual([[2_899_998_956], [2_899_998_956]]);
  });

  it("a refused Core Max leaves the amount field unchanged and shows its refusal", async () => {
    const { timers } = manualTimers();
    const platformFundQuote = vi.fn(async (): Promise<never> => {
      throw appError("credits.too_many_inputs", { kind: "number", value: 60 });
    });
    const f = flows();
    const t = new TransferModel(f, { shieldQuote: vi.fn(), unshieldQuote: vi.fn(), unshieldToPlatformQuote: vi.fn(), platformFundQuote }, WALLET_A, timers);
    t.start("coreToPlatform");
    // The field as the sheet binds it: the fund flow's text, which Max fills.
    f.fund.setText("0.3");
    t.typed("coreToPlatform", 30_000_000);
    await t.max("coreToPlatform", 3_000_000_000, (duffs) => f.fund.setText(String(duffs / 1e8)));
    expect(platformFundQuote).toHaveBeenCalledTimes(1);
    expect(f.fund.text).toBe("0.3");
    expect(t.lines.coreToPlatform.state).toMatchObject({ kind: "refused", whole: true, error: { code: "credits.too_many_inputs" } });
  });

  it("a refusal Max showed stays until the amount is typed, which clears it without a quote", async () => {
    const { timers, tick } = manualTimers();
    const platformFundQuote = vi.fn(async (): Promise<never> => {
      throw appError("credits.fund_pending", { kind: "txid", txid: "t" });
    });
    const t = new TransferModel(flows(), { shieldQuote: vi.fn(), unshieldQuote: vi.fn(), unshieldToPlatformQuote: vi.fn(), platformFundQuote }, WALLET_A, timers);
    await t.max("coreToPlatform", 3_000_000_000, vi.fn());
    expect(t.lines.coreToPlatform.state).toMatchObject({ kind: "refused", error: { code: "credits.fund_pending" } });
    t.typed("coreToPlatform", 1_000_000);
    tick(2_000);
    await flush();
    expect(t.lines.coreToPlatform.state).toEqual({ kind: "idle" });
    expect(platformFundQuote).toHaveBeenCalledTimes(1);
  });
});

describe("the sheet over the routes' flows", () => {
  it("opens a route's flow at a fresh form, and closes it with the sheet", () => {
    const { t, shielded, fund } = model();
    t.start("coreToShielded");
    expect([t.shown, shielded.open, shielded.kind, shielded.step, t.stepFlow()]).toEqual([true, true, "add", "form", null]);
    shielded.setText("0.5");
    t.pick("to", "platform", POOLS);
    expect([shielded.open, fund.open]).toEqual([false, true]);
    t.close();
    expect([t.shown, fund.open]).toEqual([false, false]);
  });

  it("closes with its route's own Close, and shows a route whose flow opened itself (a card's Finish)", async () => {
    const { t, shielded } = model();
    t.start("coreToShielded");
    shielded.next({ kind: "add", duffs: 1 });
    expect(t.stepFlow()).toBe("coreToShielded");
    shielded.close();
    expect(t.shown).toBe(false);
    const finishing = shielded.resume("lock1");
    expect([t.shown, t.open, t.route, t.stepFlow()]).toEqual([true, false, "coreToShielded", "coreToShielded"]);
    await finishing;
    expect(shielded.step).toBe("done");
    shielded.close();
    expect(t.shown).toBe(false);
  });

  it("opens Shielded → Platform on the shared Shielded flow, and closes it when the wallet is on mainnet", () => {
    const { t, shielded } = model();
    t.start("shieldedToPlatform");
    expect([t.route, shielded.open, shielded.kind, t.begunRoute, t.stepFlow()]).toEqual(["shieldedToPlatform", true, "toPlatform", "shieldedToPlatform", null]);
    shielded.next({ kind: "toPlatform", credits: "1000" });
    expect(t.stepFlow()).toBe("shieldedToPlatform");
    shielded.back();
    t.setMainnet(true);
    expect([t.route, shielded.open, t.begunRoute]).toEqual(["unsupported", false, null]);
    t.pick("from", "core", POOLS);
    expect([t.from, t.to, t.route]).toEqual(["core", "platform", "unsupported"]);
  });

  it("a mainnet wallet never begins Shielded → Platform", () => {
    const { t, shielded } = model();
    t.setMainnet(true);
    expect(t.shown).toBe(false);
    t.start("shieldedToCore");
    t.pick("to", "platform", POOLS);
    expect([t.route, shielded.open, t.begunRoute]).toEqual(["unsupported", false, null]);
  });

  it("leaves the shared Shielded flow alone while another of its moves runs", () => {
    const { t, shielded } = model();
    shielded.begin("out");
    shielded.running = true;
    shielded.close();
    t.start("coreToShielded");
    expect([shielded.open, shielded.kind, t.begunRoute]).toEqual([false, "out", null]);
    t.pick("from", "shielded", POOLS);
    expect([t.route, shielded.open, t.begunRoute]).toEqual(["shieldedToCore", true, "shieldedToCore"]);
  });

  it("offers another route without interrupting a running transfer", () => {
    const { t, shielded, move } = model();
    t.start("coreToShielded");
    shielded.running = true;
    shielded.step = "running";
    t.chooseRoute();
    expect([t.shown, t.picking, t.stepFlow(), shielded.running, shielded.open]).toEqual([true, "from", null, true, false]);
    t.pick("from", "platform", POOLS);
    t.pick("to", "core", POOLS);
    expect([t.route, move.open, shielded.running]).toEqual(["platformToCore", true, true]);
  });

  it("shows Platform → Dash wallet's own confirmation only after Continue", () => {
    const { t, move } = model();
    t.start("platformToCore");
    expect([move.open, move.step, t.stepFlow()]).toEqual([true, "confirm", null]);
    t.review();
    expect(t.stepFlow()).toBe("platformToCore");
    t.pick("to", "shielded", POOLS);
    expect([t.reviewed, move.open]).toEqual([false, false]);
  });
});

describe("test gaps (review r1 §8)", () => {
  it("a move Platform has not confirmed: Platform → Dash wallet is not begun again, and the route says why", () => {
    const { t, move } = model();
    Object.assign(move, { step: "done", outcome: { state: "pending" } });
    expect(move.pending).toBe(true);
    t.start("platformToCore");
    expect([t.begunRoute, move.open, t.route]).toEqual([null, false, "platformToCore"]);
    expect(routeWhy("platformToCore", POOLS, t.flows)).toBe(POOLS.platform!.waitingText);
  });

  /** A line whose every quote is answered by hand, in call order. */
  function heldLine() {
    const { timers, tick } = manualTimers();
    const calls: number[] = [];
    const answers: ReturnType<typeof deferred<{ duffs: number }>>[] = [];
    const line = new QuoteLine((d: number) => {
      calls.push(d);
      const a = deferred<{ duffs: number }>();
      answers.push(a);
      return a.promise;
    }, timers);
    return { line, tick, calls, answers };
  }
  const short = (needed: number, available: number) => appError("credits.funding_insufficient", { kind: "duffs", needed, available });

  it("Max stops at whichever answer comes after the amount changed: the quote it waited for, its probe, or its re-quote", async () => {
    const derive = fromShortfall;

    // 1. Typed while Max waits for a quote already out: no probe.
    {
      const { line, tick, calls, answers } = heldLine();
      line.set(5);
      tick(400);
      const d = vi.fn(derive);
      const max = line.max(100, d);
      line.set(6);
      answers[0].resolve({ duffs: 5 });
      await flush();
      expect(calls).toEqual([5]);
      for (const a of answers.slice(1)) a.reject(short(110, 100));
      expect(await max).toBeNull();
      expect(d).not.toHaveBeenCalled();
    }
    // 2. Typed while the probe is out: no re-quote.
    {
      const { line, calls, answers } = heldLine();
      const d = vi.fn(derive);
      const max = line.max(100, d);
      await flush();
      line.set(6);
      answers[0].reject(short(110, 100));
      await flush();
      expect(calls).toEqual([100]);
      for (const a of answers.slice(1)) a.resolve({ duffs: 90 });
      expect(await max).toBeNull();
      expect(d).not.toHaveBeenCalled();
    }
    // 3. Typed while the re-quote is out: its answer neither fills nor shows.
    {
      const { line, calls, answers } = heldLine();
      const max = line.max(100, derive);
      await flush();
      answers[0].reject(short(110, 100));
      await flush();
      expect(calls).toEqual([100, 90]);
      line.set(6);
      answers[1].resolve({ duffs: 90 });
      expect(await max).toBeNull();
      expect(line.state).toEqual({ kind: "quoting" });
      expect(line.maxing).toBe(false);
    }
  });

  it("a debounced answer that arrives while Max runs leaves Max's line alone", async () => {
    const { line, tick, calls, answers } = heldLine();
    line.set(5);
    tick(400);
    const max = line.max(100, fromShortfall);
    answers[0].resolve({ duffs: 5 });
    await flush();
    expect([line.state, line.maxing]).toEqual([{ kind: "quoting" }, true]);
    expect(calls).toEqual([5, 100]);
    answers[1].reject(short(110, 100));
    await flush();
    answers[2].resolve({ duffs: 90 });
    expect(await max).toBe(90);
    expect(line.state).toEqual({ kind: "quoted", amount: 90, quote: { duffs: 90 } });
  });

  it("the fee line's figures, exactly, per route", () => {
    const fmt = (d: number) => `${d} duffs`;
    const quoted = <Q,>(quote: Q) => ({ kind: "quoted" as const, amount: 1, quote });
    // Dash wallet → Shielded: the lock's fee plus the pool's, in credits; you receive the credits.
    expect(feeText("coreToShielded", quoted({ feeDuffs: 1_000, poolFeeCredits: "500000", credits: "99500000" } as ShieldQuote), fmt)).toBe("Fee about 1500 duffs · You receive about 99500 duffs");
    // Dash wallet → Platform: the lock's fee plus Platform's.
    expect(feeText("coreToPlatform", quoted({ feeDuffs: 1_000, feeCredits: "2000000", credits: "48000000" } as PlatformFundQuote), fmt)).toBe("Fee about 3000 duffs · You receive about 48000 duffs");
    // Shielded → Dash wallet: what arrives in the Dash wallet when known, else the credits.
    expect(feeText("shieldedToCore", quoted({ feeCredits: "3000000", expectedDuffs: 49_000, credits: "52000000" } as ShieldedSpendQuote), fmt)).toBe("Fee about 3000 duffs · You receive about 49000 duffs");
    expect(feeText("shieldedToCore", quoted({ feeCredits: "3000000", expectedDuffs: null, credits: "52000000" } as ShieldedSpendQuote), fmt)).toBe("Fee about 3000 duffs · You receive about 52000 duffs");
    expect(feeText("shieldedToPlatform", quoted({ feeCredits: "3000000", expectedDuffs: null, credits: "52000000" } as ShieldedSpendQuote), fmt)).toBe("Fee about 3000 duffs · You receive about 52000 duffs");
    expect(feeText("coreToShielded", { kind: "quoting" }, fmt)).toBe(TRANSFER_COPY.quoting);
    expect(feeText("coreToShielded", { kind: "idle" }, fmt)).toBeNull();
    expect(feeText("coreToShielded", { kind: "refused", amount: 1, error: null }, fmt)).toBeNull();
  });
});
