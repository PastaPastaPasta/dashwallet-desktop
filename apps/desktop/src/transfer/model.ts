// Transfer (POLISH 25): one sheet for the moves between the wallet's own balances. It is a view over
// the routes' own per-wallet flows (shielded/flow.ts, platform/fund.ts, platform/move.ts): each route
// keeps its command, quote, grant, confirm step, progress, outcome and recovery. This model holds
// what the sheet adds: the From and To picks, the fee line read while typing, and Max.

import { isAppError, type Backend } from "../api/backend";
import type { CoreMax } from "../api/generated/CoreMax";
import type { FundAmount } from "../api/generated/FundAmount";
import type { PlatformFundQuote } from "../api/generated/PlatformFundQuote";
import type { ShieldedSpendQuote } from "../api/generated/ShieldedSpendQuote";
import type { ShieldQuote } from "../api/generated/ShieldQuote";
import { CREDITS_PER_DUFF, SHIELDED_CODES } from "../api/pending";
import { messageTable, t } from "../i18n";
import { Model } from "../onboarding/model";
import { fundFor, type PlatformFundApi, type PlatformFundFlow } from "../platform/fund";
import { MOVE_COPY, moveFor, type MoveBlock, type PlatformMoveApi, type PlatformMoveFlow } from "../platform/move";
import { SHIELDED_COPY, shieldedFor, type ShieldedApi, type ShieldedFlow, type ShieldedKind } from "../shielded/flow";
import { SHIELDED_SYNC_COPY } from "../shielded/syncing";

export type Pool = "core" | "shielded" | "platform";

/** The routes with a desktop command. */
export type RouteId = "coreToShielded" | "coreToPlatform" | "shieldedToCore" | "shieldedToPlatform" | "platformToCore";

export const ROUTES: Record<RouteId, { from: Pool; to: Pool }> = {
  coreToShielded: { from: "core", to: "shielded" },
  coreToPlatform: { from: "core", to: "platform" },
  shieldedToCore: { from: "shielded", to: "core" },
  shieldedToPlatform: { from: "shielded", to: "platform" },
  platformToCore: { from: "platform", to: "core" },
};

/** The routes the engine keeps off on mainnet (`platform.feature_off`), each by its flag. */
export const OFF_ON_MAINNET: ReadonlySet<RouteId> = new Set<RouteId>(["shieldedToPlatform"]);

/** The route from `from` to `to`; `"unsupported"` where no desktop command moves it (Platform →
 * Shielded) or the route is off on this network; `null` for the same pool. */
export function routeOf(from: Pool, to: Pool, mainnet = false): RouteId | "unsupported" | null {
  if (from === to) return null;
  const id = (Object.keys(ROUTES) as RouteId[]).find((r) => ROUTES[r].from === from && ROUTES[r].to === to);
  return id && !(mainnet && OFF_ON_MAINNET.has(id)) ? id : "unsupported";
}

/** Why the ends make no route: off on this network, or no command for it. */
export const unsupportedText = (from: Pool, to: Pool): string => (from === "shielded" && to === "platform" ? TRANSFER_COPY.offOnNetwork : TRANSFER_COPY.unsupported);

export const TRANSFER_COPY = {
  get title() {
    return t("transfer.title");
  },
  get shortcut() {
    return t("transfer.shortcut");
  },
  get from() {
    return t("transfer.from");
  },
  get to() {
    return t("transfer.to");
  },
  pick: {
    get from() {
      return t("transfer.pick.from");
    },
    get to() {
      return t("transfer.pick.to");
    },
  },
  get swap() {
    return t("transfer.swap");
  },
  pools: messageTable("transfer.pools", ["core", "shielded", "platform"]) satisfies Record<Pool, string>,
  /** After a pool card's title, it names the button that opens Transfer from that pool. */
  get unknownBalance() {
    return t("transfer.unknownBalance");
  },
  get max() {
    return t("transfer.max");
  },
  get wholeBalance() {
    return t("transfer.wholeBalance");
  },
  /** Platform → Dash wallet's form; its confirmation names the address (MOVE_COPY.message). */
  get platformToCoreIntro() {
    return t("transfer.platformToCoreIntro");
  },
  fee: (fee: string, receive: string) => t("transfer.fee", { fee, receive }),
  get quoting() {
    return t("transfer.quoting");
  },
  get feeOnNextStep() {
    return t("transfer.feeOnNextStep");
  },
  get invalidAmount() {
    return t("transfer.invalidAmount");
  },
  why: {
    get amount() {
      return t("transfer.why.amount");
    },
    get max() {
      return t("transfer.why.max");
    },
  },
  get continue() {
    return t("transfer.continue");
  },
  get cancel() {
    return t("common.cancel");
  },
  get unsupported() {
    return t("transfer.unsupported");
  },
  get offOnNetwork() {
    return t("transfer.offOnNetwork");
  },
  none: {
    get absent() {
      return t("transfer.none.absent");
    },
    get inactive() {
      return t("transfer.none.inactive");
    },
  },
  get platformEmpty() {
    return t("transfer.platformEmpty");
  },
  get platformInactive() {
    return t("transfer.platformInactive");
  },
  get noCoreBalance() {
    return t("transfer.noCoreBalance");
  },
  get loading() {
    return t("transfer.loading");
  },
  running: (what: string) => t("transfer.running", { what }),
  get show() {
    return t("transfer.show");
  },
  routeNames: {
    get coreToShielded() {
      return SHIELDED_COPY.titles.add;
    },
    get coreToPlatform() {
      return t("transfer.routeNames.coreToPlatform");
    },
    get shieldedToCore() {
      return t("transfer.routeNames.shieldedToCore");
    },
    get shieldedToPlatform() {
      return t("transfer.routeNames.shieldedToPlatform");
    },
    get platformToCore() {
      return t("transfer.routeNames.platformToCore");
    },
    get send() {
      return SHIELDED_COPY.titles.send;
    },
  },
};

/** What the sheet knows of the balances, from the reads the Home cards use. */
export interface Pools {
  /** The Dash wallet's balance in duffs; `null` while it is not read. */
  core: number | null;
  mixing?: number;
  /** `null`: this dw-app has no Shielded commands; `undefined`: still being read. `duffs` is `null`
   * while it is no reading. */
  shielded: { active: boolean; syncing: boolean; duffs: number | null } | null | undefined;
  /** `null`: this dw-app has no Platform addresses; `undefined`: still being read. `duffs`: what a
   * move of everything would take. */
  platform: { duffs: number | null; dust: boolean; dustText: string; block: MoveBlock | null; waitingText: string } | null | undefined;
  /** The wallet's Core sync is running and not caught up: a shield's funding waits for it. */
  coreSyncing?: boolean;
}

/** The pools the picker lists: the Dash wallet always, the others while this dw-app has them. */
export function listedPools(pools: Pools): Pool[] {
  return ["core", ...(pools.shielded ? (["shielded"] as const) : []), ...(pools.platform ? (["platform"] as const) : [])];
}

/** The route the shortcut opens: to Shielded, or to Platform when Shielded is not active. `null`:
 * neither is there, and `noRouteText` says why; `undefined`: not known until the pools are read. */
export function defaultRoute(pools: Pools): RouteId | null | undefined {
  if (pools.shielded?.active) return "coreToShielded";
  if (pools.shielded === undefined) return undefined;
  if (pools.platform) return "coreToPlatform";
  if (pools.platform === undefined) return undefined;
  return null;
}

export function noRouteText(pools: Pools): string {
  return pools.shielded ? TRANSFER_COPY.none.inactive : TRANSFER_COPY.none.absent;
}

/** The other end after a pick made both ends `pool`: the Dash wallet, or from it the default pool. */
export function defaultEnd(pool: Pool, pools: Pools): Pool {
  if (pool !== "core") return "core";
  return pools.shielded?.active || !pools.platform ? "shielded" : "platform";
}

/** Whether the ends can swap: the reverse is a route with a command. */
export const canSwap = (from: Pool, to: Pool, mainnet = false): boolean => {
  const r = routeOf(to, from, mainnet);
  return r !== null && r !== "unsupported";
};

/** The routes the Shielded flow runs. */
export type ShieldedRouteId = "coreToShielded" | "shieldedToCore" | "shieldedToPlatform";

const SHIELDED_KIND: Record<ShieldedRouteId, ShieldedKind> = { coreToShielded: "add", shieldedToCore: "out", shieldedToPlatform: "toPlatform" };

export const isShieldedRoute = (route: RouteId): route is ShieldedRouteId => route in SHIELDED_KIND;

/** The route a Shielded flow kind belongs to (a lock's Finish belongs to the add); `"send"` for a
 * payment to another Shielded address. */
export const shieldedRoute = (kind: ShieldedKind): ShieldedRouteId | "send" => (kind === "send" ? "send" : kind === "out" ? "shieldedToCore" : kind === "toPlatform" ? "shieldedToPlatform" : "coreToShielded");

export interface Flows {
  shielded: ShieldedFlow;
  fund: PlatformFundFlow;
  move: PlatformMoveFlow;
}

/** The route's flow is running or holds a result not seen yet: the sheet opens at its step. */
export function routeActive(route: RouteId, flows: Flows): boolean {
  switch (route) {
    case "coreToShielded":
    case "shieldedToCore":
    case "shieldedToPlatform": {
      const f = flows.shielded;
      return (f.running || f.unseen) && shieldedRoute(f.kind) === route;
    }
    case "coreToPlatform":
      return flows.fund.running || flows.fund.unseen;
    case "platformToCore":
      return flows.move.running || flows.move.unseen;
  }
}

/** Why `route` can't start now (the reason the Home card's button gave), or `null`. A route that is
 * running or has an unseen result is never held back: the sheet shows its step. */
export function routeWhy(route: RouteId, pools: Pools, flows: Flows): string | null {
  if (routeActive(route, flows)) return null;
  switch (route) {
    case "coreToShielded":
    case "shieldedToCore":
    case "shieldedToPlatform": {
      const s = pools.shielded;
      if (s === undefined) return TRANSFER_COPY.loading;
      if (!s?.active) return SHIELDED_COPY.card.inactive;
      if (s.syncing || (route === "coreToShielded" && pools.coreSyncing)) return SHIELDED_SYNC_COPY.syncing;
      const f = flows.shielded;
      // The flow is shared: another of its moves is running, or its result waits to be seen.
      if (f.running) return SHIELDED_COPY.card.why.busy;
      if (f.unseen) return SHIELDED_COPY.unseen;
      if (route !== "coreToShielded" && !s.duffs) return SHIELDED_COPY.card.why.empty;
      return null;
    }
    case "coreToPlatform":
      return pools.platform ? null : pools.platform === undefined ? TRANSFER_COPY.loading : TRANSFER_COPY.platformInactive;
    case "platformToCore": {
      const p = pools.platform;
      if (p === undefined) return TRANSFER_COPY.loading;
      if (!p) return TRANSFER_COPY.platformInactive;
      if (flows.move.pending || p.block === "pending") return p.waitingText;
      if (p.block === "checking") return MOVE_COPY.check.loading;
      if (p.block === "failed") return MOVE_COPY.check.failed;
      if (!p.duffs) return p.dust ? p.dustText : TRANSFER_COPY.platformEmpty;
      return null;
    }
  }
}

/** A quote's answer or refusal. */
export type Settled<Q> = { ok: true; quote: Q } | { ok: false; error: unknown };

const settle = <Q>(p: Promise<Q>): Promise<Settled<Q>> =>
  p.then(
    (quote) => ({ ok: true as const, quote }),
    (error: unknown) => ({ ok: false as const, error }),
  );

/** `whole`: the refusal of a quote of the whole source (a Core Max). */
export type LineState<Q> = { kind: "idle" } | { kind: "quoting" } | { kind: "quoted"; amount: number; quote: Q } | { kind: "refused"; amount: number; error: unknown; whole?: boolean };

export interface Timers {
  set(f: () => void, ms: number): unknown;
  clear(t: unknown): void;
}

const WINDOW_TIMERS: Timers = { set: (f, ms) => setTimeout(f, ms), clear: (t) => clearTimeout(t as ReturnType<typeof setTimeout>) };

/** How long the amount stays the same before the fee line reads its quote. */
export const QUOTE_DEBOUNCE_MS = 400;

/** Routes whose quote is read only on Continue (and Max), never while typing: `platform_fund_quote`
 * may derive a receive address under the wallet-manager write lock (platform_fund.rs:319). */
export const QUOTE_ON_CONTINUE: ReadonlySet<RouteId> = new Set<RouteId>(["coreToPlatform"]);

/** The fee line of one route: the route's own quote of the amount typed, read once the amount has
 * stayed the same for `QUOTE_DEBOUNCE_MS`, one quote at a time. A newer amount wins: an older answer is
 * dropped, and the newest amount is read once the call out has answered. */
export class QuoteLine<Q> extends Model {
  state: LineState<Q> = { kind: "idle" };
  /** Max is working. */
  maxing = false;
  private target: number | null = null;
  private due = false;
  private inflight: Promise<unknown> | null = null;
  private timer: unknown = null;
  /** What the quote also depends on (the source balance): a change re-reads the same amount. */
  private basis: unknown = undefined;
  /** Max's answer is dropped once the amount changes under it. */
  private maxGen = 0;

  constructor(
    private readonly fetch: (duffs: number) => Promise<Q>,
    private readonly timers: Timers = WINDOW_TIMERS,
    private readonly delayMs = QUOTE_DEBOUNCE_MS,
  ) {
    super();
  }

  /** The amount to quote, in duffs; `null`: nothing to quote (no amount, or the route can't run).
   * `basis` is what else the quote depends on: when it changes, the same amount is read again, so a
   * refusal never outlives the balance it was given for. */
  set(amount: number | null, basis?: unknown) {
    // Nothing to quote still clears what Max left (its refusal names no amount typed).
    if (amount === this.target && basis === this.basis && !this.maxing && (amount !== null || this.state.kind === "idle")) return;
    this.basis = basis;
    this.maxGen++;
    this.maxing = false;
    this.target = amount;
    this.due = false;
    this.timers.clear(this.timer);
    this.timer = null;
    if (amount === null) this.state = { kind: "idle" };
    else {
      this.state = { kind: "quoting" };
      this.timer = this.timers.set(() => {
        this.timer = null;
        this.due = true;
        this.pump();
      }, this.delayMs);
    }
    this.changed();
  }

  quotedAt(amount: number | null, basis: unknown): boolean {
    return this.state.kind === "quoted" && this.state.amount === amount && this.basis === basis;
  }

  private pump() {
    if (this.inflight || !this.due || this.target === null) return;
    const amount = this.target;
    this.due = false;
    this.inflight = this.run(amount).then((r) => {
      this.inflight = null;
      if (amount === this.target && !this.maxing) {
        this.state = r.ok ? { kind: "quoted", amount, quote: r.quote } : { kind: "refused", amount, error: r.error };
        this.changed();
      }
      this.pump();
    });
  }

  private run(duffs: number): Promise<Settled<Q>> {
    return settle(this.fetch(duffs));
  }

  /** Max (one quote of `probe`, then one of what `derive` makes of its answer, and while a refusal
   * derives a smaller amount, one of that, up to `quotes` in all): the amount the last quote took, or
   * `null` when a quote refused (the line shows that refusal) or the amount changed meanwhile. `derive`
   * returns `probe` itself when the first quote already took it. */
  async max(probe: number, derive: (answer: Settled<Q>, amount: number) => number | null, quotes = 2): Promise<number | null> {
    return this.maxing ? null : this.maxRun(probe, async (live) => {
      const first = await this.exclusive(() => this.run(probe));
      if (!live()) return null;
      const derived = derive(first, probe);
      if (derived === null) {
        this.state = first.ok ? { kind: "quoted", amount: probe, quote: first.quote } : { kind: "refused", amount: probe, error: first.error };
        return null;
      }
      // A probe that took the whole balance is already a quote of that amount (Confirm quotes again).
      let amount = derived;
      let last = amount === probe && first.ok ? first : await this.exclusive(() => this.run(amount));
      for (let n = 2; !last.ok; n++) {
        if (!live()) return null;
        const next = n < quotes ? derive(last, amount) : null;
        if (next === null || next >= amount) {
          this.state = { kind: "refused", amount, error: last.error };
          return null;
        }
        amount = next;
        last = await this.exclusive(() => this.run(amount));
      }
      if (!live()) return null;
      this.target = amount;
      this.state = { kind: "quoted", amount, quote: last.quote };
      return amount;
    });
  }

  /** Max as one quote of the whole source (`whole`, the engine's Core Max, the fee out of it): the
   * amount it moves (`amountOf`), or `null` when it refused (the line shows that refusal) or the amount
   * changed meanwhile. */
  async maxWhole(probe: number, whole: () => Promise<Q>, amountOf: (quote: Q) => number): Promise<number | null> {
    return this.maxing ? null : this.maxRun(probe, async (live) => {
      const got = await this.exclusive(() => settle(whole()));
      if (!live()) return null;
      if (!got.ok) {
        this.state = { kind: "refused", amount: probe, error: got.error, whole: true };
        return null;
      }
      const amount = amountOf(got.quote);
      this.target = amount;
      this.state = { kind: "quoted", amount, quote: got.quote };
      return amount;
    });
  }

  /** Runs one Max: the quotes after any out, its answer dropped once the amount changes under it. */
  private async maxRun(probe: number, body: (live: () => boolean) => Promise<number | null>): Promise<number | null> {
    this.basis = probe;
    const gen = ++this.maxGen;
    this.timers.clear(this.timer);
    this.timer = null;
    this.due = false;
    this.target = null;
    this.maxing = true;
    this.changed();
    const live = () => gen === this.maxGen;
    try {
      while (this.inflight) await this.inflight;
      if (!live()) return null;
      return await body(live);
    } finally {
      if (live()) {
        this.maxing = false;
        this.changed();
      }
    }
  }

  /** One quote while nothing else is out. */
  private async exclusive(quote: () => Promise<Settled<Q>>): Promise<Settled<Q>> {
    const p = quote();
    this.inflight = p;
    try {
      return await p;
    } finally {
      if (this.inflight === p) this.inflight = null;
      // An amount typed while Max was out is read now.
      this.pump();
    }
  }

  dispose() {
    this.timers.clear(this.timer);
    this.timer = null;
    this.maxGen++;
    this.target = null;
  }
}

/** Max of a spend from Shielded: the amount less the quoted fee, or from `insufficient_funds`'s
 * figures; in whole duffs. The engine's refusal names the fee of the notes it had selected when it
 * stopped (at first the two-action minimum), so a refusal of the amount this derives can name a
 * larger fee: derive again from it (`SHIELDED_MAX_QUOTES`). */
export function shieldedSpendMax(first: Settled<ShieldedSpendQuote>, probe: number): number | null {
  const probeCredits = BigInt(probe) * CREDITS_PER_DUFF;
  let most: bigint;
  if (first.ok) most = probeCredits - BigInt(first.quote.feeCredits);
  else {
    const e = first.error;
    if (!isAppError(e) || e.code !== SHIELDED_CODES.insufficientFunds || e.data?.kind !== "credits") return null;
    most = BigInt(e.data.available) - (BigInt(e.data.needed) - probeCredits);
  }
  const duffs = most / CREDITS_PER_DUFF;
  return duffs > 0n ? Number(duffs) : null;
}

/** Quotes a Shielded Max may take: the fee a refusal names only grows, up to that of every note. */
export const SHIELDED_MAX_QUOTES = 4;

export const MAX_DERIVE = {
  shieldedToCore: shieldedSpendMax,
  shieldedToPlatform: shieldedSpendMax,
} as const;

/** The routes funded from the Dash wallet: their Max is the engine's Core Max (`{kind: "all"}`), one
 * lock of every coin in the wallet's account 0 with the fee out of it, so no change stays behind. */
export type CoreRouteId = "coreToShielded" | "coreToPlatform";

export const isCoreRoute = (route: RouteId): route is CoreRouteId => route === "coreToShielded" || route === "coreToPlatform";

/** What a Core Max leaves in the Dash wallet, each with its amount: the other accounts' coins, and the
 * coins still arriving. */
export function coreMaxNotes(max: CoreMax | null | undefined, fmt: (d: number) => string): string[] {
  const notes: string[] = [];
  if (max && max.leftDuffs > 0) notes.push(t("transfer.maxLeft", { left: fmt(max.leftDuffs) }));
  if (max && max.arrivingDuffs > 0) notes.push(t("transfer.maxArriving", { arriving: fmt(max.arrivingDuffs) }));
  return notes;
}

/** What the fee line says for a quote of the amount (`null`: nothing yet; a refusal says its own). */
export function feeText(route: Exclude<RouteId, "platformToCore">, state: LineState<unknown>, fmt: (d: number) => string): string | null {
  if (state.kind === "idle" || state.kind === "refused") return null;
  if (state.kind === "quoting") return TRANSFER_COPY.quoting;
  const credits = (c: string | bigint) => fmt(Number(BigInt(c) / CREDITS_PER_DUFF));
  switch (route) {
    case "coreToShielded": {
      const q = state.quote as ShieldQuote;
      return TRANSFER_COPY.fee(credits(BigInt(q.feeDuffs) * CREDITS_PER_DUFF + BigInt(q.poolFeeCredits)), credits(q.credits));
    }
    case "coreToPlatform": {
      const q = state.quote as PlatformFundQuote;
      return TRANSFER_COPY.fee(credits(BigInt(q.feeDuffs) * CREDITS_PER_DUFF + BigInt(q.feeCredits)), credits(q.credits));
    }
    case "shieldedToCore":
    case "shieldedToPlatform": {
      const q = state.quote as ShieldedSpendQuote;
      return TRANSFER_COPY.fee(credits(q.feeCredits), q.expectedDuffs === null ? credits(q.credits) : fmt(q.expectedDuffs));
    }
  }
}

export type TransferApi = ShieldedApi & PlatformFundApi & PlatformMoveApi & Pick<Backend, "shieldQuote" | "unshieldQuote" | "unshieldToPlatformQuote" | "platformFundQuote">;

export interface Lines {
  coreToShielded: QuoteLine<ShieldQuote>;
  coreToPlatform: QuoteLine<PlatformFundQuote>;
  shieldedToCore: QuoteLine<ShieldedSpendQuote>;
  shieldedToPlatform: QuoteLine<ShieldedSpendQuote>;
}

/** Where the sheet is. The routes' flows keep their own steps; this keeps the picks. */
export class TransferModel extends Model {
  /** The sheet was opened by the user (the shortcut, a card, a Show line). */
  open = false;
  from: Pool = "core";
  to: Pool = "shielded";
  picking: "from" | "to" | null = null;
  /** The end of the last pick, counted (`n`): the sheet returns focus there after each one. */
  picked: { end: "from" | "to"; n: number } | null = null;
  /** Platform → Dash wallet has no form of its own: Continue shows its confirmation. */
  reviewed = false;
  /** Opened by the shortcut before the pools were read: the default route is picked once they are. */
  awaitingDefault = false;
  /** The wallet is on mainnet: the routes in `OFF_ON_MAINNET` are not offered. */
  mainnet = false;
  /** The route whose flow the sheet opened. */
  private begun: RouteId | null = null;
  /** The text Max last filled on each route: discreet mode masks it while the field still holds it. */
  readonly maxText: Partial<Record<Exclude<RouteId, "platformToCore">, string>> = {};
  readonly lines: Lines;

  constructor(
    readonly flows: Flows,
    api: Pick<Backend, "shieldQuote" | "unshieldQuote" | "unshieldToPlatformQuote" | "platformFundQuote">,
    walletId: string,
    timers: Timers = WINDOW_TIMERS,
  ) {
    super();
    this.lines = {
      coreToShielded: new QuoteLine((d) => api.shieldQuote(walletId, this.amountOf("coreToShielded", d)), timers),
      coreToPlatform: new QuoteLine((d) => api.platformFundQuote(walletId, this.amountOf("coreToPlatform", d)), timers),
      shieldedToCore: new QuoteLine((d) => api.unshieldQuote(walletId, String(BigInt(d) * CREDITS_PER_DUFF), null), timers),
      shieldedToPlatform: new QuoteLine((d) => api.unshieldToPlatformQuote(walletId, String(BigInt(d) * CREDITS_PER_DUFF), null), timers),
    };
    this.wholeQuote = { coreToShielded: () => api.shieldQuote(walletId, { kind: "all" }), coreToPlatform: () => api.platformFundQuote(walletId, { kind: "all" }) };
    for (const f of [flows.shielded, flows.fund, flows.move]) f.subscribe(() => this.onFlow());
  }

  private readonly wholeQuote: Record<CoreRouteId, () => Promise<ShieldQuote | PlatformFundQuote>>;
  /** What each Core Max filled, in duffs, and the source balance it read. */
  private readonly maxAt: Partial<Record<CoreRouteId, { duffs: number; basis: number }>> = {};

  /** The field of `route` still holds the amount its Max filled: Continue sends a Core Max. */
  isMax(route: CoreRouteId): boolean {
    const filled = this.maxText[route];
    return filled !== undefined && filled === this.flowOf(route).text;
  }

  /** The amount typed on `route`: what Max filled stays a Core Max until it is edited. */
  private amountOf(route: CoreRouteId, duffs: number): FundAmount {
    return this.isMax(route) ? { kind: "all" } : { kind: "duffs", duffs };
  }

  /** The amount on `route` was edited: it is no longer Max's. */
  edited(route: Exclude<RouteId, "platformToCore">) {
    delete this.maxText[route];
    if (isCoreRoute(route)) delete this.maxAt[route];
  }

  /** The route whose flow the sheet opened (its form edits that flow's amount). */
  get begunRoute(): RouteId | null {
    return this.begun;
  }

  /** The route the ends make: a route, `"unsupported"` (no command, or off on this network) or `null`. */
  get route(): RouteId | "unsupported" | null {
    return routeOf(this.from, this.to, this.mainnet);
  }

  /** The wallet's network: on mainnet a route that is off there is unsupported, and its flow closes. */
  setMainnet(mainnet: boolean) {
    if (mainnet === this.mainnet) return;
    this.mainnet = mainnet;
    if (this.open) this.select(this.from, this.to);
    else this.changed();
  }

  /** Whether the sheet shows: opened here, or a route's flow opened itself (a Finish on a card). */
  get shown(): boolean {
    return this.open || this.flowRoute() !== null;
  }

  /** The route of the flow that is open now, if a transfer route's. */
  flowRoute(): RouteId | null {
    const { shielded, fund, move } = this.flows;
    const s = shieldedRoute(shielded.kind);
    if (shielded.open && s !== "send") return s;
    if (fund.open) return "coreToPlatform";
    if (move.open) return "platformToCore";
    return null;
  }

  /** The flow of `route`, open at its step; `null` while the sheet shows the form. */
  stepFlow(): RouteId | null {
    const r = this.flowRoute();
    if (r === null) return null;
    switch (r) {
      case "coreToShielded":
      case "shieldedToCore":
      case "shieldedToPlatform":
        // A lock's Finish waits for its grant at "form" with no amount to edit.
        return this.flows.shielded.step === "form" && this.flows.shielded.kind !== "resume" ? null : r;
      case "coreToPlatform":
        return this.flows.fund.step === "form" ? null : r;
      case "platformToCore":
        return this.flows.move.step === "confirm" && !this.reviewed ? null : r;
    }
  }

  /** Opens the sheet at `route` (`null`: where it was, or nowhere when nothing can be moved;
   * `undefined`: the default route is not known yet, `settleDefault` picks it). */
  start(route: RouteId | null | undefined) {
    this.open = true;
    this.picking = null;
    this.awaitingDefault = route === undefined;
    if (route) this.select(ROUTES[route].from, ROUTES[route].to);
    else this.changed();
  }

  /** The pools were read: a sheet waiting for its default route opens on it. */
  settleDefault(route: RouteId | null | undefined) {
    if (!this.awaitingDefault || route === undefined) return;
    this.start(route);
  }

  togglePicker(end: "from" | "to") {
    this.picking = this.picking === end ? null : end;
    this.changed();
  }

  /** A pick: if it makes both ends the same pool, the other end moves to its default. */
  pick(end: "from" | "to", pool: Pool, pools: Pools) {
    this.picking = null;
    this.picked = { end, n: (this.picked?.n ?? 0) + 1 };
    if (end === "from") this.select(pool, this.to === pool ? defaultEnd(pool, pools) : this.to);
    else this.select(this.from === pool ? defaultEnd(pool, pools) : this.from, pool);
  }

  swap() {
    if (canSwap(this.from, this.to, this.mainnet)) this.select(this.to, this.from);
  }

  /** Platform → Dash wallet: Continue shows the move's own confirmation. */
  review() {
    this.reviewed = true;
    this.changed();
  }

  /** Hides the sheet; a running call goes on, and its result waits on its card. */
  close() {
    this.open = false;
    this.picking = null;
    this.awaitingDefault = false;
    this.reviewed = false;
    const r = this.begun ?? this.flowRoute();
    this.begun = null;
    if (r) this.flowOf(r).close();
    this.changed();
  }

  chooseRoute() {
    this.close();
    this.open = true;
    this.picking = "from";
    this.changed();
  }

  private select(from: Pool, to: Pool) {
    this.awaitingDefault = false;
    const before = this.begun;
    this.begun = null;
    this.from = from;
    this.to = to;
    this.reviewed = false;
    const route = this.route;
    if (before && before === route) this.begun = before;
    else {
      if (before) this.flowOf(before).close();
      if (route && route !== "unsupported") this.beginRoute(route);
    }
    this.changed();
  }

  /** Opens the route's flow at a fresh form or at its running or unseen step. The Shielded flow is
   * shared: while another of its moves runs or waits to be seen, it is left alone (the route says why). */
  private beginRoute(route: RouteId) {
    const s = this.flows.shielded;
    switch (route) {
      case "coreToShielded":
      case "shieldedToCore":
      case "shieldedToPlatform":
        if ((s.running || s.unseen) && shieldedRoute(s.kind) !== route) return;
        this.begun = route;
        s.begin(route === "coreToShielded" ? (s.running || s.unseen ? s.kind : "add") : SHIELDED_KIND[route]);
        return;
      case "coreToPlatform":
        this.begun = route;
        this.flows.fund.begin();
        return;
      case "platformToCore":
        // A move Platform has not confirmed is not offered again: the route says so, as the card does.
        if (this.flows.move.pending) return;
        this.begun = route;
        this.flows.move.begin();
        return;
    }
  }

  /** The flow `route` runs (the Shielded routes share one). */
  flowOf(route: ShieldedRouteId): ShieldedFlow;
  flowOf(route: "coreToPlatform"): PlatformFundFlow;
  flowOf(route: "platformToCore"): PlatformMoveFlow;
  flowOf(route: Exclude<RouteId, "platformToCore">): ShieldedFlow | PlatformFundFlow;
  flowOf(route: RouteId): ShieldedFlow | PlatformFundFlow | PlatformMoveFlow;
  flowOf(route: RouteId): ShieldedFlow | PlatformFundFlow | PlatformMoveFlow {
    return isShieldedRoute(route) ? this.flows.shielded : route === "coreToPlatform" ? this.flows.fund : this.flows.move;
  }

  /** The fee line of `route`. */
  lineOf(route: Exclude<RouteId, "platformToCore">): QuoteLine<unknown> {
    return this.lines[route] as QuoteLine<unknown>;
  }

  /** A route's flow closed itself (its Close): the sheet closes with it. A flow that opened itself
   * (a card's Finish) is shown at its route. */
  private onFlow() {
    const open = this.flowRoute();
    if (this.begun && open !== this.begun && !this.flowIsOpen(this.begun)) {
      this.begun = null;
      if (this.open) {
        this.open = false;
        this.picking = null;
        this.reviewed = false;
      }
    }
    if (open && !this.begun) {
      this.begun = open;
      this.from = ROUTES[open].from;
      this.to = ROUTES[open].to;
      this.picking = null;
    }
    this.changed();
  }

  private flowIsOpen(route: RouteId): boolean {
    const f = this.flowOf(route);
    return f.open && (f !== this.flows.shielded || shieldedRoute(this.flows.shielded.kind) === route);
  }

  /** The amount typed on `route` (`null`: none, or the route can't run), with the source pool's
   * balance (`basis`): the fee line quotes it, except on a route quoted only on Continue, where
   * typing only clears what Max left. */
  typed(route: Exclude<RouteId, "platformToCore">, amount: number | null, basis?: number | null) {
    const line = this.lineOf(route);
    // A Core Max is the field's figure only for the balance it read: another amount or another
    // balance makes the figure an amount of its own.
    const at = isCoreRoute(route) ? this.maxAt[route] : undefined;
    if (at && amount !== null && (amount !== at.duffs || basis !== at.basis)) this.edited(route);
    if (QUOTE_ON_CONTINUE.has(route) && line.quotedAt(amount, basis)) return;
    line.set(QUOTE_ON_CONTINUE.has(route) ? null : amount, basis);
  }

  /** Max on `route`: `probe` is the pool's whole balance in duffs. The field is filled (`fill`, the
   * amount in duffs) only from the last quote that took it; `text` is how the field shows that amount. */
  async max(route: Exclude<RouteId, "platformToCore">, probe: number, fill: (duffs: number) => void, text?: (duffs: number) => string): Promise<void> {
    const line = this.lineOf(route);
    const got = isCoreRoute(route) ? await line.maxWhole(probe, this.wholeQuote[route], (q) => (q as ShieldQuote | PlatformFundQuote).duffs) : await line.max(probe, MAX_DERIVE[route] as (answer: Settled<unknown>, amount: number) => number | null, SHIELDED_MAX_QUOTES);
    if (got === null) return;
    if (text) this.maxText[route] = text(got);
    if (isCoreRoute(route)) this.maxAt[route] = { duffs: got, basis: probe };
    fill(got);
  }
}

const models = new Map<string, TransferModel>();

/** The wallet's transfer sheet, over its routes' flows; kept while the app runs. */
export function transferFor(api: TransferApi, walletId: string): TransferModel {
  let m = models.get(walletId);
  if (!m) {
    m = new TransferModel({ shielded: shieldedFor(api, walletId), fund: fundFor(api, walletId), move: moveFor(api, walletId) }, api, walletId);
    models.set(walletId, m);
  }
  return m;
}

/** Tests: forget every model (a fresh app). */
export function resetTransfers() {
  for (const m of models.values()) for (const l of Object.values(m.lines)) (l as QuoteLine<unknown>).dispose();
  models.clear();
}
