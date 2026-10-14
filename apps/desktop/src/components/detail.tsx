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

export function CopyValue({ value, label, mono = false }: { value: string; label: string; mono?: boolean }) {
  return (
    <span className="copy-value">
      <span className={mono ? "mono" : undefined}>{value}</span>
      <button type="button" className="copy-button" aria-label={`Copy ${label}`} title={`Copy ${label}`} onClick={() => void copyWithToast(value)}>
        <Icon id="copy" size={20} />
      </button>
    </span>
  );
}
