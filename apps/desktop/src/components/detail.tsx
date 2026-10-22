// Detail lists (UX-SPEC C11 DetailList): a label and its value, and a value with a copy button.

import type { ReactNode } from "react";
import { copyWithToast } from "./toast";
import { Icon } from "./basics";

export function DetailRow({ label, children, testId }: { label: string; children: ReactNode; testId?: string }) {
  return (
    <div className="detail-row" data-testid={testId}>
      <dt className="detail-label">{label}</dt>
      <dd className="detail-value">{children}</dd>
    </div>
  );
}

/** `value` with a copy button; `display` is shown in its place when given (a shortened id). */
export function CopyValue({ value, label, mono = false, display }: { value: string; label: string; mono?: boolean; display?: string }) {
  return (
    <span className="copy-value">
      <span className={mono ? "mono" : undefined}>{display ?? value}</span>
      <button type="button" className="copy-button" aria-label={`Copy ${label}`} title={`Copy ${label}`} onClick={() => void copyWithToast(value)}>
        <Icon id="copy" size={20} />
      </button>
    </span>
  );
}
