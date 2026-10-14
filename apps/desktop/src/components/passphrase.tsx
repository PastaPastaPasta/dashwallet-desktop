// Passphrase inputs (QT-111, IOS-010, U-02 D12, D17, D18): the new passphrase twice with its
// strength meter, and the one-field prompt of the unlock sheet, Show Recovery Phrase and the
// onboarding unlock step. Each keeps the typed text only in its fields and clears them after
// every submit.

import { useId, useState, type FormEvent, type ReactNode } from "react";
import { passphraseStrength, STRENGTH_NAME, STRENGTHS, type Strength } from "../onboarding/passphrase";
import { DashButton } from "./basics";

/** Four bars and the strength's name. */
export function StrengthMeter({ strength }: { strength: Strength }) {
  const level = STRENGTHS.indexOf(strength);
  return (
    <div className="strength" data-strength={strength} data-testid="passphrase-strength">
      <span className="strength-bars" aria-hidden="true">
        {[1, 2, 3, 4].map((n) => (
          <span key={n} className={n <= level ? "strength-bar strength-bar-on" : "strength-bar"} />
        ))}
      </span>
      <span className="strength-name" aria-live="polite">
        {strength === "none" ? "" : `Strength: ${STRENGTH_NAME[strength]}`}
      </span>
    </div>
  );
}

/** The new passphrase and its confirmation, with the meter for the first. `onEdit` hears every
 * keystroke (to clear the last refusal); `onSubmit` gets both fields. */
export function NewPassphraseForm({
  testId,
  error,
  submitLabel,
  onEdit,
  onSubmit,
  children,
}: {
  testId: string;
  error: string | null;
  submitLabel: string;
  onEdit: () => void;
  onSubmit: (text: string, confirmation: string) => void | Promise<unknown>;
  children?: ReactNode;
}) {
  const id = useId();
  const [text, setText] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const submit = (e: FormEvent) => {
    e.preventDefault();
    const [a, b] = [text, confirmation];
    setText("");
    setConfirmation("");
    void onSubmit(a, b);
  };
  return (
    <form className="secret-form" onSubmit={submit} data-testid={`${testId}-form`}>
      <div className="form-field">
        <label htmlFor={`${id}-new`}>New passphrase</label>
        <input
          id={`${id}-new`}
          type="password"
          autoComplete="new-password"
          data-testid={`${testId}-new`}
          data-autofocus
          value={text}
          aria-invalid={error ? true : undefined}
          aria-describedby={error ? `${id}-error` : undefined}
          onChange={(e) => {
            setText(e.target.value);
            onEdit();
          }}
        />
      </div>
      <StrengthMeter strength={passphraseStrength(text)} />
      <div className="form-field">
        <label htmlFor={`${id}-repeat`}>Repeat the passphrase</label>
        <input
          id={`${id}-repeat`}
          type="password"
          autoComplete="new-password"
          data-testid={`${testId}-repeat`}
          value={confirmation}
          aria-invalid={error ? true : undefined}
          onChange={(e) => {
            setConfirmation(e.target.value);
            onEdit();
          }}
        />
      </div>
      {error && (
        <p id={`${id}-error`} className="form-problem" role="alert" data-testid={`${testId}-error`}>
          {error}
        </p>
      )}
      {children}
      <DashButton type="submit" variant="filledBlue" size="large" fill data-testid={`${testId}-submit`}>
        {submitLabel}
      </DashButton>
    </form>
  );
}

/** One passphrase field and its submit button. `disabled`: no attempt is accepted now (a wait). */
export function PassphrasePrompt({
  testId,
  error,
  busy = false,
  disabled = false,
  submitLabel,
  onSubmit,
}: {
  testId: string;
  error: string | null;
  busy?: boolean;
  disabled?: boolean;
  submitLabel: string;
  onSubmit: (passphrase: string) => void | Promise<unknown>;
}) {
  const id = useId();
  const [text, setText] = useState("");
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (busy || disabled || !text) return;
    const passphrase = text;
    setText("");
    void onSubmit(passphrase);
  };
  return (
    <form className="secret-form" onSubmit={submit} data-testid={`${testId}-form`}>
      <div className="form-field">
        <label htmlFor={id}>Passphrase</label>
        <input
          id={id}
          type="password"
          autoComplete="current-password"
          data-testid={`${testId}-field`}
          // A disabled field cannot take focus: the sheet's first enabled control does.
          data-autofocus={!disabled || undefined}
          value={text}
          disabled={disabled}
          aria-invalid={error ? true : undefined}
          aria-describedby={error ? `${id}-error` : undefined}
          onChange={(e) => setText(e.target.value)}
        />
      </div>
      <p id={`${id}-error`} className="form-problem" role="alert" data-testid={`${testId}-error`}>
        {error}
      </p>
      <DashButton
        type="submit"
        variant="filledBlue"
        size="large"
        fill
        disabled={busy || disabled || !text}
        data-testid={`${testId}-submit`}
      >
        {busy ? "Checking…" : submitLabel}
      </DashButton>
    </form>
  );
}
