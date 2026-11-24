// The confirm sheet (UX-SPEC §4.7 C23, iOS ConfirmPaymentSheet): what leaves the wallet, to
// whom, the fee and the coins used; the passphrase when the payment policy asks (U-03 D3).
// Send: a `spend` grant capped at the confirmed total, the signature, the check that the signed
// transaction keeps the sheet's promise, then the broadcast (U-03 D2). Create Unsigned makes the
// draft a PSBT instead, copies it and opens Tools ▸ PSBT with it; a watch-only wallet can only do
// that (U-05 D18), and so does Advanced's unsigned path (P15), which opens this sheet as a review
// of the PSBT only.
// To a DashPay contact (DP3-06 D9–D12): "Pay to" names the contact, there is no Create Unsigned,
// and "Accept and pay" accepts the contact's request and pays it under one credential and one
// lease, ended once the first broadcast returned.

import { useQuery } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState } from "react";
import { hasCode, isAppError } from "../../api/backend";
import { refusedWhileLocked } from "../../api/lockGate";
import type { AuthGrant } from "../../api/generated/AuthGrant";
import type { CredentialSpec } from "../../api/generated/CredentialSpec";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { keys, useBackend, useLockStatus, usePaymentAuthentication, useQuickUnlockPolicy, useUtxos, useWallets } from "../../api/queries";
import { navigate, useUi } from "../../app/ui";
import { AmountText, DashButton } from "../../components/basics";
import { DetailRow } from "../../components/detail";
import { LoadingState } from "../../components/history";
import { Sheet } from "../../components/sheet";
import { FiatLine, useFiat } from "../../fiat/rates";
import { useDisplay } from "../../format/display";
import { feeWarningText, payFeeWarning } from "../../format/feeWarning";
import { messageTable, t as tr, useT } from "../../i18n";
import { Avatar } from "../contacts/Avatar";
import { useActionToast } from "../contacts/afterAction";
import { AcceptFailed, acceptFailedText, acceptUnderLease, acceptedText, type Payee } from "./contact";
import {
  acceptAndPayValue,
  authRefusal,
  broadcastFailure,
  confirmActions,
  contactConfirmActions,
  credentialPrompt,
  failureText,
  isContact,
  keepsPromise,
  mixedSendNotes,
  paysWithUnmixed,
  receivedAmounts,
  settleNotSent,
  subtractsFee,
  UNRECORDED,
  unsignedKeepsPromise,
  type Advanced,
  type Confirmed,
  type Outcome,
  type Sending,
} from "./model";
import type { PayAttempt } from "./form";
import type { DraftApi, DraftSpec } from "./useDraft";

const USING: Readonly<Record<Advanced["coins"], string>> = messageTable("pay.confirm.using", ["any", "mixed", "selected"]);

/** The contact a payment pays (DP3-06): who it is, and whether Send accepts their request too. */
export interface ConfirmContact {
  identity: string;
  payee: Payee;
  acceptsToo: boolean;
  /** The accept went through (D11): the form pays without it from now on. */
  onAccepted: () => void;
}

function coinsText(coins: Advanced): string {
  return coins.coins === "selected" ? tr("pay.confirm.selectedCoins", { count: coins.selected.length }) : USING[coins.coins];
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
  onSend,
  contact,
  unsigned = false,
}: {
  contact?: ConfirmContact;
  /** Advanced's Create unsigned transaction (P15): the sheet reviews a PSBT and sends nothing. */
  unsigned?: boolean;
  confirmed: Confirmed;
  spec: DraftSpec;
  coins: Advanced;
  walletId: string;
  network: NetworkInfo;
  draft: DraftApi;
  returnFocus: () => HTMLElement | null;
  onCancel: () => void;
  /** A result that is no payment's (Create Unsigned's). */
  onOutcome: (o: Outcome) => void;
  /** Send was pressed: the payment's own handle for what follows, kept past the sheet. */
  onSend: (sending: Sending) => PayAttempt;
}) {
  const t = useT();
  const b = useBackend();
  const shown = useDisplay(network);
  const { chain, amountUnit } = shown;
  const lock = useLockStatus();
  const policy = usePaymentAuthentication();
  const quick = useQuickUnlockPolicy(policy.data === false);
  // `undefined` until the wallet list is read: neither the prompt nor Send shows before it is
  // known whether the wallet can sign.
  const wallets = useWallets();
  const watchOnlyKnown = wallets.data?.find((w) => w.walletId === walletId)?.watchOnly;
  // Read and not there, or not readable: Send stays hidden, and the sheet says why.
  const walletUnknown = watchOnlyKnown === undefined && (wallets.isError || wallets.isSuccess);
  const actions = contact ? contactConfirmActions(watchOnlyKnown) : confirmActions(unsigned, watchOnlyKnown);
  const [askAnyway, setAskAnyway] = useState(false);
  const [passphrase, setPassphrase] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /** A contact payment's prepare is out: it waits, up to a minute, for the contact's data to be
   * checked (H-09). */
  const [checkingContact, setCheckingContact] = useState(false);
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
  // Each amount's value at the current price (MP-01): beside the DASH, which is what is sent.
  const fiat = useFiat();
  const discreet = useUi((s) => s.discreet);
  const value = (duffs: number, testId: string) => <FiatLine fiat={fiat} duffs={duffs} hidden={discreet} className="confirm-fiat" testId={testId} />;
  const { recipient, more, estimate, changeAddress, total } = confirmed;
  // A payment to several: each recipient's row with what it receives (`receivedAmounts`).
  const many = !isContact(recipient) && more.length > 0 ? [recipient, ...more] : null;
  const received = many ? receivedAmounts(many, estimate.totalSent) : null;
  const payer = many ? many.findIndex(subtractsFee) : -1;
  // What the fee took from that recipient's entered amount: nothing when a dust rest paid it.
  const deducted = many && received && payer >= 0 ? many[payer].amount - received[payer] : 0;
  const subtracting = payer >= 0 && deducted > 0 ? many![payer] : null;
  // The fee against what the payee gets (after a subtract-fee share; the change is not in it).
  const feeWarn = payFeeWarning(estimate.fee, estimate.totalSent);
  const mixed = mixedSendNotes(coins.coins, estimate);
  const toast = useActionToast(walletId);
  const normal = coins.coins !== "mixed";
  const mixing = useQuery({ queryKey: keys.coinjoinStatus(walletId), queryFn: () => b.coinjoinStatus(walletId), retry: false, enabled: normal });
  const utxos = useUtxos(walletId, coins.coins === "selected");
  const unmixed = normal && paysWithUnmixed(coins, utxos.data, mixing.data);

  // A failed read of the policy counts as "on" (U-03 D3). With the signing window, an unread
  // window also asks: the prompt waits until that read settles. A sheet that sends nothing
  // (unsigned, watch-only) asks for nothing.
  const windowNeeded = policy.data === false;
  const acceptQuote = useQuery({
    queryKey: ["pay", "accept-grant", walletId, contact?.identity] as const,
    queryFn: () => b.grantRequest(walletId, contact!.identity, { kind: "acceptRequest" }),
    enabled: Boolean(contact?.acceptsToo && contact.identity),
    retry: false,
  });
  const policyKnown =
    !policy.isPending && (!windowNeeded || quick.isSuccess || quick.isError) && (!contact?.acceptsToo || acceptQuote.isSuccess || acceptQuote.isError);
  // Accept and pay also spends the quoted platform value. Until that quote is known, ask.
  const coveredAmount = !contact?.acceptsToo ? total : acceptQuote.isSuccess ? acceptAndPayValue(total, acceptQuote.data) : Number.MAX_SAFE_INTEGER;
  const cover = quick.isSuccess
    ? { open: quick.data.signingWindowOpen, limitDuffs: quick.data.signingWindowRemainingDuffs ?? 0, amount: coveredAmount }
    : null;
  const prompt = !actions.credential ? "none" : askAnyway ? "passphrase" : credentialPrompt(lock.data, policy.isSuccess ? policy.data : undefined, windowNeeded ? cover : null);
  // The policy loads after the sheet opened: once it asks for nothing, Cancel takes the focus,
  // so Return does not send (U-03 D4). The passphrase field focuses itself when it appears.
  const cancel = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (policyKnown && prompt === "none") cancel.current?.focus();
  }, [policyKnown, prompt]);
  const unsignedButton = useRef<HTMLButtonElement>(null);
  const unsignedFailed = useRef(false);
  // The field was disabled while `authorize` ran: after a refusal, focus goes back to it.
  const field = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (!busy && error && !unsignedFailed.current) field.current?.focus();
  }, [busy, error]);
  const form = useRef<HTMLFormElement>(null);
  const dialog = () => form.current?.closest<HTMLElement>('[role="dialog"]');
  const holdFocus = () => {
    if (form.current?.contains(document.activeElement)) dialog()?.focus();
  };
  useEffect(() => {
    if (busy || !unsignedFailed.current) return;
    unsignedFailed.current = false;
    const a = document.activeElement;
    if (a === document.body || a === dialog()) unsignedButton.current?.focus();
  }, [busy]);

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
    if (!actions.send || busy || !policyKnown || !waitKnown || retryAt !== null) return;
    if (prompt === "passphrase" && passphrase === "") {
      setError(t("pay.confirm.needPassphrase"));
      return;
    }
    holdFocus();
    setBusy(true);
    setError(null);
    const attempt = onSend({ amount: estimate.totalSent, fee: estimate.fee });
    const credential: CredentialSpec = prompt === "passphrase" ? { kind: "passphrase", passphrase } : { kind: "unencrypted" };
    setPassphrase("");
    const refused = (e: unknown) => {
      setBusy(false);
      const refusal = isAppError(e) ? authRefusal(e) : { kind: "other" as const };
      if (refusal.kind === "needsPassphrase") {
        attempt.release();
        setAskAnyway(true);
        setError(t("pay.confirm.needPassphrase"));
      } else if (refusal.kind === "wrongPassphrase") {
        attempt.release();
        setError(refusal.text);
        // The vault refuses every attempt until then: Send waits too (Sol r1).
        if (refusal.waitSecs > 0) setRetryAt(Date.now() + refusal.waitSecs * 1000);
      } else attempt.outcome({ kind: "failed", text: failureText(e, chain, amountUnit) });
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
        lease = await acceptUnderLease(b, {
          walletId,
          identity: contact.identity,
          from: contact.payee.contact,
          spendCap: total,
          credential,
          windowed: credential.kind === "unencrypted" && lock.data?.encrypted === true,
        });
      } catch (e) {
        if (e instanceof AcceptFailed && e.stage === "authorize") return refused(e.cause);
        const text = acceptFailedText(e instanceof AcceptFailed ? e.cause : e);
        toast({ kind: "accept", contact: contact.payee.contact, ok: false, text }, true);
        attempt.outcome({ kind: "failed", text });
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
    const notSent = (text: string) => attempt.outcome({ kind: "failed", text: accepted ? `${accepted} ${text}` : text });
    const endLease = () => lease !== null && void b.endFlow(lease).catch(() => null);

    const toContact = isContact(spec.recipient);
    let held;
    setCheckingContact(toContact);
    try {
      held = await draft.prepare(spec, authId);
    } catch (e) {
      if (grant) void b.revokeGrant(grant.id).catch(() => null);
      endLease();
      notSent(failureText(e, chain, amountUnit, toContact));
      return;
    } finally {
      setCheckingContact(false);
    }
    const { draft: d, prepared } = held;
    if (!keepsPromise(confirmed, prepared.summary)) {
      await draft.abandon(d, prepared.handle);
      endLease();
      notSent(t("pay.error.changed"));
      return;
    }
    // An accept that went through is said with whatever outcome the payment gets, a retry's too.
    const sent = { amount: estimate.totalSent, fee: prepared.summary.fee, ...(accepted && { note: accepted }) };
    attempt.broadcasting(sent);
    try {
      const { txid } = await draft.broadcast(d, prepared.handle);
      attempt.outcome({ kind: "sent", txid, ...sent });
    } catch (e) {
      // "Not sent" only when the engine released it, held it back before its hand-off, or the
      // window locked before it was asked (then it is abandoned here); nothing else is abandoned
      // after a broadcast.
      const never = refusedWhileLocked(e);
      const o = never ? ({ kind: "failed", text: t("pay.error.lockedBeforeSend") } as const) : broadcastFailure(e, chain, { txid: prepared.summary.txid, draft: d, prepared: prepared.handle, ...sent }, true, amountUnit, toContact);
      if (o.kind === "failed") {
        const engineHolds = toContact || never || hasCode(e, UNRECORDED);
        await settleNotSent(engineHolds ? () => draft.abandon(d, prepared.handle) : null, () => notSent(o.text));
      } else attempt.outcome(accepted && o.kind === "unknown" ? { ...o, text: `${accepted} ${o.text}` } : o);
    } finally {
      // The lease ends with the first broadcast (dwcli's point): a retry needs none.
      endLease();
    }
  };

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  // A PSBT that arrives after the sheet went (another page, another wallet) is dropped: it neither
  // reaches the clipboard nor opens Tools.
  const createUnsigned = async () => {
    if (busy) return;
    holdFocus();
    setBusy(true);
    setError(null);
    let psbt: string;
    try {
      psbt = await draft.createUnsigned(spec);
    } catch (e) {
      if (!mounted.current) return;
      unsignedFailed.current = true;
      setBusy(false);
      setError(failureText(e, chain));
      return;
    }
    // The engine planned again: the PSBT leaves the sheet only if it is the payment shown, and one
    // that cannot be checked is not shown to be (D37).
    const analysis = await b.analyzePsbt(walletId, psbt).catch(() => null);
    if (!mounted.current) return;
    if (analysis === null || !unsignedKeepsPromise(confirmed, analysis)) {
      onOutcome({ kind: "failed", text: t("pay.error.changed") });
      return;
    }
    const copied = await navigator.clipboard.writeText(psbt).then(
      () => true,
      () => false,
    );
    if (!mounted.current) return;
    navigate("tools", { toolsTab: "psbt", psbtPrefill: { psbt, copied } });
  };

  return (
    <Sheet title={actions.unsignedFirst ? t("pay.confirm.titleUnsigned") : t("pay.confirm.title")} width="small" testId="pay-confirm" returnFocus={returnFocus} onClose={() => !busy && onCancel()}>
      <form
        ref={form}
        className="confirm"
        onSubmit={(e) => {
          e.preventDefault();
          if (actions.send) void send();
        }}
      >
        <div className="confirm-summary">
          <AmountText className="confirm-amount" text={shown.full(total)} unitName={unit} glyphFactor={0.8} />
          {value(total, "pay-confirm-fiat")}
          <p className="confirm-question">
            {contact?.acceptsToo
              ? t("pay.confirm.acceptQuestion", { name: contact.payee.name })
              : actions.unsignedFirst
              ? t("pay.confirm.unsignedQuestion")
              : t("pay.confirm.question")}
          </p>
        </div>
        <dl className="detail-list card-l1" data-testid="pay-confirm-facts">
          {isContact(recipient) ? (
            <DetailRow label={t("pay.confirm.payTo")} testId="pay-confirm-contact">
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
          ) : many && received ? (
            many.map((r, i) => (
              <DetailRow key={r.address} label={i === 0 ? t("pay.confirm.recipients", { count: many.length }) : ""} testId={`pay-confirm-recipient-${i}`}>
                {r.label && <span className="confirm-label">{r.label}</span>}
                <span className="address-full">{r.address}</span>
                <AmountText text={shown.full(received[i])} unitName={unit} glyph={false} />
              </DetailRow>
            ))
          ) : (
            <DetailRow label={t("pay.confirm.payTo")} testId="pay-confirm-address">
              {recipient.label && <span className="confirm-label">{recipient.label}</span>}
              <span className="address-full">{recipient.address}</span>
            </DetailRow>
          )}
          {contact?.acceptsToo && (
            <DetailRow label={t("pay.confirm.contactRequest")} testId="pay-confirm-accept">
              {t("pay.confirm.acceptedWith")}
            </DetailRow>
          )}
          {isContact(recipient) && recipient.note && <DetailRow label={t("pay.confirm.note")}>{recipient.note}</DetailRow>}
          <DetailRow label={t("pay.confirm.amount")} testId="pay-confirm-amount">
            <AmountText text={shown.full(estimate.totalSent)} unitName={unit} glyph={false} />
            {value(estimate.totalSent, "pay-confirm-amount-fiat")}
          </DetailRow>
          <DetailRow label={t("pay.confirm.networkFee")} testId="pay-confirm-fee">
            <AmountText text={shown.full(estimate.fee)} unitName={unit} glyph={false} />
            {value(estimate.fee, "pay-confirm-fee-fiat")}
          </DetailRow>
          <DetailRow label={t("pay.confirm.usingLabel")}>{coinsText(coins)}</DetailRow>
          {changeAddress !== null && (
            <DetailRow label={t("pay.confirm.changeTo")} testId="pay-confirm-change">
              <span className="address-full">{changeAddress}</span>
              <AmountText text={shown.full(estimate.change ?? 0)} unitName={unit} glyph={false} />
            </DetailRow>
          )}
          <DetailRow label={t("pay.confirm.total")} testId="pay-confirm-total">
            <AmountText text={shown.full(total)} unitName={unit} glyph={false} />
            {value(total, "pay-confirm-total-fiat")}
          </DetailRow>
        </dl>
        {feeWarn && (
          <p className="fee-warning" role="alert" data-testid="pay-confirm-fee-warning">
            {feeWarningText(feeWarn)}
          </p>
        )}
        {many
          ? subtracting && (
              <p className="placeholder-note" data-testid="pay-confirm-subtract-many">
                {t("pay.confirm.subtractNoteMany", { amount: `${shown.full(deducted)} ${unit}`, address: ("label" in subtracting && subtracting.label) || ("address" in subtracting ? subtracting.address : "") })}
              </p>
            )
          : subtractsFee(recipient) &&
            // Only when the fee took something: a remainder can pay it all (a fully mixed payment).
            estimate.totalSent < recipient.amount && <p className="placeholder-note">{t("pay.confirm.subtractNote")}</p>}
        {mixed && (
          <>
            <p className="placeholder-note" data-testid="pay-confirm-mixed-coins">
              {t("pay.confirm.mixed.coins", { count: mixed.inputCount })}
            </p>
            {mixed.manyCoins && (
              <p className="fee-warning" role="alert" data-testid="pay-confirm-mixed-warning">
                {t("pay.confirm.mixed.manyCoins")}
              </p>
            )}
            {mixed.leftOver !== null && (
              <p className="placeholder-note" data-testid="pay-confirm-mixed-change">
                {t.rich("pay.confirm.mixed.noChange", { amount: <AmountText text={shown.full(mixed.leftOver)} unitName={unit} glyph={false} /> })}
              </p>
            )}
          </>
        )}
        {changeAddress !== null && <p className="placeholder-note">{t("pay.confirm.changeNote")}</p>}
        {unmixed && (
          <p className="placeholder-note" data-testid="pay-confirm-unmixed">
            {t("pay.confirm.unmixed")}
          </p>
        )}
        {!policyKnown ? (
          <LoadingState testId="pay-confirm-policy" text={t("pay.confirm.checkingPolicy")} />
        ) : (
          prompt === "passphrase" && (
            <div className="form-field">
              <label htmlFor={fieldId}>{t("pay.confirm.passphrase")}</label>
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
            {error ? t("pay.confirm.waitAfterError", { secs: String(waitSecs) }) : t("pay.confirm.wait", { secs: String(waitSecs) })}
          </p>
        )}
        {walletUnknown && !unsigned && (
          <p className="placeholder-note" role="status" data-testid="pay-confirm-wallet-unknown">
            {contact ? t("pay.confirm.walletUnknown") : t("pay.confirm.walletUnknownUnsigned")}
          </p>
        )}
        {contact && (
          <span className="sr-only" role="status" data-testid="pay-confirm-watch-only-status">
            {watchOnlyKnown === true ? t("pay.contact.watchOnly") : ""}
          </span>
        )}
        {contact && watchOnlyKnown === true && (
          <p className="placeholder-note" aria-hidden="true" data-testid="pay-confirm-watch-only">
            {t("pay.contact.watchOnly")}
          </p>
        )}
        <div className="sheet-actions sheet-actions-fit">
          <DashButton key="cancel" ref={cancel} variant="tintedGray" size="large" data-testid="pay-confirm-cancel" disabled={busy} onClick={onCancel}>
            {t("common.cancel")}
          </DashButton>
          {!contact && (
            <DashButton
              key="unsigned"
              ref={unsignedButton}
              variant={actions.unsignedFirst ? "filledBlue" : "tintedBlue"}
              size="large"
              data-testid="pay-confirm-unsigned"
              disabled={busy}
              onClick={() => void createUnsigned()}
            >
              {t("pay.confirm.createUnsigned")}
            </DashButton>
          )}
          {actions.send && (
            <DashButton key="send" type="submit" variant="filledBlue" size="large" data-testid="pay-confirm-send" disabled={busy || !policyKnown || !waitKnown || retryAt !== null} why={busy ? null : retryAt !== null ? t("pay.confirm.waitMoment") : t("pay.confirm.checkingNetwork")}>
              {busy ? (checkingContact ? t("pay.confirm.checkingContact") : t("pay.confirm.sending")) : contact?.acceptsToo ? t("pay.confirm.acceptAndPay") : t("pay.confirm.send")}
            </DashButton>
          )}
        </div>
      </form>
    </Sheet>
  );
}
