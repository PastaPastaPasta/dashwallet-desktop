// Amount formatting with dash-qt's BitcoinUnits rules, ported from rust/crates/dw-units
// (and checked against the same testdata/amount_format.json vectors), plus the
// UX-SPEC §5.2 compact style. The thousands separator is U+2009, the decimal mark
// always "."; floors truncate, they never round.

export const THIN_SP = " ";
export const COIN = 100_000_000;

export type Unit = "DASH" | "mDASH" | "uDASH" | "duffs";
export type Chain = "main" | "test";
export type Separators = "never" | "standard" | "always";

const FACTOR: Record<Unit, number> = { DASH: 100_000_000, mDASH: 100_000, uDASH: 100, duffs: 1 };
const DECIMALS: Record<Unit, number> = { DASH: 8, mDASH: 5, uDASH: 2, duffs: 0 };
const MAX_DIGITS = 16;

export function unitName(unit: Unit, chain: Chain): string {
  const t = chain === "test";
  switch (unit) {
    case "DASH":
      return t ? "tDASH" : "DASH";
    case "mDASH":
      return t ? "mtDASH" : "mDASH";
    case "uDASH":
      return t ? "μtDASH" : "μDASH";
    case "duffs":
      return t ? "tduffs" : "duffs";
  }
}

/** `BitcoinUnits::format`: the amount without a unit name. */
export function format(unit: Unit, amount: number, plus: boolean, separators: Separators, justify: boolean): string {
  const coin = FACTOR[unit];
  const decimals = DECIMALS[unit];
  const abs = Math.abs(amount);
  let q = [...String(Math.floor(abs / coin))];
  if (justify) {
    const width = MAX_DIGITS - decimals;
    if (q.length < width) q = [...Array<string>(width - q.length).fill(" "), ...q];
  }
  const size = q.length;
  const separate = separators === "always" || (separators === "standard" && size > 4);
  if (separate) {
    // Positions are counted from the right on the unmodified length, as in the Qt loop.
    for (let i = 3; i < size; i += 3) q.splice(size - i, 0, THIN_SP);
  }
  let out = amount < 0 ? "-" : plus && amount > 0 ? "+" : "";
  out += q.join("");
  if (decimals > 0) out += "." + String(abs % coin).padStart(decimals, "0");
  return out;
}

export function formatWithUnit(unit: Unit, amount: number, plus: boolean, separators: Separators, chain: Chain): string {
  return `${format(unit, amount, plus, separators, false)} ${unitName(unit, chain)}`;
}

/** `BitcoinUnits::floorWithUnit`: decimals cut (not rounded) to `digits`. */
export function floorWithUnit(
  unit: Unit,
  amount: number,
  plus: boolean,
  separators: Separators,
  digits: number,
  chain: Chain,
): string {
  let s = format(unit, amount, plus, separators, false);
  const decimals = DECIMALS[unit];
  if (decimals > digits) s = s.slice(0, Math.max(0, s.length - (decimals - digits)));
  return `${s} ${unitName(unit, chain)}`;
}

/** `GUIUtil::formatAmount`: always separated, optionally truncated to `truncate` decimals. */
export function formatAmount(unit: Unit, amount: number, signed: boolean, truncate: number | null, chain: Chain): string {
  let s = format(unit, amount, signed, "always", false);
  if (truncate !== null) {
    const dot = s.indexOf(".");
    if (dot >= 0) {
      if (truncate === 0) s = s.slice(0, dot);
      else if (s.length > dot + 1 + truncate) s = s.slice(0, dot + 1 + truncate);
    }
  }
  return `${s} ${unitName(unit, chain)}`;
}

/** Trailing zeros after the decimal mark removed, keeping at least two decimals. */
export function trimDecimals(number: string): string {
  const dot = number.indexOf(".");
  if (dot < 0) return number;
  let end = number.length;
  while (end > dot + 3 && number[end - 1] === "0") end--;
  return number.slice(0, end);
}

/** UX-SPEC §5.2 `.compact`: the exact value, trailing zeros trimmed to two decimals, never rounded. */
export function compact(amount: number, signed: boolean, unit: Unit = "DASH"): string {
  return trimDecimals(format(unit, amount, signed, "standard", false));
}

/** UX-SPEC §5.2 hero and breakdown: floored to the Decimal digits setting (default 2). */
export function floored(amount: number, digits = 2, unit: Unit = "DASH"): string {
  const s = format(unit, amount, false, "standard", false);
  const decimals = DECIMALS[unit];
  return decimals > digits ? s.slice(0, s.length - (decimals - digits)) : s;
}

/** Full unit decimals (`.gui`): confirm sheets, transaction detail. */
export function full(amount: number, signed: boolean, unit: Unit = "DASH"): string {
  return format(unit, amount, signed, "standard", false);
}

/** Discreet mode (QT-039): every digit of the floored zero replaced by `#`. */
export function masked(digits = 2): string {
  return floored(0, digits).replace(/0/g, "#");
}
