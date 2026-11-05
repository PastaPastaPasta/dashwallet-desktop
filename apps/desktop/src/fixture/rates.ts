// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's prices (MP-01, docs/contracts/rates-engine.md): the settings, what rates.json holds
// and what the next attempt gets, read under the engine's rules (`dw_engine::rates::state`, `view`
// and `currencies`) at FIXTURE_NOW. No request leaves the fixture; `requests` counts the attempts
// the engine would make, so a test can see that none happen while prices are off.

import type { AppError } from "../api/generated/commands";
import type { CurrencyRate } from "../api/generated/CurrencyRate";
import type { RatesSettings } from "../api/generated/RatesSettings";
import type { RatesView } from "../api/generated/RatesView";
import type { Ctx, Handlers } from "./handlers";
import type { World } from "./scenarios";
import { FIXTURE_NOW, secs } from "./wallet";

const FRESH_SECS = 1800;
const EXPIRE_SECS = 86_400;
const REFRESH_MIN_SECS = 30;

export interface RatesWorld {
  settings: RatesSettings;
  /** rates.json's prices. */
  cached: CurrencyRate[];
  /** Currencies whose price moved more than half since the one before. */
  volatile: string[];
  lastAttemptAt: number | null;
  lastAttemptFailed: boolean;
  /** When the chosen currency's price was last fetched. */
  lastSuccessAt: number | null;
  /** What the next attempt gets; `null`: every source fails. */
  serves: CurrencyRate[] | null;
  /** Attempts made: what would have gone to the network. */
  requests: number;
}

/** A 0.1 install: prices on by default, but no request before the disclosure was seen. */
export function newRatesWorld(): RatesWorld {
  return {
    settings: { enabled: true, currency: null, disclosed: false },
    cached: [],
    volatile: [],
    lastAttemptAt: null,
    lastAttemptFailed: false,
    lastSuccessAt: null,
    serves: null,
    requests: 0,
  };
}

const now = () => secs(FIXTURE_NOW);

function offReason(world: World): Extract<RatesView["state"], { kind: "off" }>["reason"] | null {
  const s = world.rates.settings;
  if (!s.enabled) return "disabled";
  if (!s.disclosed) return "notDisclosed";
  if (world.launch.network.kind !== "mainnet") return "notMainnet";
  return null;
}

/** `Rates::status`. */
export function ratesView(world: World): RatesView {
  const r = world.rates;
  const view: RatesView = {
    settings: { ...r.settings },
    state: { kind: "unavailable", reason: "neverFetched" },
    rate: null,
    volatile: false,
    lastAttemptFailed: r.lastAttemptFailed,
    lastAttemptAt: r.lastAttemptAt,
    lastSuccessAt: null,
  };
  const off = offReason(world);
  if (off) return { ...view, state: { kind: "off", reason: off } };
  const code = r.settings.currency;
  if (code === null) return { ...view, state: { kind: "unavailable", reason: "noCurrency" } };
  const row = r.cached.find((c) => c.code === code);
  if (!row) {
    const reason = r.lastAttemptAt === null ? "neverFetched" : r.cached.length === 0 ? "failed" : "currencyMissing";
    return { ...view, state: { kind: "unavailable", reason } };
  }
  const age = now() - row.rateAt;
  view.lastSuccessAt = r.lastSuccessAt;
  if (age > EXPIRE_SECS) return { ...view, state: { kind: "unavailable", reason: "expired" } };
  const fresh = age <= FRESH_SECS;
  return {
    ...view,
    state: { kind: fresh ? "fresh" : "stale" },
    rate: { currency: code, price: row.price, rateAt: row.rateAt, source: row.source },
    volatile: fresh && r.volatile.includes(code),
  };
}

const changed = ({ emit }: Ctx) => emit({ kind: "rates" });

const fail = (code: AppError["code"], message: string): AppError => ({ code, message, data: null });

export function ratesHandlers(): Handlers {
  return {
    rates_status: (_, { world }) => ratesView(world),
    rates_currencies: (_, { world }) => (offReason(world) ? [] : world.rates.cached.filter((c) => now() - c.rateAt <= EXPIRE_SECS)),
    set_rates_settings: ({ settings }, ctx) => {
      if (settings.currency !== null && !/^[A-Z]{3}$/.test(settings.currency)) throw fail("rates.invalid_currency", `not a currency code: ${settings.currency}`);
      ctx.world.rates.settings = { ...settings };
      changed(ctx);
      return ratesView(ctx.world);
    },
    refresh_rates: (_, ctx) => {
      const { world } = ctx;
      const r = world.rates;
      const off = offReason(world);
      if (off) throw fail("rates.off", `prices are off (${off})`);
      if (r.lastAttemptAt !== null && now() - r.lastAttemptAt < REFRESH_MIN_SECS) throw fail("rates.throttled", "the last attempt was under 30 seconds ago");
      r.requests += 1;
      r.lastAttemptAt = now();
      r.lastAttemptFailed = r.serves === null;
      if (r.serves !== null) {
        r.cached = r.serves.map((c) => ({ ...c }));
        if (r.cached.some((c) => c.code === r.settings.currency)) r.lastSuccessAt = now();
      }
      changed(ctx);
      return ratesView(world);
    },
  };
}

// Scenarios (MP-01): on mainnet, since test coins have no price; the testnet scenarios stay fiat-free.

const at = (agoSecs: number) => now() - agoSecs;

/** The prices CTX lists, taken `agoSecs` ago. */
export function ctxPrices(agoSecs: number): CurrencyRate[] {
  const t = at(agoSecs);
  return [
    ["USD", "49.0967"],
    ["EUR", "43.8712"],
    ["GBP", "38.1504"],
    ["JPY", "7873.9517"],
    ["KWD", "15.4476"],
    ["CHF", "41.0233"],
    ["BRL", "268.5341"],
  ].map(([code, price]) => ({ code, price, rateAt: t, source: "ctx" }));
}

/** The window opened on Mainnet, the network saved for the next start too. */
export function onMainnet(w: World): World {
  w.launch = { ...w.launch, network: { kind: "mainnet", dirName: "mainnet", unitName: "DASH" } };
  if (w.pending.networkChoice) w.pending.networkChoice = { ...w.pending.networkChoice, chosen: "mainnet", saved: true };
  return w;
}

/** Mainnet with prices; rows carry stamps once the disclosure was seen. */
function priced(w: World, r: Partial<RatesWorld>): World {
  onMainnet(w);
  if (r.settings?.disclosed ?? true) stamped(w);
  const prices = ctxPrices(120);
  w.rates = {
    ...newRatesWorld(),
    settings: { enabled: true, currency: "USD", disclosed: true },
    cached: prices,
    serves: prices,
    lastAttemptAt: at(120),
    lastSuccessAt: at(120),
    ...r,
  };
  return w;
}

/** Price stamps (rates-engine §6) at each row's own time: the newest in USD, the next seen before
 * prices were on (none), the third from before a switch to USD (EUR, kept), then USD. */
function stamped(w: World): World {
  const rows = w.wallets[0]?.rows ?? [];
  const stamps: ([string, string] | null)[] = [["USD", "48.1203"], null, ["EUR", "41.2500"], ["USD", "51.3377"], ["USD", "50.0021"]];
  stamps.forEach((s, i) => {
    const row = rows[i];
    if (s && row) row.fiatThen = { currency: s[0], price: s[1], rateAt: row.timestamp ?? at(0), source: "ctx" };
  });
  return w;
}

/** `w` on Mainnet with a fresh USD price, as the `fiat` scenario. */
export const withPrices = (w: World): World => priced(w, {});

export function ratesScenarios<T>(base: (t: T) => World) {
  return {
    /** Mainnet with a fresh USD price: the hero, the breakdown and the rows show fiat. */
    fiat: (t: T) => priced(base(t), {}),
    /** Two hours since the last price: shown with the stale warning. */
    "fiat-stale": (t: T) => priced(base(t), { cached: ctxPrices(7200), lastAttemptAt: at(60), lastAttemptFailed: true, lastSuccessAt: at(7200), serves: null }),
    /** The price moved more than half since the one before. */
    "fiat-volatile": (t: T) => priced(base(t), { volatile: ["USD"] }),
    /** Every source failed and nothing is cached: no number anywhere. */
    "fiat-unavailable": (t: T) => priced(base(t), { cached: [], lastAttemptAt: at(60), lastAttemptFailed: true, lastSuccessAt: null, serves: null }),
    /** "Show values in local currency" off: no request and no fiat. */
    "fiat-off": (t: T) => priced(base(t), { settings: { enabled: false, currency: "USD", disclosed: true } }),
    /** Yen, which has no minor unit. */
    "fiat-jpy": (t: T) => priced(base(t), { settings: { enabled: true, currency: "JPY", disclosed: true } }),
    /** A 0.1 install opened on mainnet: Home asks once before any request. */
    "fiat-disclosure": (t: T) => priced(base(t), { settings: { enabled: true, currency: null, disclosed: false }, cached: [], lastAttemptAt: null, lastSuccessAt: null }),
  };
}
