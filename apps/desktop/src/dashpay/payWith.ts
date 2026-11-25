// The registration's "Pay with" step (C-funding F19, F16, F18): the sources a new identity can be
// paid from, each with its balance and, when it cannot pay, the one reason. Dash wallet is
// selected every time the step opens and the choice is never kept (F18, ruling M4). Plain
// TypeScript over the generated types; the screen is screens/dashpay/Registration.tsx.

import { hasCode } from "../api/backend";
import type { RegistrationQuote } from "../api/generated/RegistrationQuote";
import type { ShieldedCheckName } from "../api/generated/ShieldedCheckName";
import type { ShieldedReadiness } from "../api/generated/ShieldedReadiness";
import { creditsToDuffs, creditsToDuffsUp } from "../api/pending";
import { messageTable, t } from "../i18n";
import { withTable } from "./status";

export type PaySource = "core" | "platform" | "shielded";

/** The sources this build's engine can register from: Dash wallet and Platform (off on mainnet by
 * the engine's flag, which refuses its quote); Shielded once its flow step lands, until then it
 * reads "Not available on this network yet." */
export const LIVE_SOURCES: ReadonlySet<PaySource> = new Set<PaySource>(["core", "platform"]);

export const PAY_WITH_COPY = withTable(
  messageTable("dashpay.registration.payWith", ["link", "title", "core", "platform", "shielded", "notOffered", "movePending", "unsettled", "held", "syncing"]),
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
  /** A payment that may still go through holds part of the Shielded balance. */
  shieldedHeld?: boolean;
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

/** The pool is not offered here: the start refuses `platform.feature_off` for these. */
const NOT_OFFERED: readonly ShieldedCheckName[] = ["poolActive", "featureOn", "denomination"];

/** The first readiness check that fails, in the engine's order, as the step says it (F16). An
 * unsettled spend that is a hold (`held`) offers no "Try again": it lasts until the network shows
 * the payment can no longer go through. */
export function shieldedReason(readiness: ShieldedReadiness | null, amount: (duffs: number) => string, clock: (secs: number) => string, held = false): string | null {
  if (readiness === null) return null;
  const failed = readiness.checks.find((c) => !c.ready)?.check;
  if (failed === undefined) return null;
  if (NOT_OFFERED.includes(failed)) return PAY_WITH_COPY.notOffered;
  switch (failed) {
    case "balance":
      return readiness.denomination === null ? PAY_WITH_COPY.notOffered : PAY_WITH_COPY.short(amount(creditsToDuffs(readiness.denomination)));
    case "noUnsettledSpend":
      return held ? PAY_WITH_COPY.held : PAY_WITH_COPY.unsettled;
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
      reason: live.has("shielded") ? shieldedReason(i.readiness, i.amount, i.clock, i.shieldedHeld) : PAY_WITH_COPY.notOffered,
    },
  ];
}
