// Lock screen (UX-SPEC §4.4, IOS-012, IOS-013, QT-111, QT-112; U-02 D8, D9): full-window overlay
// on hero blue with the white wordmark and the passphrase prompt, the iOS lockout policy
// (`LockModel`), For mixing only and Forgot passphrase. The unlock sheet (D12) asks for the full
// unlock from mixing only with the same model and copy.

import { useEffect, useRef, useState, type FormEvent } from "react";
import type { Backend } from "../api/backend";
import type { LockOutcome } from "../api/generated/LockOutcome";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { FLOW_NAME, OUTCOME_LINE, reportsAnything, summarizeLockReport } from "../api/lockReport";
import { useBackend, useLastLockReport, useLockDrain, useLockReconcile } from "../api/queries";
import { DashButton, DashLogo, NetworkCapsule } from "../components/basics";
import { PassphrasePrompt } from "../components/passphrase";
import { Sheet } from "../components/sheet";
import { useT, type Translate } from "../i18n";
import { LockModel } from "../onboarding/lock";
import { useModel } from "../onboarding/model";

/** Re-renders with `model` and clears its wait when it runs out. */
export function useLockTimer(model: LockModel) {
  useModel(model);
  const { retryAt } = model;
  useEffect(() => {
    if (retryAt === null) return;
    const t = window.setTimeout(() => model.refresh(), Math.max(0, retryAt - Date.now()) + 50);
    return () => window.clearTimeout(t);
  }, [model, retryAt]);
}

/** A `LockModel` that reads the vault's count and wait when it appears. */
function useLockModel(backend: Backend): LockModel {
  const [model] = useState(() => new LockModel(backend));
  useLockTimer(model);
  useEffect(() => void model.load(), [model]);
  return model;
}

/** "2 more actions, 1 more step" in the language in use. */
function omittedText(t: Translate, counts: [number, "lock.report.moreActions" | "lock.report.moreSteps" | "lock.report.moreWallets"][]): string {
  const items = counts.filter(([n]) => n > 0).map(([count, key]) => t(key, { count }));
  return t("lock.report.notListed", { items: items.join(t("common.listSeparator")) });
}

/** What the last lock did to running flows (E0-13 D3, E0-04 §16.10 C4). Hidden while a newer
 * lock drains: until its `done`, the last report is the previous lock's. */
function LockReportNotice() {
  const t = useT();
  const drain = useLockDrain().data;
  const report = useLastLockReport().data;
  if (drain)
    return (
      <section className="lock-report" data-testid="lock-drain">
        <p className="lock-report-title" role="status">
          {t("lock.report.stopping", { count: drain.inFlight })}
        </p>
      </section>
    );
  return report && reportsAnything(report) ? <LockReportBody report={report} /> : null;
}

/** Unresolved while anything listed may still be sent, or anything left out cannot be shown not
 * to be; `leases`, `dispatchStatus` and `lockStatus` settle only what the report lets them. */
function LockReportBody({ report }: { report: LockOutcome }) {
  const t = useT();
  const summary = summarizeLockReport(report);
  const reconciled = useLockReconcile(report, summary.unresolved);
  const resolved = !summary.unresolved || reconciled.data?.settled === true;
  const { flows, artifacts, wallets } = summary.omitted;
  const check = resolved
    ? null
    : summary.beyondReach
      ? t("lock.report.beyondReach")
      : reconciled.isError
        ? t("lock.report.checkFailed")
        : t("lock.report.checking");
  return (
    <section
      className="lock-report"
      data-testid="lock-report"
      data-resolved={resolved}
      data-checked-at={reconciled.data?.checkedAt}
      aria-labelledby="lock-report-title"
    >
      <p id="lock-report-title" className="lock-report-title" role="status">
        {resolved ? t("lock.report.allEnded") : t("lock.report.signedBeforeLock")}
      </p>
      {check && (
        <p className="lock-report-note" data-testid="lock-report-check">
          {check}
        </p>
      )}
      <ul className="lock-report-flows" aria-label={t("lock.report.atLockTime")}>
        {report.flows.map((f) => (
          <li key={f.lease} data-testid="lock-report-flow" data-outcome={f.outcome}>
            {t("lock.report.flowLine", { flow: FLOW_NAME[f.flow], outcome: OUTCOME_LINE[f.outcome] })}
          </li>
        ))}
      </ul>
      {!summary.complete && (
        <p className="lock-report-note" data-testid="lock-report-omitted">
          {omittedText(t, [
            [flows, "lock.report.moreActions"],
            [artifacts, "lock.report.moreSteps"],
            [wallets, "lock.report.moreWallets"],
          ])}
        </p>
      )}
    </section>
  );
}

/** `covered`: Forgot passphrase is open over it, so it takes no focus and is hidden from
 * assistive tech. */
export function LockScreen({ network, covered = false, onForgot }: { network: NetworkInfo; covered?: boolean; onForgot: () => void }) {
  const t = useT();
  const model = useLockModel(useBackend());
  const [passphrase, setPassphrase] = useState("");
  const [mixingOnly, setMixingOnly] = useState(false);
  // No attempt is taken during a wait, nor after eight failures; then Forgot passphrase is
  // the way on.
  const { blocked, disabled } = model;
  const root = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (!blocked) field.current?.focus();
    else if (disabled) root.current?.querySelector<HTMLElement>('[data-testid="lock-forgot"]')?.focus();
  }, [blocked, disabled]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!passphrase || model.working || blocked) return;
    // The DOM copy of the passphrase is cleared after every attempt (H-01 note in the G-01 report).
    const typed = passphrase;
    setPassphrase("");
    await model.unlock(typed, mixingOnly);
  };

  return (
    <div ref={root} className="lock-screen" role="dialog" aria-modal="true" aria-labelledby="lock-title" data-testid="lock-screen" inert={covered}>
      <div className="lock-column" data-testid="lock-column">
        <DashLogo className="lock-logo" />
        <NetworkCapsule network={network} />
        <h1 id="lock-title" className="lock-title">
          {t("lock.title")}
        </h1>
        <p className="lock-prompt">{t("lock.prompt")}</p>
        <form className="lock-form" onSubmit={submit} data-testid="lock-form">
          <input
            ref={field}
            className="lock-field"
            data-testid="lock-field"
            type="password"
            aria-label={t("lock.passphrase")}
            placeholder={t("lock.passphrase")}
            autoComplete="current-password"
            autoFocus
            value={passphrase}
            disabled={blocked}
            aria-invalid={model.message !== null}
            aria-describedby={model.message ? "lock-error" : undefined}
            onChange={(e) => setPassphrase(e.target.value)}
          />
          <label className="check-row lock-check">
            <input type="checkbox" data-testid="lock-mixing-only" checked={mixingOnly} disabled={blocked} onChange={(e) => setMixingOnly(e.target.checked)} />
            {t("lock.mixingOnly")}
          </label>
          <p id="lock-error" className="lock-error" role="alert" data-testid="lock-error">
            {model.message}
          </p>
          <DashButton
            type="submit"
            variant="filledWhiteBlue"
            size="large"
            fill
            disabled={model.working || passphrase.length === 0 || blocked}
            data-testid="lock-submit"
          >
            {model.working ? t("lock.unlocking") : t("lock.unlock")}
          </DashButton>
        </form>
        {model.canRecover && (
          <DashButton variant="plainWhite" data-testid="lock-forgot" onClick={onForgot}>
            {t("lock.forgot")}
          </DashButton>
        )}
        <LockReportNotice />
      </div>
    </div>
  );
}

/** The full unlock from mixing only (UX-SPEC §4.4 `UnlockSheet`, U-02 D12). */
export function UnlockSheet({ onClose }: { onClose: () => void }) {
  const t = useT();
  const model = useLockModel(useBackend());
  return (
    <Sheet title={t("lock.title")} width="small" testId="unlock-sheet" onClose={onClose}>
      <p className="sheet-text">{t("lock.prompt")}</p>
      <PassphrasePrompt
        testId="unlock-sheet"
        error={model.message}
        busy={model.working}
        disabled={model.blocked}
        submitLabel={t("lock.unlock")}
        onSubmit={async (p) => {
          if (await model.unlock(p, false)) onClose();
        }}
      />
    </Sheet>
  );
}
