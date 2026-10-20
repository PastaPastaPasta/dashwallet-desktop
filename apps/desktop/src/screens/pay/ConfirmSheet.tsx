// The confirm sheet (UX-SPEC §4.7 C23, iOS ConfirmPaymentSheet): what leaves the wallet, to
// whom, the fee and the coins used; the passphrase when the payment policy asks (U-03 D3).
// Send: a `spend` grant capped at the confirmed total, the signature, the check that the signed
// transaction keeps the sheet's promise, then the broadcast (U-03 D2). Create Unsigned makes the
// draft a PSBT instead, copies it and opens Tools ▸ PSBT with it; a watch-only wallet can only do
// that (U-05 D18).
// To a DashPay contact (DP3-06 D9–D12): "Pay to" names the contact, there is no Create Unsigned,
// and "Accept and pay" accepts the contact's request and pays it under one credential and one
// lease, ended once the first broadcast returned.

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
import { useDisplay } from "../../format/display";
import { Avatar } from "../contacts/Avatar";
import { useActionToast } from "../contacts/afterAction";
import { AcceptFailed, acceptFailedText, acceptUnderLease, acceptedText, type Payee } from "./contact";
import {
  authRefusal,
  broadcastFailure,
  credentialPrompt,
  failureText,
  isContact,
  keepsPromise,
  subtractsFee,
  unsignedKeepsPromise,
  type Advanced,
  type Confirmed,
  type Outcome,
} from "./model";
import type { DraftApi, DraftSpec } from "./useDraft";

const USING: Record<Advanced["coins"], string> = {
  any: "Any available funds",
  mixed: "Fully mixed funds only",
  selected: "Selected coins",
};

/** The contact a payment pays (DP3-06): who it is, and whether Send accepts their request too. */
export interface ConfirmContact {
  identity: string;
  payee: Payee;
  acceptsToo: boolean;
  /** The accept went through (D11): the form pays without it from now on. */
  onAccepted: () => void;
}

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
  contact,
}: {
  contact?: ConfirmContact;
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
  const shown = useDisplay(network);
  const { chain, amountUnit } = shown;
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
        if (live) setError(failureText(e, chain, amountUnit));
      },
    );
    return () => {
      live = false;
    };
  }, [b, chain, amountUnit]);
  const waitSecs = retryAt === null ? 0 : Math.max(1, Math.ceil((retryAt - Date.now()) / 1000));
  const fieldId = useId();
  const unit = shown.name;
  const { recipient, estimate, changeAddress, total } = confirmed;
  const toast = useActionToast(walletId);

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

  // Set before the first await, so a second Send in the same frame (before `busy` renders) does
  // not run the accept, which cannot be undone, twice.
  const inFlight = useRef(false);
  const send = async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    try {
      await sendOnce();
    } finally {
      inFlight.current = false;
    }
  };
  const sendOnce = async () => {
    if (busy || !policyKnown || !waitKnown || retryAt !== null) return;
    if (prompt === "passphrase" && passphrase === "") {
      setError("Enter your passphrase to send this payment.");
      return;
    }
    setBusy(true);
    setError(null);
    const credential: CredentialSpec = prompt === "passphrase" ? { kind: "passphrase", passphrase } : { kind: "unencrypted" };
    setPassphrase("");
    const refused = (e: unknown) => {
      setBusy(false);
      const refusal = isAppError(e) ? authRefusal(e) : { kind: "other" as const };
      if (refusal.kind === "needsPassphrase") {
        setAskAnyway(true);
        setError("Enter your passphrase to send this payment.");
      } else if (refusal.kind === "wrongPassphrase") {
        setError(refusal.text);
        // The vault refuses every attempt until then: Send waits too (Sol r1).
        if (refusal.waitSecs > 0) setRetryAt(Date.now() + refusal.waitSecs * 1000);
      } else onOutcome({ kind: "failed", text: failureText(e, chain, amountUnit) });
    };

    // Accept and pay (DP3-06 D10): the accept runs first, under the lease the prepare takes.
    let grant: AuthGrant | null = null;
    let lease: string | null = null;
    // An accept that went through stays so whatever the payment does (E0-04 C8, D12).
    let accepted: string | null = null;
    /** The grant, or the lease, the prepare takes. */
    let authId: string;
    if (contact?.acceptsToo) {
      try {
        lease = await acceptUnderLease(b, { walletId, identity: contact.identity, from: contact.payee.contact, spendCap: total, credential });
      } catch (e) {
        if (e instanceof AcceptFailed && e.stage === "authorize") return refused(e.cause);
        const text = acceptFailedText(e instanceof AcceptFailed ? e.cause : e);
        toast({ kind: "accept", contact: contact.payee.contact, ok: false, text }, true);
        onOutcome({ kind: "failed", text });
        return;
      }
      authId = lease;
      contact.onAccepted();
      accepted = acceptedText(contact.payee.name);
      // Said where the user is if the sheet's view went meanwhile (DP2-09 D23).
      toast({ kind: "accept", contact: contact.payee.contact, ok: true, text: accepted }, true);
    } else {
      try {
        grant = await b.authorize({ kind: "spend", maxDuffs: total }, walletId, credential);
        authId = grant.id;
      } catch (e) {
        return refused(e);
      }
    }
    // Two sentences under the "Payment not sent" heading: the refusals that say "not sent" already
    // must not say it twice (Sol r1 #2).
    const notSent = (text: string) => onOutcome({ kind: "failed", text: accepted ? `${accepted} ${text}` : text });
    const endLease = () => lease !== null && void b.endFlow(lease).catch(() => null);

    let held;
    try {
      held = await draft.prepare(spec, authId);
    } catch (e) {
      if (grant) void b.revokeGrant(grant.id).catch(() => null);
      endLease();
      notSent(failureText(e, chain, amountUnit));
      return;
    }
    const { draft: d, prepared } = held;
    if (!keepsPromise(confirmed, prepared.summary)) {
      await draft.abandon(d, prepared.handle);
      endLease();
      notSent("The payment changed. Review it again.");
      return;
    }
    // An accept that went through is said with whatever outcome the payment gets, a retry's too.
    const sent = { amount: estimate.totalSent, fee: prepared.summary.fee, ...(accepted && { note: accepted }) };
    const toContact = isContact(spec.recipient);
    try {
      const { txid } = await draft.broadcast(d, prepared.handle);
      onOutcome({ kind: "sent", txid, ...sent });
    } catch (e) {
      // "Not sent" only when the engine released it, or held a contact payment back before its
      // hand-off (then it is abandoned here); nothing else is abandoned after a broadcast.
      const o = broadcastFailure(e, chain, { draft: d, prepared: prepared.handle, ...sent }, true, amountUnit, toContact);
      if (o.kind === "failed") {
        if (toContact) await draft.abandon(d, prepared.handle);
        notSent(o.text);
      } else onOutcome(accepted && o.kind === "unknown" ? { ...o, text: `${accepted} ${o.text}` } : o);
    } finally {
      // The lease ends with the first broadcast (dwcli's point): a retry needs none.
      endLease();
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
    // The engine planned again: the PSBT leaves the sheet only if it is the payment shown, and one
    // that cannot be checked is not shown to be (D37).
    const analysis = await b.analyzePsbt(walletId, psbt).catch(() => null);
    if (analysis === null || !unsignedKeepsPromise(confirmed, analysis)) {
      onOutcome({ kind: "failed", text: "The payment changed. Review it again." });
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
          <AmountText className="confirm-amount" text={shown.full(total)} unitName={unit} glyphFactor={0.8} />
          <p className="confirm-question">
            {contact?.acceptsToo
              ? `Accept ${contact.payee.name}'s contact request and send this payment?`
              : watchOnly
              ? "Please, review your transaction proposal. This will produce a Partially Signed Transaction (PSBT) which you can save or copy and then sign with e.g. an offline Dash Core wallet."
              : "Do you want to send this payment?"}
          </p>
        </div>
        <dl className="detail-list card-l1" data-testid="pay-confirm-facts">
          {isContact(recipient) ? (
            <DetailRow label="Pay to" testId="pay-confirm-contact">
              {contact && (
                <span className="pay-payee">
                  <Avatar letters={contact.payee.letters} identity={contact.payee.contact} />
                  <span className="pay-payee-names">
                    <span className="confirm-label">{contact.payee.name}</span>
                    {contact.payee.sub && <span className="contact-row-username">{contact.payee.sub}</span>}
                  </span>
                </span>
              )}
            </DetailRow>
          ) : (
            <DetailRow label="Pay to" testId="pay-confirm-address">
              {recipient.label && <span className="confirm-label">{recipient.label}</span>}
              <span className="address-full">{recipient.address}</span>
            </DetailRow>
          )}
          {contact?.acceptsToo && (
            <DetailRow label="Contact request" testId="pay-confirm-accept">
              Accepted with this payment
            </DetailRow>
          )}
          {isContact(recipient) && recipient.note && <DetailRow label="Note">{recipient.note}</DetailRow>}
          <DetailRow label="Amount" testId="pay-confirm-amount">
            <AmountText text={shown.full(estimate.totalSent)} unitName={unit} glyph={false} />
          </DetailRow>
          <DetailRow label="Network fee" testId="pay-confirm-fee">
            <AmountText text={shown.full(estimate.fee)} unitName={unit} glyph={false} />
          </DetailRow>
          <DetailRow label="Using">{coinsText(coins)}</DetailRow>
          {changeAddress !== null && (
            <DetailRow label="Change to" testId="pay-confirm-change">
              <span className="address-full">{changeAddress}</span>
              <AmountText text={shown.full(estimate.change ?? 0)} unitName={unit} glyph={false} />
            </DetailRow>
          )}
          <DetailRow label="Total" testId="pay-confirm-total">
            <AmountText text={shown.full(total)} unitName={unit} glyph={false} />
          </DetailRow>
        </dl>
        {subtractsFee(recipient) && <p className="placeholder-note">The fee is subtracted from the amount: the payee receives the amount above.</p>}
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
            This wallet could not be read, so it cannot send now.{contact ? "" : " Create Unsigned still works."}
          </p>
        )}
        <div className="sheet-actions sheet-actions-fit">
          <DashButton key="cancel" ref={cancel} variant="tintedGray" size="large" data-testid="pay-confirm-cancel" disabled={busy} onClick={onCancel}>
            Cancel
          </DashButton>
          {!contact && (
            <DashButton
              key="unsigned"
              variant={watchOnly ? "filledBlue" : "tintedBlue"}
              size="large"
              data-testid="pay-confirm-unsigned"
              disabled={busy}
              onClick={() => void createUnsigned()}
            >
              Create Unsigned
            </DashButton>
          )}
          {watchOnlyKnown === false && (
            <DashButton key="send" type="submit" variant="filledBlue" size="large" data-testid="pay-confirm-send" disabled={busy || !policyKnown || !waitKnown || retryAt !== null}>
              {busy ? "Sending…" : contact?.acceptsToo ? "Accept and pay" : "Send"}
            </DashButton>
          )}
        </div>
      </form>
    </Sheet>
  );
}
