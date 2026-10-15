// The display settings applied to amounts (U-04 D4): dash-qt's unit everywhere at once, the
// decimal digits on Home's hero and breakdown. The engine's `NetworkInfo.unitName` names DASH
// only; the other units' names come from `unitName` here, as dash-qt's `BitcoinUnits::name`.

import { useMemo } from "react";
import type { DisplayUnit } from "../api/generated/DisplayUnit";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { UiSettings } from "../api/generated/UiSettings";
import { useUi } from "../app/ui";
import { compact, floored, formatWithUnit, full, masked, unitName, type Chain, type Unit } from "./amount";

export const UNIT_OF: Record<DisplayUnit, Unit> = { dash: "DASH", milliDash: "mDASH", microDash: "uDASH", duffs: "duffs" };

/** The units' names in General and in the amount fields' refusals, whatever the network. */
export const UNIT_LABEL: Record<DisplayUnit, string> = { dash: "DASH", milliDash: "mDASH", microDash: "μDASH", duffs: "duffs" };

export function chainOf(network: NetworkInfo): Chain {
  return network.kind === "mainnet" ? "main" : "test";
}

export interface Display {
  /** The unit typed amounts are parsed in. */
  unit: DisplayUnit;
  /** The same unit, as `format/amount.ts` names it. */
  amountUnit: Unit;
  chain: Chain;
  /** The unit's name on this network ("tDASH", "mtDASH", …). */
  name: string;
  full(duffs: number, signed?: boolean): string;
  compact(duffs: number, signed?: boolean): string;
  /** Hero and breakdown: floored to the decimal digits setting. */
  floored(duffs: number): string;
  masked(): string;
  withUnit(duffs: number): string;
  /** An amount field's placeholder and its refusal of text that is not an amount. */
  placeholder: string;
  invalidAmount: string;
}

export function display(prefs: Pick<UiSettings, "unit" | "decimalDigits">, network: NetworkInfo): Display {
  const unit = UNIT_OF[prefs.unit];
  const chain = chainOf(network);
  const digits = prefs.decimalDigits;
  const duffs = prefs.unit === "duffs";
  return {
    unit: prefs.unit,
    amountUnit: unit,
    chain,
    name: unitName(unit, chain),
    full: (amount, signed = false) => full(amount, signed, unit),
    compact: (amount, signed = false) => compact(amount, signed, unit),
    floored: (amount) => floored(amount, digits, unit),
    masked: () => masked(digits, unit),
    withUnit: (amount) => formatWithUnit(unit, amount, false, "standard", chain),
    placeholder: duffs ? "0" : "0.00",
    invalidAmount: `Enter an amount in ${UNIT_LABEL[prefs.unit]}, for example ${duffs ? "25000" : "0.25"}.`,
  };
}

/** The display settings for `network`; screens re-render when General changes them. */
export function useDisplay(network: NetworkInfo): Display {
  const unit = useUi((s) => s.prefs.unit);
  const decimalDigits = useUi((s) => s.prefs.decimalDigits);
  return useMemo(() => display({ unit, decimalDigits }, network), [unit, decimalDigits, network]);
}
