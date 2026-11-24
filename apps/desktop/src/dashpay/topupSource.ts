// The top-up sheet's "Pay with" (C-routes D-C3, C3-8): Dash wallet, Platform and Shielded, each
// with its balance and, when it cannot pay, the one reason. Dash wallet is chosen every time the
// sheet opens and the choice is never kept. Platform tops up from the wallet's Platform addresses
// (`credits_top_up_from_platform`); Shielded is listed and not offered yet. Plain TypeScript over
// the generated types; the screen is screens/dashpay/TopUpSheet.tsx.

import { isAppError } from "../api/backend";
import type { PlatformActivity } from "../api/generated/PlatformActivity";
import type { PlatformAddressBalance } from "../api/generated/PlatformAddressBalance";
import type { TopUpFromPlatformOutcome } from "../api/generated/TopUpFromPlatformOutcome";
import { creditsToDuffs } from "../api/pending";
import { messageTable, t } from "../i18n";
import { isHeldOp } from "../platform/move";
import { creditCount } from "./profile";
import { PAY_WITH_COPY, type PaySource } from "./payWith";
import { withTable } from "./status";
import { topUpErrorText } from "./topup";

export type { PaySource };

export const SOURCE_COPY = withTable(messageTable("dashpay.topUp.payWith", ["title", "core", "platform", "shielded", "nothing", "max"]), {
  get notOffered() {
    return PAY_WITH_COPY.notOffered;
  },
  /** The Platform addresses are not read yet. */
  get loading() {
    return t("transfer.loading");
  },
});

const from = <K extends string>(prefix: string, keys: readonly K[]) => messageTable(`dashpay.topUp.fromPlatform.${prefix}`, keys);

export const FROM_PLATFORM_COPY = withTable(messageTable("dashpay.topUp.fromPlatform", ["fee", "you"]), {
  confirm: (identity: string, amount: string, total: string, fee: string, address: string) => t("dashpay.topUp.fromPlatform.confirm", { identity, amount, total, fee, address }),
  pending: from("pending", ["title", "text"]),
  held: withTable(from("held", ["text", "action"]), {
    get title() {
      return t("dashpay.topUp.outcome.notApplied.title");
    },
  }),
  failed: from("failed", ["title", "text"]),
  error: withTable(from("error", ["short"]), {
    below: (min: string) => t("dashpay.topUp.fromPlatform.error.below", { min }),
  }),
});

/** How a move, send or top-up holds the Platform addresses: `pending` while Platform has not
 * answered, `held` once it failed by height and may still execute ("Move to Dash wallet" frees
 * it), `null` when nothing holds them. */
export type PlatformHold = "pending" | "held" | null;

export function platformHold(activity: readonly PlatformActivity[] | null | undefined): PlatformHold {
  const held = (activity ?? []).filter(isHeldOp);
  if (held.some((a) => a.state === "pending")) return "pending";
  return held.length > 0 ? "held" : null;
}

/** What the picker reads. Duffs; `undefined` while unread. */
export interface SourceInputs {
  /** The Dash wallet's spendable balance. */
  coreDuffs: number | undefined;
  /** The Platform addresses; `null` when this network has none (below protocol version 11, or
   * the commands are absent). */
  platform: PlatformAddressBalance | null | undefined;
  hold: PlatformHold;
  shieldedDuffs: number | null | undefined;
  amount: (duffs: number) => string;
  discreet: boolean;
}

export interface SourceRow {
  source: PaySource;
  label: string;
  /** The balance with its unit; `null` while unread or in discreet mode. */
  balance: string | null;
  /** Why this source cannot pay now; `null` when it can. */
  reason: string | null;
}

/** One row per source, Dash wallet first. Platform pays when its addresses hold something a move
 * can take and nothing holds them; Shielded is not offered yet. */
export function topUpSources(i: SourceInputs): SourceRow[] {
  const shown = (duffs: number | null | undefined) => (duffs === null || duffs === undefined || i.discreet ? null : i.amount(duffs));
  const p = i.platform;
  const platformReason =
    p === undefined
      ? SOURCE_COPY.loading
      : p === null
        ? SOURCE_COPY.notOffered
        : i.hold === "pending"
          ? FROM_PLATFORM_COPY.pending.text
          : i.hold === "held"
            ? FROM_PLATFORM_COPY.held.text
            : BigInt(p.movableCredits) === 0n
              ? SOURCE_COPY.nothing
              : null;
  return [
    { source: "core", label: SOURCE_COPY.core, balance: shown(i.coreDuffs), reason: null },
    { source: "platform", label: SOURCE_COPY.platform, balance: shown(p?.duffs), reason: platformReason },
    { source: "shielded", label: SOURCE_COPY.shielded, balance: shown(i.shieldedDuffs), reason: SOURCE_COPY.notOffered },
  ];
}

/** The Platform source can be chosen: its row has no reason. */
export const canPay = (rows: readonly SourceRow[], source: PaySource) => rows.find((r) => r.source === source)?.reason === null;

/** What a top-up from Platform came to. "Nothing left" only when the engine says it can never
 * execute (`failed`, not `mayLand`): nothing was spent. Anything else not done or pending may
 * still go through, and offers "Move to Dash wallet". */
export function platformOutcomeText(o: TopUpFromPlatformOutcome, balance: (credits: string) => string): { title: string; text: string; move: boolean } {
  if (o.state === "done") {
    const added = creditCount(o.credits);
    const text = o.balance === null ? t("dashpay.topUp.outcome.plus", { credits: added }) : t("dashpay.topUp.outcome.plusBalance", { credits: added, balance: balance(o.balance) });
    return { title: t("dashpay.topUp.outcome.added"), text, move: false };
  }
  if (o.state === "pending") return { title: FROM_PLATFORM_COPY.pending.title, text: FROM_PLATFORM_COPY.pending.text, move: false };
  if (o.state === "failed" && !o.mayLand) return { title: FROM_PLATFORM_COPY.failed.title, text: FROM_PLATFORM_COPY.failed.text, move: false };
  return { title: FROM_PLATFORM_COPY.held.title, text: FROM_PLATFORM_COPY.held.text, move: true };
}

/** A top-up from Platform's refusal, by code; the rest as a top-up's. */
export function fromPlatformErrorText(e: unknown, amount: (duffs: number) => string, discreet = false): string {
  if (!isAppError(e)) return topUpErrorText(e, amount, discreet);
  switch (e.code) {
    case "credits.move_pending":
      return FROM_PLATFORM_COPY.pending.text;
    case "credits.send_out_of_range":
      return e.data?.kind === "creditMinimum" ? FROM_PLATFORM_COPY.error.below(amount(creditsToDuffs(e.data.min))) : FROM_PLATFORM_COPY.error.short;
    case "platform.insufficient_credits":
      return FROM_PLATFORM_COPY.error.short;
    case "platform.feature_off":
      return SOURCE_COPY.notOffered;
    default:
      return topUpErrorText(e, amount, discreet);
  }
}
