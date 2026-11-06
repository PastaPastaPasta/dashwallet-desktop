// Fiat values of DASH amounts (MP-01, docs/contracts/rates-engine.md §0). The engine gives a price as an
// exact decimal string (fiat per 1 DASH); this module is the only place an amount is converted. All
// arithmetic is on integers (BigInt) and rounds once, half to even, to the currency's ISO 4217 minor
// unit; the text is built from that exact integer, so nothing rounds again (no float ever reaches
// Intl.NumberFormat). testdata/fiat_convert.json pins the rounding.

const DUFFS_PER_DASH = 100_000_000n;

/** ISO 4217 minor units where they are not 2. Pinned here rather than read from Intl: the webviews
 * ship different ICU data, and an amount must convert the same on every OS. */
const MINOR_UNITS: Record<string, number> = {
  BIF: 0, CLP: 0, DJF: 0, GNF: 0, ISK: 0, JPY: 0, KMF: 0, KRW: 0, PYG: 0, RWF: 0, UGX: 0, VND: 0, VUV: 0,
  XAF: 0, XOF: 0, XPF: 0,
  BHD: 3, IQD: 3, JOD: 3, KWD: 3, LYD: 3, OMR: 3, TND: 3,
};

/** Codes a source lists that are not a local currency: crypto, metals, units of account. */
const NOT_LOCAL = new Set(["BTC", "ETH", "LTC", "XAG", "XAU", "XDR", "CLF", "USDT", "USDC"]);

export function isLocalCurrency(code: string): boolean {
  return /^[A-Z]{3}$/.test(code) && !NOT_LOCAL.has(code);
}

/** The currency's decimal places (ISO 4217). */
export function minorUnits(currency: string): number {
  return MINOR_UNITS[currency] ?? 2;
}

interface Decimal {
  mantissa: bigint;
  scale: number;
}

const PRICE = /^([0-9]{1,15})(?:\.([0-9]{1,12}))?$/;

/** A price as the engine sends it; `null` for anything else (zero included). */
export function parsePrice(price: string): Decimal | null {
  const m = PRICE.exec(price);
  if (!m) return null;
  const frac = m[2] ?? "";
  const mantissa = BigInt(m[1] + frac);
  return mantissa > 0n ? { mantissa, scale: frac.length } : null;
}

/** `n / d` rounded half to even (`n ≥ 0`, `d > 0`). */
export function divHalfEven(n: bigint, d: bigint): bigint {
  const q = n / d;
  const r2 = (n % d) * 2n;
  if (r2 < d) return q;
  if (r2 > d) return q + 1n;
  return q % 2n === 0n ? q : q + 1n;
}

const pow10 = (n: number) => 10n ** BigInt(n);

/** `duffs` in the currency's minor units at `price`, half to even; `null` for a bad price or a
 * negative or non-integer amount. */
export function duffsToMinor(duffs: number | bigint, price: string, currency: string): bigint | null {
  const p = parsePrice(price);
  if (p === null) return null;
  if (typeof duffs === "number" && !Number.isSafeInteger(duffs)) return null;
  const d = BigInt(duffs);
  if (d < 0n) return null;
  return divHalfEven(d * p.mantissa * pow10(minorUnits(currency)), DUFFS_PER_DASH * pow10(p.scale));
}

/** The duffs `minor` units of the currency buy at `price`, half to even (amount entry in fiat). */
export function minorToDuffs(minor: bigint, price: string, currency: string): bigint | null {
  const p = parsePrice(price);
  if (p === null || minor < 0n) return null;
  return divHalfEven(minor * DUFFS_PER_DASH * pow10(p.scale), p.mantissa * pow10(minorUnits(currency)));
}

/** Typed fiat text to minor units: digits with at most one "." and at most the currency's decimals;
 * spaces and thin spaces are ignored, as in DASH amounts. `null` when it is not such a number. */
export function parseFiat(text: string, currency: string): bigint | null {
  const t = text.replace(/[\s ]/g, "");
  const m = /^([0-9]*)(?:\.([0-9]*))?$/.exec(t);
  if (!m || t === "" || t === "." || m[1].length > 15) return null;
  const e = minorUnits(currency);
  const frac = m[2] ?? "";
  if (frac.length > e) return null;
  return BigInt((m[1] || "0") + frac.padEnd(e, "0"));
}

/** The minor units as plain text with "." ("12.30", "7874"): what a fiat field shows. */
export function minorToPlain(minor: bigint, currency: string): string {
  const e = minorUnits(currency);
  const s = minor.toString().padStart(e + 1, "0");
  return e === 0 ? s : `${s.slice(0, -e)}.${s.slice(-e)}`;
}

/** `minor` (≥ 0) as the locale writes the currency, built from the exact integer: Intl places the
 * symbol, the grouping and the decimal mark around integer and fraction digits we supply. */
export function formatMinor(minor: bigint, currency: string, locale?: string): string {
  const e = minorUnits(currency);
  const unit = pow10(e);
  const int = minor / unit;
  const frac = (minor % unit).toString().padStart(e, "0");
  let nf: Intl.NumberFormat;
  try {
    nf = new Intl.NumberFormat(locale, { style: "currency", currency, numberingSystem: "latn", minimumFractionDigits: e, maximumFractionDigits: e });
  } catch {
    nf = new Intl.NumberFormat(undefined, { style: "currency", currency, numberingSystem: "latn", minimumFractionDigits: e, maximumFractionDigits: e });
  }
  // A BigInt is formatted exactly; its fraction digits are zeros, replaced by ours.
  return nf
    .formatToParts(int)
    .map((p) => (p.type === "fraction" ? frac : p.value))
    .join("");
}

/** The fiat line of an amount: `null` for 0 or a bad price ("never 0"); "< US$0.01" when a non-zero
 * amount rounds to nothing. The sign is the DASH amount's to show, so this is of the magnitude. */
export function fiatText(duffs: number, price: string, currency: string, locale?: string): string | null {
  if (!Number.isSafeInteger(duffs) || duffs === 0) return null;
  const minor = duffsToMinor(Math.abs(duffs), price, currency);
  if (minor === null) return null;
  if (minor === 0n) return `< ${formatMinor(1n, currency, locale)}`;
  return formatMinor(minor, currency, locale);
}

/** The currency's name in the locale, or its code. */
export function currencyName(code: string, locale?: string): string {
  try {
    return new Intl.DisplayNames(locale ? [locale] : undefined, { type: "currency" }).of(code) ?? code;
  } catch {
    return code;
  }
}

/** The currency a locale's region uses (a proposal for the first choice), or USD. */
export function localeCurrency(locale: string | undefined): string {
  let region: string | undefined;
  try {
    region = new Intl.Locale(locale ?? "en-US").maximize().region;
  } catch {
    region = undefined;
  }
  return (region && REGION_CURRENCY[region]) || "USD";
}

const EURO = "AD AT BE CY DE EE ES FI FR GR HR IE IT LT LU LV MC ME MT NL PT SI SK SM VA XK".split(" ");

const REGION_CURRENCY: Record<string, string> = {
  ...Object.fromEntries(EURO.map((r) => [r, "EUR"])),
  AE: "AED", AF: "AFN", AL: "ALL", AM: "AMD", AO: "AOA", AR: "ARS", AU: "AUD", AZ: "AZN", BA: "BAM", BB: "BBD",
  BD: "BDT", BG: "BGN", BH: "BHD", BI: "BIF", BM: "BMD", BN: "BND", BO: "BOB", BR: "BRL", BS: "BSD", BT: "BTN",
  BW: "BWP", BY: "BYN", BZ: "BZD", CA: "CAD", CD: "CDF", CH: "CHF", CL: "CLP", CN: "CNY", CO: "COP", CR: "CRC",
  CU: "CUP", CV: "CVE", CZ: "CZK", DJ: "DJF", DK: "DKK", DO: "DOP", DZ: "DZD", EG: "EGP", ER: "ERN", ET: "ETB",
  FJ: "FJD", GB: "GBP", GE: "GEL", GH: "GHS", GM: "GMD", GN: "GNF", GT: "GTQ", GY: "GYD", HK: "HKD", HN: "HNL",
  HT: "HTG", HU: "HUF", ID: "IDR", IL: "ILS", IN: "INR", IQ: "IQD", IR: "IRR", IS: "ISK", JM: "JMD", JO: "JOD",
  JP: "JPY", KE: "KES", KG: "KGS", KH: "KHR", KM: "KMF", KR: "KRW", KW: "KWD", KY: "KYD", KZ: "KZT", LA: "LAK",
  LB: "LBP", LK: "LKR", LR: "LRD", LY: "LYD", MA: "MAD", MD: "MDL", MG: "MGA", MK: "MKD", MM: "MMK", MN: "MNT",
  MO: "MOP", MR: "MRU", MU: "MUR", MV: "MVR", MW: "MWK", MX: "MXN", MY: "MYR", MZ: "MZN", NA: "NAD", NG: "NGN",
  NI: "NIO", NO: "NOK", NP: "NPR", NZ: "NZD", OM: "OMR", PA: "PAB", PE: "PEN", PG: "PGK", PH: "PHP", PK: "PKR",
  PL: "PLN", PY: "PYG", QA: "QAR", RO: "RON", RS: "RSD", RU: "RUB", RW: "RWF", SA: "SAR", SB: "SBD", SC: "SCR",
  SD: "SDG", SE: "SEK", SG: "SGD", SO: "SOS", SR: "SRD", SS: "SSP", SV: "USD", SY: "SYP", TH: "THB", TJ: "TJS",
  TM: "TMT", TN: "TND", TO: "TOP", TR: "TRY", TT: "TTD", TW: "TWD", TZ: "TZS", UA: "UAH", UG: "UGX", US: "USD",
  UY: "UYU", UZ: "UZS", VE: "VES", VN: "VND", VU: "VUV", WS: "WST", YE: "YER", ZA: "ZAR", ZM: "ZMW",
  CM: "XAF", CF: "XAF", TD: "XAF", CG: "XAF", GQ: "XAF", GA: "XAF",
  BJ: "XOF", BF: "XOF", CI: "XOF", GW: "XOF", ML: "XOF", NE: "XOF", SN: "XOF", TG: "XOF",
  PF: "XPF", NC: "XPF", WF: "XPF", EC: "USD", PR: "USD",
};
