// Lock screen (UX-SPEC §4.4): full-window overlay on hero blue with the white wordmark and the
// dash-qt passphrase prompt. The vault's own throttle (retryAfterSecs) gates the next attempt.

import { useEffect, useState, type FormEvent } from "react";
import { errorMessage, isAppError } from "../api/backend";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { useBackend } from "../api/queries";
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
      const wait = isAppError(err) ? err.retryAfterSecs ?? 0 : 0;
      if (wait > 0) {
        // Wrong passphrase with a wait, or throttled: the vault refuses attempts until then.
        setRetryAt(Date.now() + wait * 1000);
        setError(`${isAppError(err) && err.code === "vault.wrong_passphrase" ? `${Lock.incorrect} ` : ""}${Lock.tryAgainIn(waitText(wait))}`);
      } else if (isAppError(err) && err.code === "vault.wrong_passphrase") {
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
      </div>
    </div>
  );
}
