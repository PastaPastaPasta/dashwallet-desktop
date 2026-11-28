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

export function middleTruncate(value: string): string {
  return value.length > 16 ? `${value.slice(0, 8)}…${value.slice(-7)}` : value;
}

export function Identifier({ value, mono = false, suffix }: { value: string; mono?: boolean; suffix?: string }) {
  const shown = middleTruncate(value);
  const full = suffix ? `${value}${suffix}` : value;
  const visible = suffix ? `${shown}${suffix}` : shown;
  if (shown === value) {
    return (
      <bdi className={mono ? "identifier mono" : "identifier"} title={full}>
        {full}
      </bdi>
    );
  }
  return (
    <bdi className={mono ? "identifier mono" : "identifier"} title={full}>
      <span aria-hidden="true">{visible}</span>
      <span className="sr-only">{full}</span>
    </bdi>
  );
}

/** `value` with a copy button; `display` is shown in its place when given (a shortened id). The
 * button's name is `action`, a whole sentence ("Copy data directory"), else the catalog's copy
 * sentence around the noun `label`. */
export function CopyValue({ value, label, action, mono = false, display, truncate = false, suffix }: { value: string; mono?: boolean; display?: string; truncate?: boolean; suffix?: string } & ({ label: string; action?: undefined } | { label?: undefined; action: string })) {
  const t = useT();
  const copied = suffix ? `${value}${suffix}` : value;
  return (
    <span className="copy-value">
      {truncate ? <Identifier value={value} mono={mono} suffix={suffix} /> : <span className={mono ? "mono" : undefined}>{display ?? copied}</span>}
      <CopyIconButton text={copied} label={action ?? t("components.copy.value", { label: label! })} />
    </span>
  );
}
