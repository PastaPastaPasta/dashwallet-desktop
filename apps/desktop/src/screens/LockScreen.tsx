// Lock screen (UX-SPEC §4.4): full-window overlay on hero blue with the white wordmark, the
// dash-qt passphrase prompt and the iOS lockout copy (IOS-012) as UX throttling.

import { useState, type CSSProperties, type FormEvent } from "react";
import { isBackendError } from "../api/backend";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { useBackend } from "../api/queries";
import { DashButton } from "../components/basics";
import { iconFiles } from "../components/icons";

export const Lock = {
  title: "Unlock wallet",
  prompt: "This operation needs your wallet passphrase to unlock the wallet.",
  attemptsRemaining: (n: number) => (n === 1 ? "1 attempt remaining" : `${n} attempts remaining`),
  tryAgainIn: (wait: string) => `Wallet disabled, please try again in ${wait}`,
};

/** IOS-012 as used by the Swift LockViewModel: eight attempts before only restore is offered. */
const DISABLED_AFTER = 8;

function waitText(secs: number): string {
  if (secs >= 3600) {
    const h = Math.ceil(secs / 3600);
    return h === 1 ? "1 hour" : `${h} hours`;
  }
  const m = Math.max(1, Math.ceil(secs / 60));
  return m === 1 ? "1 minute" : `${m} minutes`;
}

export function LockScreen({ network }: { network: NetworkInfo }) {
  const backend = useBackend();
  const [passphrase, setPassphrase] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!passphrase || busy) return;
    setBusy(true);
    setError(null);
    try {
      await backend.unlock(passphrase);
      setPassphrase("");
    } catch (err) {
      if (isBackendError(err) && err.code === "vault.throttled" && err.retryAfterSecs) {
        setError(Lock.tryAgainIn(waitText(err.retryAfterSecs)));
      } else if (isBackendError(err) && err.code === "vault.wrong_passphrase") {
        const status = await backend.lockStatus();
        setError(`The passphrase entered for the wallet decryption was incorrect. ${Lock.attemptsRemaining(Math.max(0, DISABLED_AFTER - status.failedAttempts))}`);
      } else {
        setError(isBackendError(err) ? err.message : String(err));
      }
      setPassphrase("");
    } finally {
      setBusy(false);
    }
  };

  const logo = { "--logo-url": `url("${iconFiles.dashLogo.light}")` } as CSSProperties;
  return (
    <div className="lock-screen" role="dialog" aria-modal="true" aria-labelledby="lock-title" data-testid="lock-screen">
      <div className="lock-column" data-testid="lock-column">
        <span className="lock-logo" style={logo} role="img" aria-label="Dash" data-testid="lock-logo" />
        {network.kind !== "mainnet" && <span className="network-capsule">{network.kind.toUpperCase()}</span>}
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
          <DashButton type="submit" variant="filledWhiteBlue" size="large" fill disabled={busy || passphrase.length === 0} data-testid="lock-submit">
            {busy ? "Unlocking…" : "Unlock"}
          </DashButton>
        </form>
      </div>
    </div>
  );
}
