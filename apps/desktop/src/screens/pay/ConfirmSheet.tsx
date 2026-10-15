// The confirm sheet (UX-SPEC §4.7 C23, iOS ConfirmPaymentSheet): what leaves the wallet, to
// whom, the fee and the coins used; the passphrase when the payment policy asks (U-03 D3).
// Send: a `spend` grant capped at the confirmed total, the signature, the check that the signed
// transaction keeps the sheet's promise, then the broadcast (U-03 D2). Create Unsigned makes the
// draft a PSBT instead, copies it and opens Tools ▸ PSBT with it; a watch-only wallet can only do
// that (U-05 D18).

import { useEffect, useId, useRef, useState } from "react";
import { isAppError } from "../../api/backend";
import type { AuthGrant } from "../../api/generated/AuthGrant";
import type { CredentialSpec } from "../../api/generated/CredentialSpec";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { useBackend, useLockStatus, usePaymentAuthentication, useWallets } from "../../api/queries";
import { navigate } from "../../app/ui";
import { AmountText, DashButton } from "../../components/basics";
import { DetailRow } from "../../components/detail";
import { LoadingState } from "../../components/history";
import { Sheet } from "../../components/sheet";
import { full } from "../../format/amount";
import { authRefusal, broadcastFailure, credentialPrompt, failureText, keepsPromise, type Advanced, type Confirmed, type Outcome } from "./model";
import { chainOf } from "./PayToAddress";
import type { DraftApi, DraftSpec } from "./useDraft";

const USING: Record<Advanced["coins"], string> = {
  any: "Any available funds",
  mixed: "Fully mixed funds only",
  selected: "Selected coins",
};

function coinsText(coins: Advanced): string {
  return coins.coins === "selected" ? `${coins.selected.length} selected ${coins.selected.length === 1 ? "coin" : "coins"}` : USING[coins.coins];
}

export function ConfirmSheet({
  confirmed,
  spec,
  coins,
  walletId,
  network,
  draft,
  returnFocus,
  onCancel,
  onOutcome,
}: {
  confirmed: Confirmed;
  spec: DraftSpec;
  coins: Advanced;
  walletId: string;
  network: NetworkInfo;
  draft: DraftApi;
  returnFocus: () => HTMLElement | null;
  onCancel: () => void;
  onOutcome: (o: Outcome) => void;
}) {
  const b = useBackend();
  const chain = chainOf(network);
  const lock = useLockStatus();
  const policy = usePaymentAuthentication();
  // `undefined` until the wallet list is read: neither the prompt nor Send shows before it is
  // known whether the wallet can sign.
  const wallets = useWallets();
  const watchOnlyKnown = wallets.data?.find((w) => w.walletId === walletId)?.watchOnly;
  // Read and not there, or not readable: Send stays hidden, and the sheet says why.
  const walletUnknown = watchOnlyKnown === undefined && (wallets.isError || wallets.isSuccess);
  const watchOnly = watchOnlyKnown ?? false;
  const [askAnyway, setAskAnyway] = useState(false);
  const [passphrase, setPassphrase] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /** When the vault takes the next attempt (ms since epoch), as on the lock screen; null: now. */
  const [retryAt, setRetryAt] = useState<number | null>(null);
  const [, tick] = useState(0);
  useEffect(() => {
    if (retryAt === null) return;
    const t = window.setInterval(() => (Date.now() >= retryAt ? setRetryAt(null) : tick((n) => n + 1)), 250);
    return () => window.clearInterval(t);
  }, [retryAt]);
  // A wait left by earlier attempts (a cancelled sheet, the lock screen) holds Send from the start:
  // Send waits for this read, and stays off if it fails (Sol r2).
  const [waitKnown, setWaitKnown] = useState(false);
  useEffect(() => {
    let live = true;
    b.lockStatus().then(
      ({ retryAfterSecs: secs }) => {
        if (!live) return;
        if (secs) setRetryAt((at) => at ?? Date.now() + secs * 1000);
        setWaitKnown(true);
      },
      (e) => {
        if (live) setError(failureText(e, chain));
      },
    );
    return () => {
      live = false;
    };
  }, [b, chain]);
  const waitSecs = retryAt === null ? 0 : Math.max(1, Math.ceil((retryAt - Date.now()) / 1000));
  const fieldId = useId();
  const unit = network.unitName;
  const { recipient, estimate, changeAddress, total } = confirmed;

  // A failed read of the policy counts as "on" (U-03 D3): the prompt waits only while it loads.
  const policyKnown = !policy.isPending;
  const prompt = watchOnlyKnown !== false ? "none" : askAnyway ? "passphrase" : credentialPrompt(lock.data, policy.isSuccess ? policy.data : undefined);
  // The policy loads after the sheet opened: once it asks for nothing, Cancel takes the focus,
  // so Return does not send (U-03 D4). The passphrase field focuses itself when it appears.
  const cancel = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (policyKnown && prompt === "none") cancel.current?.focus();
  }, [policyKnown, prompt]);
  // The field was disabled while `authorize` ran: after a refusal, focus goes back to it.
  const field = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (!busy && error) field.current?.focus();
  }, [busy, error]);

  const send = async () => {
    if (busy || !policyKnown || !waitKnown || retryAt !== null) return;
    if (prompt === "passphrase" && passphrase === "") {
      setError("Enter your passphrase to send this payment.");
      return;
    }
    setBusy(true);
    setError(null);
    const credential: CredentialSpec = prompt === "passphrase" ? { kind: "passphrase", passphrase } : { kind: "unencrypted" };
    setPassphrase("");
    let grant: AuthGrant;
    try {
      grant = await b.authorize({ kind: "spend", maxDuffs: total }, walletId, credential);
    } catch (e) {
      setBusy(false);
      const refusal = isAppError(e) ? authRefusal(e) : { kind: "other" as const };
      if (refusal.kind === "needsPassphrase") {
        setAskAnyway(true);
        setError("Enter your passphrase to send this payment.");
      } else if (refusal.kind === "wrongPassphrase") {
        setError(refusal.text);
        // The vault refuses every attempt until then: Send waits too (Sol r1).
        if (refusal.waitSecs > 0) setRetryAt(Date.now() + refusal.waitSecs * 1000);
      } else onOutcome({ kind: "failed", text: failureText(e, chain) });
      return;
    }

    let held;
    try {
      held = await draft.prepare(spec, grant.id);
    } catch (e) {
      void b.revokeGrant(grant.id).catch(() => null);
      onOutcome({ kind: "failed", text: failureText(e, chain) });
      return;
    }
    const { draft: d, prepared } = held;
    if (!keepsPromise(confirmed, prepared.summary)) {
      await draft.abandon(d, prepared.handle);
      onOutcome({ kind: "failed", text: "The payment changed. Review it again." });
      return;
    }
    const sent = { amount: estimate.totalSent, fee: prepared.summary.fee };
    try {
      const { txid } = await draft.broadcast(d, prepared.handle);
      onOutcome({ kind: "sent", txid, ...sent });
    } catch (e) {
      // "Not sent" only when the engine released it; nothing is abandoned after a broadcast.
      onOutcome(broadcastFailure(e, chain, { draft: d, prepared: prepared.handle, ...sent }, true));
    }
  };

  const createUnsigned = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    let psbt: string;
    try {
      psbt = await draft.createUnsigned(spec);
    } catch (e) {
      setBusy(false);
      setError(failureText(e, chain));
      return;
    }
    const copied = await navigator.clipboard.writeText(psbt).then(
      () => true,
      () => false,
    );
    navigate("tools", { toolsTab: "psbt", psbtPrefill: { psbt, copied } });
  };

  return (
    <Sheet title="Confirm payment" width="small" testId="pay-confirm" returnFocus={returnFocus} onClose={() => !busy && onCancel()}>
      <form
        className="confirm"
        onSubmit={(e) => {
          e.preventDefault();
          if (watchOnlyKnown === false) void send();
        }}
      >
        <div className="confirm-summary">
          <AmountText className="confirm-amount" text={full(total, false)} unitName={unit} glyphFactor={0.8} />
          <p className="confirm-question">
            {watchOnly
              ? "Please, review your transaction proposal. This will produce a Partially Signed Transaction (PSBT) which you can save or copy and then sign with e.g. an offline Dash Core wallet."
              : "Do you want to send this payment?"}
          </p>
        </div>
        <dl className="detail-list card-l1" data-testid="pay-confirm-facts">
          <DetailRow label="Pay to" testId="pay-confirm-address">
            {recipient.label && <span className="confirm-label">{recipient.label}</span>}
            <span className="address-full">{recipient.address}</span>
          </DetailRow>
          <DetailRow label="Amount" testId="pay-confirm-amount">
            <AmountText text={full(estimate.totalSent, false)} unitName={unit} glyph={false} />
          </DetailRow>
          <DetailRow label="Network fee" testId="pay-confirm-fee">
            <AmountText text={full(estimate.fee, false)} unitName={unit} glyph={false} />
          </DetailRow>
          <DetailRow label="Using">{coinsText(coins)}</DetailRow>
          {changeAddress !== null && (
            <DetailRow label="Change to" testId="pay-confirm-change">
              <span className="address-full">{changeAddress}</span>
              <AmountText text={full(estimate.change ?? 0, false)} unitName={unit} glyph={false} />
            </DetailRow>
          )}
          <DetailRow label="Total" testId="pay-confirm-total">
            <AmountText text={full(total, false)} unitName={unit} glyph={false} />
          </DetailRow>
        </dl>
        {recipient.subtractFeeFromAmount && <p className="placeholder-note">The fee is subtracted from the amount: the payee receives the amount above.</p>}
        {changeAddress !== null && <p className="placeholder-note">The total includes the change: the custom change address may not belong to this wallet.</p>}
        {!policyKnown ? (
          <LoadingState testId="pay-confirm-policy" text="Checking the payment settings" />
        ) : (
          prompt === "passphrase" && (
            <div className="form-field">
              <label htmlFor={fieldId}>Passphrase</label>
              <input
                ref={field}
                id={fieldId}
                type="password"
                data-testid="pay-confirm-passphrase"
                autoComplete="current-password"
                value={passphrase}
                aria-invalid={!!error}
                aria-describedby={[error && `${fieldId}-error`, waitSecs > 0 && `${fieldId}-wait`].filter(Boolean).join(" ") || undefined}
                // Mounted once the policy is known, or when the engine asks for it: focus it then.
                autoFocus
                disabled={busy}
                onChange={(e) => setPassphrase(e.target.value)}
              />
            </div>
          )
        )}
        <p id={`${fieldId}-error`} className="form-problem" role="alert" data-testid="pay-confirm-error">
          {error}
        </p>
        {/* Outside the alert: a screen reader announces the refusal once, not every second. */}
        {waitSecs > 0 && (
          <p id={`${fieldId}-wait`} className="form-problem" data-testid="pay-confirm-wait">
            {error ? "" : "Too many attempts. "}Try again in {waitSecs} s.
          </p>
        )}
        {walletUnknown && (
          <p className="placeholder-note" role="status" data-testid="pay-confirm-wallet-unknown">
            This wallet could not be read, so it cannot send now. Create Unsigned still works.
          </p>
        )}
        <div className="sheet-actions sheet-actions-fit">
          <DashButton ref={cancel} variant="tintedGray" size="large" data-testid="pay-confirm-cancel" disabled={busy} onClick={onCancel}>
            Cancel
          </DashButton>
          <DashButton
            variant={watchOnly ? "filledBlue" : "tintedBlue"}
            size="large"
            data-testid="pay-confirm-unsigned"
            disabled={busy}
            onClick={() => void createUnsigned()}
          >
            Create Unsigned
          </DashButton>
          {watchOnlyKnown === false && (
            <DashButton type="submit" variant="filledBlue" size="large" data-testid="pay-confirm-send" disabled={busy || !policyKnown || !waitKnown || retryAt !== null}>
              {busy ? "Sending…" : "Send"}
            </DashButton>
          )}
        </div>
      </form>
    </Sheet>
  );
}
