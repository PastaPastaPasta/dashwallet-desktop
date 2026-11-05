// Amount entry in fiat (MP-01, SEND-4; rates-engine §0): a toggle swaps an amount field between the
// display unit and the chosen currency. The fiat typed is converted here, half to even, to duffs and
// written into the field's DASH text, so the engine still parses, estimates and sends that DASH text
// only: fiat never reaches a send. Typing in fiat needs a fresh price; when it stops being fresh the
// field is DASH again with the same duffs.

import { useEffect, useId, useRef, useState } from "react";
import { useUi } from "../app/ui";
import { DashButton } from "../components/basics";
import type { Display } from "../format/display";
import { duffsToMinor, minorToDuffs, minorToPlain, minorUnits, parseFiat } from "../format/fiat";
import { FiatLine, useFiat } from "./rates";
import { ENTRY_TEXT } from "./entryText";

export { ENTRY_TEXT };


/** An amount field the entry drives: its DASH text and the engine's parse of it. */
export interface AmountText {
  text: string;
  setText: (text: string) => void;
  /** Duffs once parsed; `undefined` while empty or unread, `null` when not an amount. */
  duffs: number | null | undefined;
}

export interface FiatEntry {
  /** The field takes fiat now. */
  active: boolean;
  /** The currency of fiat entry, while a price may show. */
  currency: string | null;
  /** What the input shows and its change handler. */
  value: string;
  onChange: (text: string) => void;
  /** The unit the label names. */
  unitName: string;
  placeholder: string;
  /** The typed fiat is not an amount, or too large to convert. */
  error: string | null;
  /** The field went back to DASH by itself. */
  note: string | null;
  /** `why`: the reason it is disabled, on the line `whyId` names (DEC-1058). */
  toggle: { shown: boolean; disabled: boolean; why: string | null; whyId: string; flip: () => void };
  /** In fiat: the DASH the typed fiat converts to. */
  dash: string | null;
  fiat: ReturnType<typeof useFiat>;
  discreet: boolean;
}

export function useFiatEntry(shown: Display, amount: AmountText): FiatEntry {
  const id = useId();
  const fiat = useFiat();
  const discreet = useUi((s) => s.discreet);
  const view = fiat.view;
  const fresh = view?.state.kind === "fresh" && view.rate ? view.rate : null;
  const offered = !!view && view.state.kind !== "off" && !discreet;
  const [active, setActive] = useState(false);
  const [fiatText, setFiatText] = useState("");
  const [note, setNote] = useState<string | null>(null);
  // The DASH text the entry last wrote, the currency it typed in, and whether fiat was typed.
  const wrote = useRef<string | null>(null);
  const currency = useRef<string | null>(null);
  const typed = useRef(false);

  const convert = (text: string, rate: { price: string; currency: string }) => {
    const minor = parseFiat(text, rate.currency);
    const duffs = minor === null ? null : minorToDuffs(minor, rate.price, rate.currency);
    return duffs === null || duffs > BigInt(Number.MAX_SAFE_INTEGER) ? null : Number(duffs);
  };
  const write = (text: string) => {
    wrote.current = text;
    amount.setText(text);
  };
  const leave = (why: string | null) => {
    setActive(false);
    setNote(why);
    wrote.current = null;
    typed.current = false;
  };

  // Max or a reset wrote the DASH text: the field is DASH again.
  useEffect(() => {
    if (active && amount.text !== wrote.current) leave(null);
  }, [amount.text, active]);
  // No fresh price, another currency or discreet mode: DASH again, the duffs kept.
  useEffect(() => {
    if (active && (!fresh || fresh.currency !== currency.current || discreet)) leave(ENTRY_TEXT.backToDash);
  }, [active, fresh, discreet]);
  // A new fresh price converts the typed fiat again.
  useEffect(() => {
    if (active && typed.current && fresh && fresh.currency === currency.current && fiatText.trim() !== "") {
      const duffs = convert(fiatText, fresh);
      if (duffs !== null) write(shown.full(duffs));
    }
  }, [fresh?.price]);

  const flip = () => {
    setNote(null);
    if (active) return leave(null);
    if (!fresh) return;
    // The field's duffs are kept until fiat is typed: switching alone never changes the amount.
    const duffs = typeof amount.duffs === "number" && amount.duffs > 0 ? amount.duffs : null;
    const minor = duffs === null ? null : duffsToMinor(duffs, fresh.price, fresh.currency);
    setFiatText(minor === null ? "" : minorToPlain(minor, fresh.currency));
    wrote.current = amount.text;
    currency.current = fresh.currency;
    typed.current = false;
    setActive(true);
  };
  const onChange = (text: string) => {
    if (!active) return amount.setText(text);
    setFiatText(text);
    typed.current = true;
    const duffs = fresh && text.trim() !== "" ? convert(text, fresh) : null;
    write(duffs === null ? "" : shown.full(duffs));
  };

  const code = view?.rate?.currency ?? view?.settings.currency ?? null;
  const typedAmount = active && fiatText.trim() !== "" && code !== null;
  const invalid = typedAmount && parseFiat(fiatText, code) === null;
  // An amount whose duffs are past what a number holds exactly: never left as an empty DASH text.
  const tooLarge = typedAmount && !invalid && !!fresh && convert(fiatText, fresh) === null;
  const converted = active && fresh && !invalid && !tooLarge && typed.current ? convert(fiatText, fresh) : null;
  return {
    active,
    currency: code,
    value: active ? fiatText : amount.text,
    onChange,
    unitName: active && code ? code : shown.name,
    placeholder: active && code ? (minorUnits(code) === 0 ? "0" : "0.00") : shown.placeholder,
    error: invalid && code ? ENTRY_TEXT.invalid(code) : tooLarge ? ENTRY_TEXT.tooLarge : null,
    note,
    toggle: {
      shown: offered && code !== null,
      disabled: !active && !fresh,
      why: active || fresh ? null : view?.state.kind === "stale" ? ENTRY_TEXT.notFresh : ENTRY_TEXT.unavailable,
      whyId: `${id}-fiat-why`,
      flip,
    },
    dash: active ? (converted !== null ? `${shown.full(converted)} ${shown.name}` : typeof amount.duffs === "number" && !typed.current ? `${shown.full(amount.duffs)} ${shown.name}` : null) : null,
    fiat,
    discreet,
  };
}

/** The toggle beside an amount field: "USD" to type in fiat, the display unit to type in it again. */
export function FiatToggle({ entry, shown, testId }: { entry: FiatEntry; shown: Display; testId: string }) {
  if (!entry.toggle.shown) return null;
  const to = entry.active ? shown.name : entry.currency!;
  return (
    <DashButton
      variant="tintedGray"
      size="small"
      className="amount-toggle"
      data-testid={testId}
      aria-pressed={entry.active}
      aria-label={`Enter the amount in ${to}`}
      why={entry.toggle.why}
      whyId={entry.toggle.whyId}
      disabled={entry.toggle.disabled}
      onClick={entry.toggle.flip}
    >
      <span aria-hidden="true">⇅ </span>
      {to}
    </DashButton>
  );
}

/** Under the field: the amount in the other unit, and why the field went back to DASH. */
export function FiatSecondary({ entry, duffs, testId }: { entry: FiatEntry; duffs: number | null | undefined; testId: string }) {
  return (
    <>
      {entry.active ? (
        entry.dash && (
          <span className="amount-secondary" data-testid={testId}>
            {entry.dash}
          </span>
        )
      ) : (
        <FiatLine fiat={entry.fiat} duffs={typeof duffs === "number" && duffs > 0 ? duffs : null} hidden={entry.discreet} className="amount-secondary" testId={testId} />
      )}
      {entry.toggle.shown && entry.toggle.why && (
        <span id={entry.toggle.whyId} className="field-hint" data-testid={`${testId}-why`}>
          {entry.toggle.why}
        </span>
      )}
      {entry.note && (
        <span className="field-hint" role="status" data-testid={`${testId}-note`}>
          {entry.note}
        </span>
      )}
    </>
  );
}
