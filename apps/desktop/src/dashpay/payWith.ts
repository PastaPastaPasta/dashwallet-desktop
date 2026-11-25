// The registration's source step (C-funding F19, F16, F18), "Make your username private" as the
// iOS and Android wallets show it: the sources a new identity can be paid from, each with its
// balance and, when it cannot pay, the one reason; and the tip card shown instead while the
// Shielded balance cannot pay. Nothing is selected when the step opens and the choice is never
// kept. Plain TypeScript over the generated types; the screen is screens/dashpay/Registration.tsx.

import { hasCode } from "../api/backend";
import type { RegistrationQuote } from "../api/generated/RegistrationQuote";
import type { ShieldedReadiness } from "../api/generated/ShieldedReadiness";
import { creditsToDuffs, creditsToDuffsUp } from "../api/pending";
import { messageTable, t } from "../i18n";
import { NOT_OFFERED, withTable } from "./status";

export type PaySource = "core" | "platform" | "shielded";

/** The sources this build's engine can register from: Dash wallet, Shielded, and Platform (off on
 * mainnet by the engine's flag, which refuses its quote). */
export const LIVE_SOURCES: ReadonlySet<PaySource> = new Set<PaySource>(["core", "platform", "shielded"]);

/** Whether the step opens with Shielded selected when it can pay. Off: nothing is selected, as on
 * iOS, and Continue waits for a pick. Never Dash wallet. */
export const PRESELECT_SHIELDED = false;

const P = "dashpay.registration.privacy.tip";

export const PRIVACY_COPY = withTable(
  messageTable("dashpay.registration.privacy", ["title", "body", "shieldedNote", "coreNote", "advanced", "shieldFirst", "withoutPrivacy"]),
  {
    tip: {
      resting: (time: string) => ({ title: t(`${P}.restingTitle`), text: t(`${P}.resting`, { time }) }),
      pool: (notes: string, required: string) => ({ title: t(`${P}.poolTitle`), text: t(`${P}.pool`, { notes, required }) }),
      partly: (have: string, need: string, more: string) => ({ title: t(`${P}.partlyTitle`), text: t(`${P}.partly`, { have, need, more }) }),
      nothing: (need: string) => ({ title: t(`${P}.nothingTitle`), text: t(`${P}.nothing`, { need }) }),
      /** Discreet mode: the price alone, under the part-shielded title. */
      partlyDiscreet: (need: string) => ({ title: t(`${P}.partlyTitle`), text: t(`${P}.nothing`, { need }) }),
      wait: (text: string) => ({ title: t(`${P}.waitTitle`), text }),
    },
  },
);

export const PAY_WITH_COPY = withTable(
  messageTable("dashpay.registration.payWith", ["link", "title", "core", "platform", "shielded", "notOffered", "movePending", "unsettled", "syncing"]),
  {
    short: (amount: string) => t("dashpay.registration.payWith.short", { amount }),
    resting: (time: string) => t("dashpay.registration.payWith.resting", { time }),
  },
);

/** What the step reads. Duffs; `null` while unread. */
export interface PayWithInputs {
  /** The Core quote: what the funding account can spend and what the registration takes. */
  core: Pick<RegistrationQuote, "availableDuffs" | "totalDuffs"> | null;
  platformDuffs: number | null;
  /** The Platform quote of a plain name: `undefined` while unread, else the quote or its refusal. */
  platform: { quote: RegistrationQuote } | { error: unknown } | undefined;
  shieldedDuffs: number | null;
  /** The Shielded quote's readiness (C4b-1). */
  readiness: ShieldedReadiness | null;
  /** Duffs with the unit. */
  amount: (duffs: number) => string;
  /** Unix seconds as a clock time. */
  clock: (secs: number) => string;
  /** Discreet mode: no balances. */
  discreet: boolean;
  live?: ReadonlySet<PaySource>;
}

export interface PaySourceRow {
  source: PaySource;
  label: string;
  /** The balance with its unit, `null` while unread or in discreet mode. */
  balance: string | null;
  /** Why this source cannot pay now; `null` when it can. */
  reason: string | null;
}

/** The first readiness check that fails, in the engine's order, as the step says it (F16). */
export function shieldedReason(readiness: ShieldedReadiness | null, amount: (duffs: number) => string, clock: (secs: number) => string): string | null {
  if (readiness === null) return null;
  const failed = readiness.checks.find((c) => !c.ready)?.check;
  if (failed === undefined) return null;
  if (NOT_OFFERED.includes(failed)) return PAY_WITH_COPY.notOffered;
  switch (failed) {
    case "balance":
      return readiness.denomination === null ? PAY_WITH_COPY.notOffered : PAY_WITH_COPY.short(amount(creditsToDuffs(readiness.denomination)));
    case "noUnsettledSpend":
      return PAY_WITH_COPY.unsettled;
    case "rested":
      return readiness.readyAt === null ? PAY_WITH_COPY.syncing : PAY_WITH_COPY.resting(clock(readiness.readyAt));
    default:
      return PAY_WITH_COPY.syncing;
  }
}

/** Why the Platform balance cannot pay, from its quote's refusal (C-funding F9): short of the
 * planned debit, held by another move, or off on this network. */
export function platformReason(platform: PayWithInputs["platform"], amount: (duffs: number) => string): string | null {
  if (platform === undefined) return t("transfer.loading");
  if ("quote" in platform) return null;
  const e = platform.error;
  if (hasCode(e, "platform.feature_off")) return PAY_WITH_COPY.notOffered;
  if (hasCode(e, "credits.move_pending")) return PAY_WITH_COPY.movePending;
  if (hasCode(e, "platform.insufficient_credits") && e.data?.kind === "credits") return PAY_WITH_COPY.short(amount(creditsToDuffsUp(e.data.needed)));
  return t("transfer.move.error.unavailable");
}

/** One row per source, Dash wallet first. */
export function payWithRows(i: PayWithInputs): PaySourceRow[] {
  const live = i.live ?? LIVE_SOURCES;
  const shown = (duffs: number | null) => (duffs === null || i.discreet ? null : i.amount(duffs));
  const coreShort = i.core !== null && i.core.availableDuffs < i.core.totalDuffs ? PAY_WITH_COPY.short(i.amount(i.core.totalDuffs)) : null;
  return [
    { source: "core", label: PAY_WITH_COPY.core, balance: shown(i.core?.availableDuffs ?? null), reason: coreShort },
    { source: "platform", label: PAY_WITH_COPY.platform, balance: shown(i.platformDuffs), reason: live.has("platform") ? platformReason(i.platform, i.amount) : PAY_WITH_COPY.notOffered },
    {
      source: "shielded",
      label: PAY_WITH_COPY.shielded,
      balance: shown(i.shieldedDuffs),
      reason: live.has("shielded") ? shieldedReason(i.readiness, i.amount, i.clock) : PAY_WITH_COPY.notOffered,
    },
  ];
}

/** The card shown instead of the sources while the Shielded balance cannot pay (C-funding F16). */
export interface PrivacyTip {
  kind: "resting" | "pool" | "partly" | "nothing" | "wait";
  title: string;
  text: string;
}

/** What the source step shows: the sources to pick from, or a tip card with "Shield your funds
 * first" and "Continue without privacy". Platform is behind Advanced in both, only when it can pay. */
export type PrivacyStep =
  | {
      kind: "sources";
      /** Shielded (only when it can pay), then Dash wallet (always). */
      rows: PaySourceRow[];
      /** The source the step opens on: Shielded under `PRESELECT_SHIELDED`, else none. */
      preselect: PaySource | null;
      platform: PaySourceRow | null;
    }
  | { kind: "tip"; tip: PrivacyTip; shieldFirst: boolean; platform: PaySourceRow | null }
  | { kind: "loading"; platform: PaySourceRow | null };

const NOTES = (n: number) => new Intl.NumberFormat().format(n);

/** The tip for the first readiness check that fails (`null`: Shielded can pay, or is not offered
 * on this network at all). Discreet mode shows no balance, so a part-shielded balance reads as
 * the price alone. */
export function privacyTip(i: Pick<PayWithInputs, "readiness" | "shieldedDuffs" | "amount" | "clock" | "discreet">): PrivacyTip | null {
  const r = i.readiness;
  const failed = r?.checks.find((c) => !c.ready)?.check;
  if (r === null || failed === undefined || NOT_OFFERED.includes(failed) || r.denomination === null) return null;
  const need = creditsToDuffs(r.denomination);
  switch (failed) {
    case "balance": {
      const have = i.shieldedDuffs;
      if (have === null || have <= 0) return { kind: "nothing", ...PRIVACY_COPY.tip.nothing(i.amount(need)) };
      if (i.discreet) return { kind: "partly", ...PRIVACY_COPY.tip.partlyDiscreet(i.amount(need)) };
      return { kind: "partly", ...PRIVACY_COPY.tip.partly(i.amount(have), i.amount(need), i.amount(Math.max(0, need - have))) };
    }
    case "rested":
      return r.readyAt === null ? { kind: "wait", ...PRIVACY_COPY.tip.wait(PAY_WITH_COPY.syncing) } : { kind: "resting", ...PRIVACY_COPY.tip.resting(i.clock(r.readyAt)) };
    case "poolSize":
      return { kind: "pool", ...PRIVACY_COPY.tip.pool(NOTES(r.poolNotes ?? 0), NOTES(r.poolRequired)) };
    case "noUnsettledSpend":
      return { kind: "wait", ...PRIVACY_COPY.tip.wait(PAY_WITH_COPY.unsettled) };
    default:
      return { kind: "wait", ...PRIVACY_COPY.tip.wait(PAY_WITH_COPY.syncing) };
  }
}

/** The source step (C-funding F19, mobile parity): Shielded is offered only when it can pay now,
 * Dash wallet always, Platform behind Advanced only when its quote covers the cost (its quote is
 * refused on mainnet). While Shielded cannot pay, a tip card replaces the sources; "Shield your
 * funds first" waits while the funds rest or the pool is too small. */
export function privacyStep(i: PayWithInputs): PrivacyStep {
  const live = i.live ?? LIVE_SOURCES;
  const [core, platform, shielded] = payWithRows(i);
  const advanced = live.has("platform") && platform.reason === null ? platform : null;
  if (!live.has("shielded")) return { kind: "sources", rows: [core], preselect: null, platform: advanced };
  if (i.readiness === null) return { kind: "loading", platform: advanced };
  const tip = privacyTip(i);
  if (tip) return { kind: "tip", tip, shieldFirst: tip.kind !== "resting" && tip.kind !== "pool", platform: advanced };
  const canPay = shielded.reason === null;
  const rows = canPay ? [shielded, core] : [core];
  return { kind: "sources", rows, preselect: canPay && PRESELECT_SHIELDED ? "shielded" : null, platform: advanced };
}
