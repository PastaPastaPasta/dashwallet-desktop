// Detail lists (UX-SPEC C11 DetailList): a label and its value, and a value with a copy button.

import type { ReactNode } from "react";
import { useT } from "../i18n";
import { CopyIconButton } from "./copy";

export function DetailRow({ label, children, testId }: { label: string; children: ReactNode; testId?: string }) {
  return (
    <div className="detail-row" data-testid={testId}>
      <dt className="detail-label">{label}</dt>
      <dd className="detail-value">{children}</dd>
    </div>
  );
}

/** `value` with a copy button; `display` is shown in its place when given (a shortened id). The
 * button's name is `copyLabel`, a whole catalog sentence ("Copy data directory"), else "Copy " and
 * the noun `label`. */
export function CopyValue({ value, label, copyLabel, mono = false, display }: { value: string; mono?: boolean; display?: string } & ({ label: string; copyLabel?: never } | { copyLabel: string; label?: never })) {
  const t = useT();
  return (
    <span className="copy-value">
      <span className={mono ? "mono" : undefined}>{display ?? value}</span>
      <CopyIconButton text={value} label={copyLabel ?? t("components.copy.value", { label: label! })} />
    </span>
  );
}
