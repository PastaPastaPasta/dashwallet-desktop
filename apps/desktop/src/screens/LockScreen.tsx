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
import { SIGNED_BEFORE_LOCK } from "../api/send";
import { DashButton, DashLogo, NetworkCapsule } from "../components/basics";
import { PassphrasePrompt } from "../components/passphrase";
import { Sheet } from "../components/sheet";
import { LockModel, LockText } from "../onboarding/lock";
import { useModel } from "../onboarding/model";

/** A `LockModel` that reads the vault's count and wait when it appears, and clears a wait when
 * it runs out. */
function useLockModel(backend: Backend): LockModel {
  const [model] = useState(() => new LockModel(backend));
  useModel(model);
  useEffect(() => void model.load(), [model]);
  const { retryAt } = model;
  useEffect(() => {
    if (retryAt === null) return;
    const t = window.setTimeout(() => model.refresh(), Math.max(0, retryAt - Date.now()) + 50);
    return () => window.clearTimeout(t);
  }, [model, retryAt]);
  return model;
}

const more = (n: number, noun: string) => n > 0 && `${n} more ${noun}${n === 1 ? "" : "s"}`;

/** What the last lock did to running flows (E0-13 D3, E0-04 §16.10 C4). Hidden while a newer
 * lock drains: until its `done`, the last report is the previous lock's. */
function LockReportNotice() {
  const drain = useLockDrain().data;
  const report = useLastLockReport().data;
  if (drain)
    return (
      <section className="lock-report" data-testid="lock-drain">
        <p className="lock-report-title" role="status">
          Stopping {drain.inFlight === 1 ? "1 running action" : `${drain.inFlight} running actions`}…
        </p>
      </section>
    );
  return report && reportsAnything(report) ? <LockReportBody report={report} /> : null;
}

/** Unresolved while anything listed may still be sent, or anything left out cannot be shown not
 * to be; `leases`, `dispatchStatus` and `lockStatus` settle only what the report lets them. */
function LockReportBody({ report }: { report: LockOutcome }) {
  const summary = summarizeLockReport(report);
  const reconciled = useLockReconcile(report, summary.unresolved);
  const resolved = !summary.unresolved || reconciled.data?.settled === true;
  const { flows, artifacts, wallets } = summary.omitted;
  const check = resolved
    ? null
    : summary.beyondReach
      ? "The app cannot check all of these actions, so some may still be sent."
      : reconciled.isError
        ? "Could not check whether any of these actions may still be sent."
        : "Checking whether any of these actions may still be sent…";
  return (
    <section
      className="lock-report"
      data-testid="lock-report"
      data-resolved={resolved}
      data-checked-at={reconciled.data?.checkedAt}
      aria-labelledby="lock-report-title"
    >
      <p id="lock-report-title" className="lock-report-title" role="status">
        {resolved ? "Every action the lock stopped has ended." : SIGNED_BEFORE_LOCK}
      </p>
      {check && (
        <p className="lock-report-note" data-testid="lock-report-check">
          {check}
        </p>
      )}
      <ul className="lock-report-flows" aria-label="At the time of the lock">
        {report.flows.map((f) => (
          <li key={f.lease} data-testid="lock-report-flow" data-outcome={f.outcome}>
            {FLOW_NAME[f.flow]}: {OUTCOME_LINE[f.outcome]}
          </li>
        ))}
      </ul>
      {!summary.complete && (
        <p className="lock-report-note" data-testid="lock-report-omitted">
          {[more(flows, "action"), more(artifacts, "step"), more(wallets, "wallet")].filter(Boolean).join(", ")} not listed here.
        </p>
      )}
    </section>
  );
}

export function LockScreen({ network, onForgot }: { network: NetworkInfo; onForgot: () => void }) {
  const model = useLockModel(useBackend());
  const [passphrase, setPassphrase] = useState("");
  const [mixingOnly, setMixingOnly] = useState(false);
  // No attempt is taken during a wait, nor after eight failures.
  const blocked = model.disabled || model.retryAt !== null;
  const field = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (!blocked) field.current?.focus();
  }, [blocked]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!passphrase || model.working || blocked) return;
    // The DOM copy of the passphrase is cleared after every attempt (H-01 note in the G-01 report).
    const typed = passphrase;
    setPassphrase("");
    await model.unlock(typed, mixingOnly);
  };

  return (
    <div className="lock-screen" role="dialog" aria-modal="true" aria-labelledby="lock-title" data-testid="lock-screen">
      <div className="lock-column" data-testid="lock-column">
        <DashLogo className="lock-logo" />
        <NetworkCapsule network={network} />
        <h1 id="lock-title" className="lock-title">
          {LockText.title}
        </h1>
        <p className="lock-prompt">{LockText.prompt}</p>
        <form className="lock-form" onSubmit={submit} data-testid="lock-form">
          <input
            ref={field}
            className="lock-field"
            data-testid="lock-field"
            type="password"
            aria-label="Passphrase"
            placeholder="Passphrase"
            autoComplete="current-password"
            autoFocus
            value={passphrase}
            disabled={blocked}
            aria-invalid={model.message !== null}
            aria-describedby={model.message ? "lock-error" : undefined}
            onChange={(e) => setPassphrase(e.target.value)}
          />
          <label className="lock-check">
            <input type="checkbox" data-testid="lock-mixing-only" checked={mixingOnly} disabled={blocked} onChange={(e) => setMixingOnly(e.target.checked)} />
            For mixing only
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
            {model.working ? "Unlocking…" : "Unlock"}
          </DashButton>
        </form>
        {model.canRecover && (
          <DashButton variant="plainWhite" data-testid="lock-forgot" onClick={onForgot}>
            Forgot passphrase?
          </DashButton>
        )}
        <LockReportNotice />
      </div>
    </div>
  );
}

/** The full unlock from mixing only (UX-SPEC §4.4 `UnlockSheet`, U-02 D12). */
export function UnlockSheet({ onClose }: { onClose: () => void }) {
  const model = useLockModel(useBackend());
  return (
    <Sheet title={LockText.title} width="small" testId="unlock-sheet" onClose={onClose}>
      <p className="sheet-text">{LockText.prompt}</p>
      <PassphrasePrompt
        testId="unlock-sheet"
        error={model.message}
        busy={model.working}
        disabled={model.disabled || model.retryAt !== null}
        submitLabel="Unlock"
        onSubmit={async (p) => {
          if (await model.unlock(p, false)) onClose();
        }}
      />
    </Sheet>
  );
}
