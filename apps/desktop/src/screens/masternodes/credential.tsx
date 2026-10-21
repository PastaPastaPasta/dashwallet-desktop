// The vote and create sheets' passphrase (D12, D14): Pay's confirm-sheet rules (U-03 D3–D4) for
// a governance or spend grant. The prompt follows the lock and "Require authentication for every
// payment" (a failed read counts as on); a vault that asks anyway shows the field; a wrong
// passphrase says so and holds the button for the vault's wait.

import { useEffect, useId, useRef, useState } from "react";
import { isAppError } from "../../api/backend";
import type { AuthGrant } from "../../api/generated/AuthGrant";
import type { CredentialSpec } from "../../api/generated/CredentialSpec";
import type { GrantPurposeSpec } from "../../api/generated/GrantPurposeSpec";
import { useBackend, useLockStatus, usePaymentAuthentication } from "../../api/queries";
import { authRefusal, credentialPrompt } from "../pay/model";

export interface Credential {
  /** The policy and the vault's wait are read: the confirming button may act. */
  ready: boolean;
  prompt: "passphrase" | "none";
  passphrase: string;
  setPassphrase: (p: string) => void;
  error: string | null;
  setError: (e: string | null) => void;
  waitSecs: number;
  /** A grant for `purpose`, or `null` after a refusal it showed (the field asks again). Throws
   * what is not the vault's credential refusal. */
  authorize: (purpose: GrantPurposeSpec) => Promise<AuthGrant | null>;
}

export function useCredential(walletId: string, ask: string): Credential {
  const b = useBackend();
  const lock = useLockStatus();
  const policy = usePaymentAuthentication();
  const [askAnyway, setAskAnyway] = useState(false);
  const [passphrase, setPassphrase] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [retryAt, setRetryAt] = useState<number | null>(null);
  const [waitKnown, setWaitKnown] = useState(false);
  const [, tick] = useState(0);
  useEffect(() => {
    if (retryAt === null) return;
    const t = window.setInterval(() => (Date.now() >= retryAt ? setRetryAt(null) : tick((n) => n + 1)), 250);
    return () => window.clearInterval(t);
  }, [retryAt]);
  // A wait left by earlier attempts holds the button from the start (Pay's rule).
  useEffect(() => {
    let live = true;
    b.lockStatus().then(
      ({ retryAfterSecs: secs }) => {
        if (!live) return;
        if (secs) setRetryAt((at) => at ?? Date.now() + secs * 1000);
        setWaitKnown(true);
      },
      () => live && setWaitKnown(true),
    );
    return () => {
      live = false;
    };
  }, [b]);
  const prompt = askAnyway ? "passphrase" : credentialPrompt(lock.data, policy.isSuccess ? policy.data : undefined);
  const waitSecs = retryAt === null ? 0 : Math.max(1, Math.ceil((retryAt - Date.now()) / 1000));

  const authorize = async (purpose: GrantPurposeSpec): Promise<AuthGrant | null> => {
    if (prompt === "passphrase" && passphrase === "") {
      setError(ask);
      return null;
    }
    const credential: CredentialSpec = prompt === "passphrase" ? { kind: "passphrase", passphrase } : { kind: "unencrypted" };
    setPassphrase("");
    setError(null);
    try {
      return await b.authorize(purpose, walletId, credential);
    } catch (e) {
      const refusal = isAppError(e) ? authRefusal(e) : { kind: "other" as const };
      if (refusal.kind === "needsPassphrase") {
        setAskAnyway(true);
        setError(ask);
        return null;
      }
      if (refusal.kind === "wrongPassphrase") {
        setError(refusal.text);
        if (refusal.waitSecs > 0) setRetryAt(Date.now() + refusal.waitSecs * 1000);
        return null;
      }
      throw e;
    }
  };

  return {
    ready: !policy.isPending && waitKnown && retryAt === null,
    prompt,
    passphrase,
    setPassphrase,
    error,
    setError,
    waitSecs,
    authorize,
  };
}

/** The passphrase field (when asked), the refusal and the wait. */
export function CredentialField({ c, busy, testId, className }: { c: Credential; busy: boolean; testId: string; className?: string }) {
  const id = useId();
  const field = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (!busy && c.error) field.current?.focus();
  }, [busy, c.error]);
  return (
    <div className={className}>
      {c.prompt === "passphrase" && (
        <div className="form-field">
          <label htmlFor={id}>Passphrase</label>
          <input
            ref={field}
            id={id}
            type="password"
            data-testid={`${testId}-passphrase`}
            autoComplete="current-password"
            value={c.passphrase}
            aria-invalid={!!c.error}
            aria-describedby={[c.error && `${id}-error`, c.waitSecs > 0 && `${id}-wait`].filter(Boolean).join(" ") || undefined}
            disabled={busy}
            onChange={(e) => c.setPassphrase(e.target.value)}
          />
        </div>
      )}
      <p id={`${id}-error`} className="form-problem" role="alert" data-testid={`${testId}-error`}>
        {c.error}
      </p>
      {c.waitSecs > 0 && (
        <p id={`${id}-wait`} className="form-problem" data-testid={`${testId}-wait`}>
          {c.error ? "" : "Too many attempts. "}Try again in {c.waitSecs} s.
        </p>
      )}
    </div>
  );
}
