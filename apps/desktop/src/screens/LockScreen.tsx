// Lock screen (UX-SPEC §4.4): full-window overlay on hero blue with the white wordmark and the
// dash-qt passphrase prompt. The vault's own throttle (retryAfterSecs) gates the next attempt.

import { useEffect, useState, type FormEvent } from "react";
import { errorMessage, hasCode, retryAfterSecs } from "../api/backend";
import type { LockOutcome } from "../api/generated/LockOutcome";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { FLOW_NAME, OUTCOME_LINE, reportsAnything, summarizeLockReport } from "../api/lockReport";
import { useBackend, useLastLockReport, useLockDrain, useLockReconcile } from "../api/queries";
import { SIGNED_BEFORE_LOCK } from "../api/send";
import { DashButton, DashLogo, NetworkCapsule } from "../components/basics";

const Lock = {
  title: "Unlock wallet",
  prompt: "This operation needs your wallet passphrase to unlock the wallet.",
  /** dash-qt `AskPassphraseDialog`. */
  incorrect: "The passphrase entered for the wallet decryption was incorrect.",
  tryAgainIn: (wait: string) => `Wallet disabled, please try again in ${wait}`,
};

function waitText(secs: number): string {
  if (secs >= 3600) {
    const h = Math.ceil(secs / 3600);
    return h === 1 ? "1 hour" : `${h} hours`;
  }
  const m = Math.max(1, Math.ceil(secs / 60));
  return m === 1 ? "1 minute" : `${m} minutes`;
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

export function LockScreen({ network }: { network: NetworkInfo }) {
  const backend = useBackend();
  const [passphrase, setPassphrase] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /** When the vault accepts the next attempt (ms since epoch); null when not throttled. */
  const [retryAt, setRetryAt] = useState<number | null>(null);

  useEffect(() => {
    if (retryAt === null) return;
    const t = window.setTimeout(() => (setRetryAt(null), setError(null)), Math.max(0, retryAt - Date.now()));
    return () => window.clearTimeout(t);
  }, [retryAt]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!passphrase || busy || retryAt !== null) return;
    setBusy(true);
    setError(null);
    try {
      await backend.unlock(passphrase);
    } catch (err) {
      const wait = retryAfterSecs(err);
      if (wait > 0) {
        // Wrong passphrase with a wait, or throttled: the vault refuses attempts until then.
        setRetryAt(Date.now() + wait * 1000);
        setError(`${hasCode(err, "vault.wrong_passphrase") ? `${Lock.incorrect} ` : ""}${Lock.tryAgainIn(waitText(wait))}`);
      } else if (hasCode(err, "vault.wrong_passphrase")) {
        setError(Lock.incorrect);
      } else {
        setError(errorMessage(err));
      }
    } finally {
      // The DOM copy of the passphrase is cleared after every attempt (H-01 note in the G-01 report).
      setPassphrase("");
      setBusy(false);
    }
  };

  return (
    <div className="lock-screen" role="dialog" aria-modal="true" aria-labelledby="lock-title" data-testid="lock-screen">
      <div className="lock-column" data-testid="lock-column">
        <DashLogo className="lock-logo" />
        <NetworkCapsule network={network} />
        <h1 id="lock-title" className="lock-title">
          {Lock.title}
        </h1>
        <p className="lock-prompt">{Lock.prompt}</p>
        <form className="lock-form" onSubmit={submit} data-testid="lock-form">
          <input
            className="lock-field"
            data-testid="lock-field"
            type="password"
            aria-label="Passphrase"
            placeholder="Passphrase"
            autoComplete="current-password"
            autoFocus
            value={passphrase}
            aria-invalid={error !== null}
            aria-describedby={error ? "lock-error" : undefined}
            onChange={(e) => setPassphrase(e.target.value)}
          />
          <p id="lock-error" className="lock-error" role="alert">
            {error}
          </p>
          <DashButton
            type="submit"
            variant="filledWhiteBlue"
            size="large"
            fill
            disabled={busy || passphrase.length === 0 || retryAt !== null}
            data-testid="lock-submit"
          >
            {busy ? "Unlocking…" : "Unlock"}
          </DashButton>
        </form>
        <LockReportNotice />
      </div>
    </div>
  );
}
