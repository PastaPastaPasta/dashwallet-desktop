// Transfer's model: which routes exist and when each can start, the route the shortcut opens, the
// picks, the fee line's debounce and one-quote-at-a-time, Max (one re-quote, filled only from a quote
// that took it), and the sheet following its routes' flows.

import { afterEach, describe, expect, it, vi } from "vitest";
import type { ShieldedSpendQuote } from "../api/generated/ShieldedSpendQuote";
import type { ShieldOutcome } from "../api/generated/ShieldOutcome";
import { SHIELDED_CODES } from "../api/pending";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { PlatformFundFlow, resetPlatformFunds } from "../platform/fund";
import { MOVE_COPY, PlatformMoveFlow, resetPlatformMoves } from "../platform/move";
import { resetShielded, SHIELDED_COPY, ShieldedFlow } from "../shielded/flow";
import { SHIELDED_SYNCING_TEXT } from "../shielded/syncing";
import { fakeVault } from "../tools/testApi";
import { coreFundedMax, defaultRoute, listedPools, noRouteText, QuoteLine, routeOf, routeWhy, shieldedSpendMax, TRANSFER_COPY, TransferModel, type Pools, type Settled, type Timers } from "./model";

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
    platformFund: vi.fn(),
    resumePlatformFund: vi.fn(),
    platformMoveToCore: vi.fn(),
  };
  return { shielded: new ShieldedFlow(api, WALLET_A), fund: new PlatformFundFlow(api, WALLET_A), move: new PlatformMoveFlow(api, WALLET_A) };
}

function model() {
  const f = flows();
  const quotes = { shieldQuote: vi.fn(), unshieldQuote: vi.fn(), platformFundQuote: vi.fn() };
  return { t: new TransferModel(f, quotes, WALLET_A, manualTimers().timers), ...f, quotes };
}

afterEach(() => {
  resetShielded();
  resetPlatformFunds();
  resetPlatformMoves();
});

describe("routes", () => {
  it("are the four with a command; Shielded ↔ Platform has none", () => {
    expect(routeOf("core", "shielded")).toBe("coreToShielded");
    expect(routeOf("core", "platform")).toBe("coreToPlatform");
    expect(routeOf("shielded", "core")).toBe("shieldedToCore");
    expect(routeOf("platform", "core")).toBe("platformToCore");
    expect(routeOf("shielded", "platform")).toBe("unsupported");
    expect(routeOf("platform", "shielded")).toBe("unsupported");
    expect(routeOf("core", "core")).toBeNull();
  });

  it("list the pools this dw-app has; Mixed coins is not one", () => {
    expect(listedPools(POOLS)).toEqual(["core", "shielded", "platform"]);
    expect(listedPools({ ...POOLS, shielded: null })).toEqual(["core", "platform"]);
    expect(listedPools({ ...POOLS, shielded: null, platform: null })).toEqual(["core"]);
  });

  it("each say why they can't start, with the text the Home card gave", () => {
    const { shielded, fund, move } = flows();
    const f = { shielded, fund, move };
    for (const r of ["coreToShielded", "coreToPlatform", "shieldedToCore", "platformToCore"] as const) expect(routeWhy(r, POOLS, f), r).toBeNull();
    const inactive = { ...POOLS, shielded: { active: false, syncing: false, duffs: null } };
    expect(routeWhy("coreToShielded", inactive, f)).toBe(SHIELDED_COPY.card.inactive);
    const syncing = { ...POOLS, shielded: { active: true, syncing: true, duffs: null } };
    expect(routeWhy("coreToShielded", syncing, f)).toBe(SHIELDED_SYNCING_TEXT);
    expect(routeWhy("shieldedToCore", syncing, f)).toBe(SHIELDED_SYNCING_TEXT);
    expect(routeWhy("shieldedToCore", { ...POOLS, shielded: { active: true, syncing: false, duffs: 0 } }, f)).toBe(SHIELDED_COPY.card.why.empty);
    expect(routeWhy("coreToPlatform", { ...POOLS, platform: null }, f)).toBe("Platform addresses are not active on this network yet.");
    const platform = POOLS.platform!;
    expect(routeWhy("platformToCore", { ...POOLS, platform: { ...platform, duffs: 0 } }, f)).toBe(TRANSFER_COPY.platformEmpty);
    expect(routeWhy("platformToCore", { ...POOLS, platform: { ...platform, duffs: 0, dust: true } }, f)).toBe("dust");
    expect(routeWhy("platformToCore", { ...POOLS, platform: { ...platform, block: "checking" } }, f)).toBe(MOVE_COPY.check.loading);
    expect(routeWhy("platformToCore", { ...POOLS, platform: { ...platform, block: "failed" } }, f)).toBe(MOVE_COPY.check.failed);
    expect(routeWhy("platformToCore", { ...POOLS, platform: { ...platform, block: "pending" } }, f)).toBe(MOVE_COPY.waiting.text);
  });

  it("share the Shielded flow: another of its moves running holds the other route, not its own", () => {
    const f = flows();
    f.shielded.begin("out");
    f.shielded.running = true;
    expect(routeWhy("coreToShielded", POOLS, f)).toBe(SHIELDED_COPY.card.why.busy);
    // Its own route opens at its step, even with nothing left in Shielded.
    expect(routeWhy("shieldedToCore", { ...POOLS, shielded: { active: true, syncing: false, duffs: 0 } }, f)).toBeNull();
  });
});

describe("the default route", () => {
  it("is Dash wallet → Shielded, or → Platform when Shielded is not active", () => {
    expect(defaultRoute(POOLS)).toBe("coreToShielded");
    expect(defaultRoute({ ...POOLS, shielded: { active: false, syncing: false, duffs: null } })).toBe("coreToPlatform");
    expect(defaultRoute({ ...POOLS, shielded: null })).toBe("coreToPlatform");
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
    expect([t.from, t.to, t.route]).toEqual(["shielded", "platform", "unsupported"]);
    // Not a dead end: the picker still picks.
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
  const fundingShort = (needed: number, available: number): Settled<unknown> => ({ ok: false, error: appError("credits.funding_insufficient", { kind: "duffs", needed, available }) });

  it("of a Core-funded route is available less the fee, from the refusal's own figures", () => {
    const derive = coreFundedMax("credits.funding_insufficient");
    expect(derive(fundingShort(3_000_001_000, 2_900_000_000), 3_000_000_000)).toBe(2_899_999_000);
    // A quote that took the whole balance already: that amount.
    expect(derive({ ok: true, quote: {} }, 1_000)).toBe(1_000);
    // Any other refusal, or nothing left after the fee: no Max.
    expect(derive({ ok: false, error: appError("credits.fund_pending", { kind: "txid", txid: "t" }) }, 1_000)).toBeNull();
    expect(derive(fundingShort(2_000, 900), 1_000)).toBeNull();
    expect(derive({ ok: false, error: appError("credits.funding_insufficient", { kind: "duffs", needed: null, available: 5 }) }, 1_000)).toBeNull();
  });

  it("of Shielded → Dash wallet is the balance less the fee, in whole duffs", () => {
    const quote = { credits: "125000000000", feeCredits: "20000500", totalCredits: "0", expectedDuffs: 0, grant: { maxDuffs: 0, maxCredits: "0" } } satisfies ShieldedSpendQuote;
    expect(shieldedSpendMax({ ok: true, quote }, 125_000_000)).toBe(124_979_999);
    const short = appError(SHIELDED_CODES.insufficientFunds, { kind: "credits", needed: "125020000000", available: "125000000000" });
    expect(shieldedSpendMax({ ok: false, error: short }, 125_000_000)).toBe(124_980_000);
    expect(shieldedSpendMax({ ok: false, error: appError(SHIELDED_CODES.poolTooSmall) }, 125_000_000)).toBeNull();
  });

  it("re-quotes once and fills only from the quote that took it", async () => {
    const { timers } = manualTimers();
    const fetch = vi.fn(async (d: number) => {
      if (d > 2_899_999_000) throw appError("credits.funding_insufficient", { kind: "duffs", needed: d + 1_000, available: 2_900_000_000 });
      return { duffs: d };
    });
    const line = new QuoteLine(fetch, timers);
    const got = await line.max(3_000_000_000, coreFundedMax("credits.funding_insufficient"));
    expect(got).toBe(2_899_999_000);
    expect(fetch.mock.calls.map(([d]) => d)).toEqual([3_000_000_000, 2_899_999_000]);
    expect(line.state).toEqual({ kind: "quoted", amount: 2_899_999_000, quote: { duffs: 2_899_999_000 } });
    // The field then reads that amount: it is already quoted, so it is not read again.
    line.set(2_899_999_000);
    expect(fetch).toHaveBeenCalledTimes(2);
  });

  it("shows the re-quote's refusal (too many coins) and fills nothing, with no third quote", async () => {
    const { timers } = manualTimers();
    const fetch = vi.fn(async (_wallet: string, d: number): Promise<never> => {
      if (d === 600_000_000) throw appError("credits.funding_insufficient", { kind: "duffs", needed: d + 1_000, available: 600_000_000 });
      throw appError("credits.too_many_inputs", { kind: "number", value: 60 });
    });
    const t = new TransferModel(flows(), { shieldQuote: vi.fn(), unshieldQuote: vi.fn(), platformFundQuote: fetch }, WALLET_A, timers);
    const fill = vi.fn();
    await t.max("coreToPlatform", 600_000_000, fill);
    expect(fill).not.toHaveBeenCalled();
    expect(fetch).toHaveBeenCalledTimes(2);
    expect(t.lines.coreToPlatform.state).toMatchObject({ kind: "refused", amount: 599_999_000, error: { code: "credits.too_many_inputs" } });
  });

  it("shows the first quote's refusal when it names no shortfall (a pending lock), with no re-quote", async () => {
    const { timers } = manualTimers();
    const fetch = vi.fn(async () => {
      throw appError(SHIELDED_CODES.shieldPending, { kind: "txid", txid: "t" });
    });
    const line = new QuoteLine(fetch, timers);
    expect(await line.max(3_000_000_000, coreFundedMax(SHIELDED_CODES.fundingInsufficient))).toBeNull();
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

describe("Dash wallet → Platform quotes only on Continue", () => {
  /** A model whose Platform quote refuses the whole balance with its shortfall and takes less. */
  function fundModel() {
    const { timers, tick } = manualTimers();
    const platformFundQuote = vi.fn(async (_wallet: string, d: number) => {
      if (d > 2_899_999_000) throw appError("credits.funding_insufficient", { kind: "duffs", needed: d + 1_000, available: 2_900_000_000 });
      return { duffs: d } as never;
    });
    const shieldQuote = vi.fn(async (_wallet: string, d: number) => ({ duffs: d }) as never);
    const t = new TransferModel(flows(), { shieldQuote, unshieldQuote: vi.fn(), platformFundQuote }, WALLET_A, timers);
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
    expect(shieldQuote.mock.calls).toEqual([[WALLET_A, 100_000]]);
  });

  it("Max is a click, not typing: two calls (the whole balance, then the most it leaves room for), and typing after it quotes nothing", async () => {
    const { t, tick, platformFundQuote } = fundModel();
    const fill = vi.fn();
    await t.max("coreToPlatform", 3_000_000_000, fill);
    expect(platformFundQuote.mock.calls.map(([, d]) => d)).toEqual([3_000_000_000, 2_899_999_000]);
    expect(fill).toHaveBeenCalledWith(2_899_999_000);
    t.typed("coreToPlatform", 2_899_999_000);
    t.typed("coreToPlatform", 1_000_000);
    tick(2_000);
    await flush();
    expect(platformFundQuote).toHaveBeenCalledTimes(2);
    expect(t.lines.coreToPlatform.state).toEqual({ kind: "idle" });
  });

  it("a refusal Max showed stays until the amount is typed, which clears it without a quote", async () => {
    const { timers, tick } = manualTimers();
    const platformFundQuote = vi.fn(async (): Promise<never> => {
      throw appError("credits.fund_pending", { kind: "txid", txid: "t" });
    });
    const t = new TransferModel(flows(), { shieldQuote: vi.fn(), unshieldQuote: vi.fn(), platformFundQuote }, WALLET_A, timers);
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
