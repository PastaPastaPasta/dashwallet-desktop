// Transfer (POLISH 25): one sheet for the moves between the wallet's own balances. It is a view over
// the routes' own per-wallet flows (shielded/flow.ts, platform/fund.ts, platform/move.ts): each route
// keeps its command, quote, grant, confirm step, progress, outcome and recovery. This model holds
// what the sheet adds: the From and To picks, the fee line read while typing, and Max.

import { isAppError, type Backend } from "../api/backend";
import type { PlatformFundQuote } from "../api/generated/PlatformFundQuote";
import type { ShieldedSpendQuote } from "../api/generated/ShieldedSpendQuote";
import type { ShieldQuote } from "../api/generated/ShieldQuote";
import { CREDITS_PER_DUFF, SHIELDED_CODES } from "../api/pending";
import { Model } from "../onboarding/model";
import { fundFor, type PlatformFundApi, type PlatformFundFlow } from "../platform/fund";
import { MOVE_COPY, moveFor, type MoveBlock, type PlatformMoveApi, type PlatformMoveFlow } from "../platform/move";
import { SHIELDED_COPY, shieldedFor, type ShieldedApi, type ShieldedFlow, type ShieldedKind } from "../shielded/flow";
import { SHIELDED_SYNCING_TEXT } from "../shielded/syncing";

export type Pool = "core" | "shielded" | "platform";

/** The routes with a desktop command. */
export type RouteId = "coreToShielded" | "coreToPlatform" | "shieldedToCore" | "platformToCore";

export const ROUTES: Record<RouteId, { from: Pool; to: Pool }> = {
  coreToShielded: { from: "core", to: "shielded" },
  coreToPlatform: { from: "core", to: "platform" },
  shieldedToCore: { from: "shielded", to: "core" },
  platformToCore: { from: "platform", to: "core" },
};

/** The route from `from` to `to`; `"unsupported"` between Shielded and Platform, which has no desktop
 * command; `null` for the same pool. */
export function routeOf(from: Pool, to: Pool): RouteId | "unsupported" | null {
  if (from === to) return null;
  const id = (Object.keys(ROUTES) as RouteId[]).find((r) => ROUTES[r].from === from && ROUTES[r].to === to);
  return id ?? "unsupported";
}

export const TRANSFER_COPY = {
  title: "Transfer",
  shortcut: "Transfer",
  from: "From",
  to: "To",
  pick: { from: "Transfer from", to: "Transfer to" },
  swap: "Swap From and To",
  pools: { core: "Dash wallet", shielded: "Shielded", platform: "Platform" } satisfies Record<Pool, string>,
  /** After a pool card's title, it names the button that opens Transfer from that pool. */
  cardButton: ", transfer",
  unknownBalance: "—",
  amount: "Amount",
  max: "Max",
  wholeBalance: "Platform moves its whole balance.",
  fee: (fee: string, receive: string) => `Fee about ${fee} · You receive about ${receive}`,
  quoting: "Working out the fee…",
  feeOnNextStep: "Fee shown on the next step",
  invalidAmount: "Enter an amount.",
  why: { amount: "Enter an amount first.", quote: "Wait for the fee first.", max: "Working out the most you can move…" },
  continue: "Continue",
  cancel: "Cancel",
  unsupported: "Moving straight between Shielded and Platform isn't available on desktop yet. Move through your Dash wallet.",
  none: {
    absent: "Transfers move Dash between your Dash wallet, Shielded and Platform. This version has neither Shielded nor Platform addresses.",
    inactive: "Transfers move Dash between your Dash wallet, Shielded and Platform. Shielded is not active on this network yet, and this version has no Platform addresses.",
  },
  platformEmpty: "Nothing on your Platform addresses to move yet.",
  noCoreBalance: "Nothing in your Dash wallet to move yet.",
  loading: "Reading your balances…",
  running: (what: string) => `Running: ${what}.`,
  show: "Show",
  routeNames: {
    coreToShielded: SHIELDED_COPY.titles.add,
    coreToPlatform: "Add to Platform",
    shieldedToCore: "Move from Shielded to Dash wallet",
    platformToCore: "Move from Platform to Dash wallet",
    send: SHIELDED_COPY.titles.send,
  },
} as const;

/** What the sheet knows of the balances, from the reads the Home cards use. */
export interface Pools {
  /** The Dash wallet's balance in duffs; `null` while it is not read. */
  core: number | null;
  /** `null`: this dw-app has no Shielded commands; `undefined`: still being read. `duffs` is `null`
   * while it is no reading. */
  shielded: { active: boolean; syncing: boolean; duffs: number | null } | null | undefined;
  /** `null`: this dw-app has no Platform addresses; `undefined`: still being read. `duffs`: what a
   * move of everything would take. */
  platform: { duffs: number | null; dust: boolean; dustText: string; block: MoveBlock | null; waitingText: string } | null | undefined;
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

/** The other end after a pick made both ends the same pool. */
export function defaultTo(from: Pool, pools: Pools): Pool {
  if (from !== "core") return "core";
  return pools.shielded?.active || !pools.platform ? "shielded" : "platform";
}

export function defaultFrom(to: Pool, pools: Pools): Pool {
  if (to !== "core") return "core";
  return pools.shielded?.active || !pools.platform ? "shielded" : "platform";
}

/** Whether the ends can swap: the reverse is a route with a command. */
export const canSwap = (from: Pool, to: Pool): boolean => {
  const r = routeOf(to, from);
  return r !== null && r !== "unsupported";
};

/** The kinds of the Shielded flow each of its routes runs (a lock's Finish belongs to the add). */
const SHIELDED_KINDS: Record<"coreToShielded" | "shieldedToCore", readonly ShieldedKind[]> = { coreToShielded: ["add", "resume"], shieldedToCore: ["out"] };

/** The route a Shielded flow kind belongs to; `"send"` for a payment to another Shielded address. */
export const shieldedRoute = (kind: ShieldedKind): "coreToShielded" | "shieldedToCore" | "send" => (kind === "send" ? "send" : kind === "out" ? "shieldedToCore" : "coreToShielded");

export interface Flows {
  shielded: ShieldedFlow;
  fund: PlatformFundFlow;
  move: PlatformMoveFlow;
}

/** The route's flow is running or holds a result not seen yet: the sheet opens at its step. */
export function routeActive(route: RouteId, flows: Flows): boolean {
  switch (route) {
    case "coreToShielded":
    case "shieldedToCore": {
      const f = flows.shielded;
      return (f.running || f.unseen) && SHIELDED_KINDS[route].includes(f.kind);
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
    case "shieldedToCore": {
      const s = pools.shielded;
      if (s === undefined) return TRANSFER_COPY.loading;
      if (!s?.active) return SHIELDED_COPY.card.inactive;
      if (s.syncing) return SHIELDED_SYNCING_TEXT;
      const f = flows.shielded;
      // The flow is shared: another of its moves is running, or its result waits to be seen.
      if (f.running) return SHIELDED_COPY.card.why.busy;
      if (f.unseen) return SHIELDED_COPY.unseen;
      if (route === "shieldedToCore" && !s.duffs) return SHIELDED_COPY.card.why.empty;
      return null;
    }
    case "coreToPlatform":
      return pools.platform ? null : pools.platform === undefined ? TRANSFER_COPY.loading : "Platform addresses are not active on this network yet.";
    case "platformToCore": {
      const p = pools.platform;
      if (p === undefined) return TRANSFER_COPY.loading;
      if (!p) return "Platform addresses are not active on this network yet.";
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

export type LineState<Q> = { kind: "idle" } | { kind: "quoting" } | { kind: "quoted"; amount: number; quote: Q } | { kind: "refused"; amount: number; error: unknown };

export interface Timers {
  set(f: () => void, ms: number): unknown;
  clear(t: unknown): void;
}

const WINDOW_TIMERS: Timers = { set: (f, ms) => setTimeout(f, ms), clear: (t) => clearTimeout(t as ReturnType<typeof setTimeout>) };

/** How long the amount stays the same before the fee line reads its quote. */
export const QUOTE_DEBOUNCE_MS = 400;

/** Routes whose quote is read only on Continue (and Max), never while typing: `platform_fund_quote`
 * may derive a receive address under the wallet-manager write lock (platform_fund.rs:743-751). */
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
  /** Max's answer is dropped once the amount changes under it. */
  private maxGen = 0;

  constructor(
    private readonly fetch: (duffs: number) => Promise<Q>,
    private readonly timers: Timers = WINDOW_TIMERS,
    private readonly delayMs = QUOTE_DEBOUNCE_MS,
  ) {
    super();
  }

  /** The amount to quote, in duffs; `null`: nothing to quote (no amount, or the route can't run). */
  set(amount: number | null) {
    // Nothing to quote still clears what Max left (its refusal names no amount typed).
    if (amount === this.target && !this.maxing && (amount !== null || this.state.kind === "idle")) return;
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

  /** Max (one quote of `probe`, then one of what `derive` makes of its answer): the amount the second
   * quote took, or `null` when either refused (the line shows that refusal) or the amount changed
   * meanwhile. `derive` returns `probe` itself when the first quote already took it. */
  async max(probe: number, derive: (first: Settled<Q>, probe: number) => number | null): Promise<number | null> {
    if (this.maxing) return null;
    const gen = ++this.maxGen;
    this.timers.clear(this.timer);
    this.timer = null;
    this.due = false;
    this.target = null;
    this.maxing = true;
    this.state = { kind: "quoting" };
    this.changed();
    const live = () => gen === this.maxGen;
    try {
      while (this.inflight) await this.inflight;
      if (!live()) return null;
      const first = await this.exclusive(probe);
      if (!live()) return null;
      const amount = derive(first, probe);
      if (amount === null) {
        this.state = first.ok ? { kind: "quoted", amount: probe, quote: first.quote } : { kind: "refused", amount: probe, error: first.error };
        return null;
      }
      const second = amount === probe && first.ok ? first : await this.exclusive(amount);
      if (!live()) return null;
      if (!second.ok) {
        this.state = { kind: "refused", amount, error: second.error };
        return null;
      }
      this.target = amount;
      this.state = { kind: "quoted", amount, quote: second.quote };
      return amount;
    } finally {
      if (live()) {
        this.maxing = false;
        this.changed();
      }
    }
  }

  /** One quote while nothing else is out. */
  private async exclusive(duffs: number): Promise<Settled<Q>> {
    const p = this.run(duffs);
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

/** The figures of a `{needed, available}` refusal of `code` in duffs, else `null`. */
function duffsShortfall(e: unknown, code: string): { needed: number; available: number } | null {
  if (!isAppError(e) || e.code !== code) return null;
  const d = e.data;
  return d?.kind === "duffs" && d.needed !== null ? { needed: d.needed, available: d.available } : null;
}

/** Max of a route funded from the Dash wallet: the quote of the whole balance takes it, or its
 * `funding_insufficient` names what is needed with the fee and what is available, so the most is
 * `available − (needed − amount)`. */
export function coreFundedMax(code: string) {
  return (first: Settled<unknown>, probe: number): number | null => {
    if (first.ok) return probe;
    const s = duffsShortfall(first.error, code);
    if (!s) return null;
    const most = s.available - (s.needed - probe);
    return most > 0 ? most : null;
  };
}

/** Max of Shielded → Dash wallet: the balance less the quoted fee, or from `insufficient_funds`'s
 * figures; in whole duffs. */
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

export const MAX_DERIVE = {
  coreToShielded: coreFundedMax(SHIELDED_CODES.fundingInsufficient),
  coreToPlatform: coreFundedMax("credits.funding_insufficient"),
  shieldedToCore: shieldedSpendMax,
} as const;

export type TransferApi = ShieldedApi & PlatformFundApi & PlatformMoveApi & Pick<Backend, "shieldQuote" | "unshieldQuote" | "platformFundQuote">;

export interface Lines {
  coreToShielded: QuoteLine<ShieldQuote>;
  coreToPlatform: QuoteLine<PlatformFundQuote>;
  shieldedToCore: QuoteLine<ShieldedSpendQuote>;
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
  /** The route whose flow the sheet opened. */
  private begun: RouteId | null = null;
  /** The text Max last filled on each route: discreet mode masks it while the field still holds it. */
  readonly maxText: Partial<Record<Exclude<RouteId, "platformToCore">, string>> = {};
  readonly lines: Lines;

  constructor(
    readonly flows: Flows,
    api: Pick<Backend, "shieldQuote" | "unshieldQuote" | "platformFundQuote">,
    walletId: string,
    timers: Timers = WINDOW_TIMERS,
  ) {
    super();
    this.lines = {
      coreToShielded: new QuoteLine((d) => api.shieldQuote(walletId, d), timers),
      coreToPlatform: new QuoteLine((d) => api.platformFundQuote(walletId, d), timers),
      shieldedToCore: new QuoteLine((d) => api.unshieldQuote(walletId, String(BigInt(d) * CREDITS_PER_DUFF), null), timers),
    };
    for (const f of [flows.shielded, flows.fund, flows.move]) f.subscribe(() => this.onFlow());
  }

  /** The route whose flow the sheet opened (its form edits that flow's amount). */
  get begunRoute(): RouteId | null {
    return this.begun;
  }

  /** The route the ends make: a route, `"unsupported"` (Shielded ↔ Platform) or `null`. */
  get route(): RouteId | "unsupported" | null {
    return routeOf(this.from, this.to);
  }

  /** Whether the sheet shows: opened here, or a route's flow opened itself (a Finish on a card). */
  get shown(): boolean {
    return this.open || this.flowRoute() !== null;
  }

  /** The route of the flow that is open now, if a transfer route's. */
  flowRoute(): RouteId | null {
    const { shielded, fund, move } = this.flows;
    if (shielded.open && shielded.kind !== "send") return shieldedRoute(shielded.kind) as RouteId;
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

  /** Opens the sheet from a card's pool, to the Dash wallet. */
  startFrom(from: "shielded" | "platform") {
    this.open = true;
    this.picking = null;
    this.select(from, "core");
  }

  togglePicker(end: "from" | "to") {
    this.picking = this.picking === end ? null : end;
    this.changed();
  }

  /** A pick: if it makes both ends the same pool, the other end moves to its default. */
  pick(end: "from" | "to", pool: Pool, pools: Pools) {
    this.picking = null;
    this.picked = { end, n: (this.picked?.n ?? 0) + 1 };
    if (end === "from") this.select(pool, this.to === pool ? defaultTo(pool, pools) : this.to);
    else this.select(this.from === pool ? defaultFrom(pool, pools) : this.from, pool);
  }

  swap() {
    if (canSwap(this.from, this.to)) this.select(this.to, this.from);
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
        if ((s.running || s.unseen) && !SHIELDED_KINDS[route].includes(s.kind)) return;
        this.begun = route;
        s.begin(route === "coreToShielded" ? (s.running || s.unseen ? s.kind : "add") : "out");
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

  private flowOf(route: RouteId): { close(): void } {
    return route === "coreToShielded" || route === "shieldedToCore" ? this.flows.shielded : route === "coreToPlatform" ? this.flows.fund : this.flows.move;
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
    const { shielded, fund, move } = this.flows;
    if (route === "coreToPlatform") return fund.open;
    if (route === "platformToCore") return move.open;
    return shielded.open && shielded.kind !== "send" && shieldedRoute(shielded.kind) === route;
  }

  /** The amount typed on `route` (`null`: none, or the route can't run): the fee line quotes it,
   * except on a route quoted only on Continue, where typing only clears what Max left. */
  typed(route: Exclude<RouteId, "platformToCore">, amount: number | null) {
    (this.lines[route] as QuoteLine<unknown>).set(QUOTE_ON_CONTINUE.has(route) ? null : amount);
  }

  /** Max on `route`: `probe` is the pool's whole balance in duffs. The field is filled (`fill`, the
   * amount in duffs) only from a second quote that took it; `text` is how the field shows that amount. */
  async max(route: Exclude<RouteId, "platformToCore">, probe: number, fill: (duffs: number) => void, text?: (duffs: number) => string): Promise<void> {
    const line = this.lines[route] as QuoteLine<unknown>;
    const got = await line.max(probe, MAX_DERIVE[route] as (first: Settled<unknown>, probe: number) => number | null);
    if (got === null) return;
    if (text) this.maxText[route] = text(got);
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
