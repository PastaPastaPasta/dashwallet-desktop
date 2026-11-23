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
 * button's name is `action`, a whole sentence ("Copy data directory"), else the catalog's copy
 * sentence around the noun `label`. */
export function CopyValue({ value, label, action, mono = false, display }: { value: string; mono?: boolean; display?: string } & ({ label: string; action?: undefined } | { label?: undefined; action: string })) {
  const t = useT();
  return (
    <span className="copy-value">
      <span className={mono ? "mono" : undefined}>{display ?? value}</span>
      <CopyIconButton text={value} label={action ?? t("components.copy.value", { label: label! })} />
    </span>
  );
}
