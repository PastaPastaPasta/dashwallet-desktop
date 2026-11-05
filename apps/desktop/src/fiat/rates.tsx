// Fiat lines (MP-01, UX-SPEC §5.3): the price state the engine keeps (rates-engine §2) as the screens read
// it. A fiat line shows only with a fresh or stale price and never where the DASH amount is masked or
// unknown; there is no "≈" and no 0 ("never 0, never guessed").

import { useQuery } from "@tanstack/react-query";
import type { RatesView } from "../api/generated/RatesView";
import { keys, useBackend } from "../api/queries";
import { fiatText } from "../format/fiat";

export function useRates() {
  const b = useBackend();
  return useQuery({ queryKey: keys.ratesStatus, queryFn: () => b.ratesStatus() });
}

export function useRateCurrencies(enabled: boolean) {
  const b = useBackend();
  return useQuery({ queryKey: keys.ratesCurrencies, queryFn: () => b.ratesCurrencies(), enabled });
}

/** "14:05": the time of day a price is from. */
export function timeText(secs: number, locale: string | undefined): string {
  return new Date(secs * 1000).toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit" });
}

/** The price a fiat value may be computed with now, or `null`. */
export function shownRate(view: RatesView | undefined) {
  if (!view?.rate) return null;
  if (view.state.kind !== "fresh" && view.state.kind !== "stale") return null;
  return { ...view.rate, stale: view.state.kind === "stale" };
}

export interface Fiat {
  /** The fiat text of `duffs`, `null` when none may be shown. */
  text(duffs: number | null | undefined): string | null;
  /** A price is shown and it is over 30 minutes old. */
  stale: boolean;
  /** A price is shown. */
  shown: boolean;
  view: RatesView | undefined;
  /** "Price from 14:05, may be out of date" while stale. */
  staleTitle: string | undefined;
}

export function useFiat(): Fiat {
  const b = useBackend();
  const view = useRates().data;
  const rate = shownRate(view);
  const time = rate ? timeText(rate.rateAt, b.locale) : "";
  return {
    view,
    shown: rate !== null,
    stale: rate?.stale ?? false,
    staleTitle: rate?.stale ? `Price from ${time}, may be out of date` : undefined,
    text: (duffs) => (rate && typeof duffs === "number" ? fiatText(duffs, rate.price, rate.currency, b.locale) : null),
  };
}

/** The fiat line under an amount. Renders nothing when `hidden` (the amount is masked) or when no
 * price may be shown. */
export function FiatLine({ fiat, duffs, hidden, className, testId }: { fiat: Fiat; duffs: number | null | undefined; hidden?: boolean; className?: string; testId: string }) {
  const text = hidden ? null : fiat.text(duffs);
  if (text === null) return null;
  return (
    <span className={`fiat-line${fiat.stale ? " is-stale" : ""}${className ? ` ${className}` : ""}`} data-testid={testId} title={fiat.staleTitle}>
      {text}
    </span>
  );
}

/** What the hero says about prices (rates-engine §2), beside or instead of the fiat line; `null`
 * says nothing. */
export function ratesNote(view: RatesView | undefined): { text: string; tone: "caution" | "quiet"; title?: string } | null {
  if (!view) return null;
  switch (view.state.kind) {
    case "off":
      return null;
    case "fresh":
      return view.volatile ? { text: "Prices moved more than 50% since the last update.", tone: "caution" } : null;
    case "stale":
      return { text: "Prices are over 30 minutes old.", tone: "caution" };
    case "unavailable":
      return { text: "Prices unavailable", tone: "quiet", title: UNAVAILABLE_TEXT[view.state.reason] };
  }
}

export const UNAVAILABLE_TEXT: Record<Extract<RatesView["state"], { kind: "unavailable" }>["reason"], string> = {
  neverFetched: "Prices have not been fetched yet.",
  failed: "Prices could not be fetched. The wallet tries again in a few minutes.",
  expired: "The last prices are over 24 hours old, so none are shown.",
  noCurrency: "Choose a local currency in Settings ▸ General.",
  currencyMissing: "There is no price for the chosen currency. Choose another in Settings ▸ General.",
};

const SOURCE_TEXT = { ctx: "rates.ctx.com", bitPayDashCentral: "BitPay and DashCentral" } as const;

const TEST_ONLY_TEXT = "Test coins have no price: prices are fetched only while Mainnet is open.";

/** The status line: when the shown price is from, or why there is none. */
export function ratesStatusText(view: RatesView, locale: string | undefined): string | null {
  const failed = view.lastAttemptFailed && view.lastAttemptAt !== null ? `Last update failed at ${timeText(view.lastAttemptAt, locale)}.` : null;
  switch (view.state.kind) {
    case "off":
      return view.state.reason === "notMainnet" ? TEST_ONLY_TEXT : null;
    case "unavailable":
      return UNAVAILABLE_TEXT[view.state.reason];
    case "fresh":
    case "stale": {
      const r = view.rate!;
      const from = `Updated ${timeText(r.rateAt, locale)} from ${SOURCE_TEXT[r.source]}.`;
      const stale = view.state.kind === "stale" ? " Prices are over 30 minutes old." : "";
      return [from + stale, failed].filter(Boolean).join(" ");
    }
  }
}

/** The disclosure, word for word, wherever it is shown (onboarding, Home's card, Settings). */
export const RATES_DISCLOSURE =
  "The wallet asks rates.ctx.com for DASH prices about every 5 minutes, and BitPay and DashCentral when it cannot reach it. No address, balance or transaction is sent; the requests show these services your IP address, the time, and that a Dash wallet is running.";
