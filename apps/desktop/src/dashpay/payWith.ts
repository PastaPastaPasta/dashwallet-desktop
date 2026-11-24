// The registration's "Pay with" step (C-funding F19, F16, F18): the sources a new identity can be
// paid from, each with its balance and, when it cannot pay, the one reason. Dash wallet is
// selected every time the step opens and the choice is never kept (F18, ruling M4). Plain
// TypeScript over the generated types; the screen is screens/dashpay/Registration.tsx.

import type { RegistrationQuote } from "../api/generated/RegistrationQuote";
import type { ShieldedCheckName } from "../api/generated/ShieldedCheckName";
import type { ShieldedReadiness } from "../api/generated/ShieldedReadiness";
import { creditsToDuffs } from "../api/pending";
import { messageTable, t } from "../i18n";
import { withTable } from "./status";

export type PaySource = "core" | "platform" | "shielded";

/** The sources this build's engine can register from: Dash wallet; Platform and Shielded once
 * their flow steps land, until then each reads "Not available on this network yet." */
export const LIVE_SOURCES: ReadonlySet<PaySource> = new Set<PaySource>(["core"]);

export const PAY_WITH_COPY = withTable(
  messageTable("dashpay.registration.payWith", ["link", "title", "core", "platform", "shielded", "notOffered", "unsettled", "syncing"]),
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

/** The pool is not offered here: the start refuses `platform.feature_off` for these. */
const NOT_OFFERED: readonly ShieldedCheckName[] = ["poolActive", "featureOn", "denomination"];

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

/** One row per source, Dash wallet first. */
export function payWithRows(i: PayWithInputs): PaySourceRow[] {
  const live = i.live ?? LIVE_SOURCES;
  const shown = (duffs: number | null) => (duffs === null || i.discreet ? null : i.amount(duffs));
  const coreShort = i.core !== null && i.core.availableDuffs < i.core.totalDuffs ? PAY_WITH_COPY.short(i.amount(i.core.totalDuffs)) : null;
  return [
    { source: "core", label: PAY_WITH_COPY.core, balance: shown(i.core?.availableDuffs ?? null), reason: coreShort },
    { source: "platform", label: PAY_WITH_COPY.platform, balance: shown(i.platformDuffs), reason: live.has("platform") ? null : PAY_WITH_COPY.notOffered },
    {
      source: "shielded",
      label: PAY_WITH_COPY.shielded,
      balance: shown(i.shieldedDuffs),
      reason: live.has("shielded") ? shieldedReason(i.readiness, i.amount, i.clock) : PAY_WITH_COPY.notOffered,
    },
  ];
}
