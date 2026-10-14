// The pieces every Settings tab is built of: a titled card, a row that is not available yet
// (with the task that brings it, D7), and a row's hint.

import { useId, type ReactNode } from "react";

export function SettingsCard({ title, testId, children }: { title: string; testId: string; children: ReactNode }) {
  const id = useId();
  return (
    <section className="card-l1 settings-card" aria-labelledby={id} data-testid={testId}>
      <h2 id={id} className="settings-card-title">
        {title}
      </h2>
      {children}
    </section>
  );
}

/** A setting the app does not have yet: its name, and why or who brings it. */
export function NotYetRow({ title, why, testId, label = "Not available yet" }: { title: string; why: string; testId: string; label?: string }) {
  return (
    <div className="settings-row settings-not-yet" data-testid={testId}>
      <span className="settings-row-title">{title}</span>
      <span className="field-hint">
        {label}: {why}
      </span>
    </div>
  );
}

export function Hint({ children, id }: { children: ReactNode; id?: string }) {
  return (
    <p className="field-hint settings-hint" id={id}>
      {children}
    </p>
  );
}

export function Problem({ text, testId }: { text: string | null; testId: string }) {
  if (!text) return null;
  return (
    <p className="form-problem" role="alert" data-testid={testId}>
      {text}
    </p>
  );
}

export function Done({ text, testId }: { text: string | null; testId: string }) {
  if (!text) return null;
  return (
    <p className="settings-done" role="status" data-testid={testId}>
      {text}
    </p>
  );
}
