// Pay ▸ To an address (U-03, UX-SPEC §4.7 on the mobile layout): a recipient, Max, subtract
// fee, label and message, and (off mainnet, `multiRecipientPay`) more recipients in the same
// payment, one of them at most paying the fee; the live fee from the engine's draft; Advanced holds coin control and
// the unsigned transaction (P15); Review opens the confirm sheet, which authenticates, signs and
// sends; the result follows. A watch-only wallet's Review makes the PSBT.

import { useCallback, useEffect, useMemo, useRef, useState, type ClipboardEvent, type DragEvent } from "react";
import { useQuery } from "@tanstack/react-query";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { useBackend, useLockStatus, useShieldedStatus, useWallets } from "../../api/queries";
import { ui, type PayPrefill } from "../../app/ui";
import { useScreenScanMode } from "../../contacts/queries";
import { useModel } from "../../onboarding/model";
import { PayScanModel, type Scanned } from "./scan";
import { useDebounced } from "../../app/useDebounced";
import { DashButton } from "../../components/basics";
import { AddressBookPicker } from "../tools/AddressBook";
import { ShieldedPayButton } from "../ShieldedSheet";
import { useDisplay } from "../../format/display";
import { useT } from "../../i18n";
import { CoinControl } from "./CoinControl";
import { ConfirmSheet } from "./ConfirmSheet";
import { HeldPlatformSend, PayFromPlatform } from "./PayFromPlatform";
import { AmountFields, draftSpec, FormField, PayFooter, SETTLE_MS, useAvailable, useEstimate, useMax, payFormKey, usePayAmount, usePaySheets } from "./form";
import { addressProblem, confirmedOf, DEFAULT_ADVANCED, multiRecipientPay, recipientOf, reviewBlockedReason, type Advanced } from "./model";
import { MoreRecipient, type MoreRow } from "./MoreRecipients";
import { ResultSheet, SendingSheet } from "./ResultSheet";
import { UnsignedPsbt } from "./UnsignedPsbt";
import { useDraft } from "./useDraft";

export function PayToAddress({
  prefill,
  coins,
  walletId,
  network,
}: {
  prefill: PayPrefill | null;
  /** Coin Selection's coins (U-05 D19): Advanced starts on "Selected coins" with them. */
  coins: string[] | null;
  walletId: string;
  network: NetworkInfo;
}) {
  const t = useT();
  const b = useBackend();
  const shown = useDisplay(network);
  const locked = useLockStatus().data?.state === "locked";
  const draft = useDraft(walletId, locked);
  const watchOnly = useWallets().data?.find((w) => w.walletId === walletId)?.watchOnly === true;

  const [address, setAddress] = useState(prefill?.address ?? "");
  const amount = usePayAmount(shown, prefill?.amount ? shown.full(prefill.amount) : "");
  // Settings ▸ General's default starts each new form (U-04 D5); Max still turns it on.
  const subtractFeeByDefault = () => ui.get().prefs.subtractFeeByDefault;
  const [subtractFee, setSubtractFeeFirst] = useState(subtractFeeByDefault);
  // Added recipients (QT-052). One recipient at most subtracts the fee: turning it on for one
  // turns it off for the others.
  const [more, setMore] = useState<MoreRow[]>([]);
  const nextRow = useRef(1);
  const setSubtractFee = useCallback((on: boolean) => {
    setSubtractFeeFirst(on);
    if (on) setMore((rows) => rows.map((r) => (r.subtractFee ? { ...r, subtractFee: false } : r)));
  }, []);
  const setRowSubtractFee = (id: number, on: boolean) => {
    if (on) setSubtractFeeFirst(false);
    setMore((rows) => rows.map((r) => (r.id === id ? { ...r, subtractFee: on } : on ? { ...r, subtractFee: false } : r)));
  };
  const multi = multiRecipientPay(network);
  const [label, setLabel] = useState(prefill?.label ?? "");
  const [message, setMessage] = useState(prefill?.message ?? "");
  const [advanced, setAdvanced] = useState<Advanced>(coins?.length ? { ...DEFAULT_ADVANCED, coins: "selected", selected: coins } : DEFAULT_ADVANCED);
  // A payment that went, or may have, ends the form: paying again is a new payment.
  const sheets = usePaySheets(payFormKey(walletId), walletId, draft, () => {
    setAddress("");
    amount.setText("");
    setSubtractFee(subtractFeeByDefault());
    setMore([]);
    setLabel("");
    setMessage("");
    setAdvanced(DEFAULT_ADVANCED);
  }, "link");
  const { phase } = sheets;
  // Closing the confirm or result sheet returns to Review, or to the address of a cleared form.
  const reviewButton = useRef<HTMLButtonElement>(null);
  const addressField = useRef<HTMLInputElement>(null);
  // Advanced's unsigned path gets the focus back, never Review's signing one (P15): its button, or
  // its summary while the payment is estimated again.
  const psbtButton = useRef<HTMLButtonElement>(null);
  const psbtSummary = useRef<HTMLElement>(null);
  const openedBy = useRef<"review" | "psbt">("review");
  const backToForm = useCallback(() => {
    if (openedBy.current === "psbt") return psbtButton.current?.disabled === false ? psbtButton.current : psbtSummary.current;
    return reviewButton.current?.disabled === false ? reviewButton.current : addressField.current;
  }, []);
  // The sending address book (QT-097): choosing fills the address, and its label if none was typed.
  const [picking, setPicking] = useState(false);
  const chosen = useCallback((a: string, l: string) => {
    setAddress(a);
    setLabel((old) => old || l);
    setPicking(false);
    addressField.current?.focus();
  }, []);

  // Scan QR (IOS-043): a code that pays opens Pay prefilled, as its `dash:` link would.
  // The form stays as it is (coins, fee, subtract fee); a link's amount, label and message replace
  // the typed ones only when it carries them, and every field stays editable before Review.
  const onScanned = useRef<(s: Scanned) => void>(() => undefined);
  onScanned.current = (s) => {
    setAddress(s.address);
    if (s.amount !== null) amount.setText(shown.full(s.amount));
    if (s.label) setLabel(s.label);
    if (s.message) setMessage(s.message);
    addressField.current?.focus();
  };
  const scanner = useModel(useMemo(() => new PayScanModel(b, network.dirName, (s) => onScanned.current(s)), [b, network.dirName]));
  useEffect(() => () => scanner.dispose(), [scanner]);
  const scanMode = useScreenScanMode();
  const imageOf = (data: DataTransfer | null) => [...(data?.files ?? [])].find((f) => f.type.startsWith("image/")) ?? null;
  const pasteImage = (e: ClipboardEvent<HTMLInputElement>) => {
    const file = imageOf(e.clipboardData);
    if (!file) return;
    e.preventDefault();
    void scanner.imageFile(file);
  };
  const dropImage = (e: DragEvent<HTMLElement>) => {
    const file = imageOf(e.dataTransfer);
    if (!file) return;
    e.preventDefault();
    void scanner.imageFile(file);
  };
  const scan = scanner.state;

  const addr = address.trim();
  const checkedAddr = useDebounced(addr, SETTLE_MS);
  const settled = checkedAddr === addr && amount.settled;

  const addrClass = useQuery({
    queryKey: ["classifyAddress", network.dirName, checkedAddr],
    queryFn: () => b.classifyAddress(network.dirName, checkedAddr),
    enabled: checkedAddr !== "",
    staleTime: Infinity,
  });

  // A Shielded address is paid with Send from Shielded while Shielded is on (the page's link).
  const shieldedStatus = useShieldedStatus(walletId).data ?? null;
  const shieldedRoute = shieldedStatus?.active ? shieldedStatus : null;
  const shieldedTo = shieldedRoute !== null && checkedAddr !== "" && addrClass.data?.kind === "shielded";
  const addrError = checkedAddr !== "" && addrClass.data && !shieldedTo ? addressProblem(addrClass.data) : null;
  // A Platform address is paid from the Platform balance (platform/send.ts), not by a Core draft.
  const platform = checkedAddr !== "" && checkedAddr === addr && addrClass.data?.kind === "platform";
  const { duffs } = amount;

  const recipient =
    addrClass.data?.kind === "core" && checkedAddr !== "" && typeof duffs === "number" && duffs > 0
      ? recipientOf({ address: checkedAddr, amount: duffs, subtractFee, label, message })
      : null;
  const moreRecipients = more.every((r) => r.recipient) ? more.map((r) => r.recipient!) : null;
  const othersTotal = more.reduce((n, r) => n + (r.amount ?? 0), 0);
  const rowsSettled = more.every((r) => r.settled);
  const spec = draftSpec(recipient, advanced, multi ? moreRecipients : []);
  const available = useAvailable(walletId, advanced, sheets.held);

  const { estimate, general: generalError, fieldError, ready: estimated, offer } = useEstimate(walletId, draft, spec, advanced, locked || sheets.held, shown);
  const ready = estimated && settled && !amount.fiat.error;
  const max = useMax(
    walletId,
    advanced,
    shown,
    (text) => {
      amount.setText(text);
      setSubtractFee(true);
    },
    othersTotal,
    !rowsSettled,
  );

  const review = (from: "review" | "psbt") => {
    const unsigned = from === "psbt" || watchOnly;
    openedBy.current = from;
    if (ready && estimate.data && spec) sheets.review(confirmedOf(spec.recipient, estimate.data, spec.change, spec.more), spec, unsigned);
  };

  return (
    <>
      <form
        className="pay-form"
        data-testid="pay-address-form"
        aria-label={t("pay.toAddress.form")}
        onSubmit={(e) => {
          e.preventDefault();
          review("review");
        }}
      >
        {prefill?.username && (
          <p className="placeholder-note" data-testid="pay-username">
            {t("pay.toAddress.linkUsername", { username: `@${prefill.username}` })}
          </p>
        )}
        <section className="pay-card card-l1">
          <FormField label={t("pay.toAddress.payTo")} error={addrError ?? fieldError("address")} hint={shieldedTo ? t("pay.address.shieldedRoute") : undefined}>
            {(p) => (
              <span className="amount-entry">
                <input
                  {...p}
                  ref={addressField}
                  data-testid="pay-address"
                  value={address}
                  autoComplete="off"
                  spellCheck={false}
                  placeholder={t("pay.toAddress.placeholder")}
                  onChange={(e) => {
                    setAddress(e.target.value);
                    scanner.dismiss();
                  }}
                  onPaste={pasteImage}
                  onDragOver={(e) => {
                    if ([...e.dataTransfer.items].some((i) => i.kind === "file" && i.type.startsWith("image/"))) e.preventDefault();
                  }}
                  onDrop={dropImage}
                />
                <DashButton variant="tintedBlue" size="small" data-testid="pay-address-book" onClick={() => setPicking(true)}>
                  {t("pay.toAddress.addressBook")}
                </DashButton>
                <DashButton variant="tintedBlue" size="small" data-testid="pay-scan-image" disabled={scanner.busy} onClick={() => void scanner.openImage()}>
                  {t("pay.scan.openImage")}
                </DashButton>
              </span>
            )}
          </FormField>
          {shieldedTo && (
            <div className="pay-shielded-route">
              <ShieldedPayButton walletId={walletId} status={shieldedRoute} to={checkedAddr} text={typeof amount.duffs === "number" && amount.duffs > 0 ? amount.text.trim() : ""} />
            </div>
          )}
          <div className="pay-scan-row" data-testid="pay-scan">
            {scan.kind === "reading" || scan.kind === "capturing" ? (
              <span className="field-hint" role="status" data-testid="pay-scan-status">
                {t(scan.kind === "reading" ? "pay.scan.reading" : "pay.scan.capturing")}
              </span>
            ) : scan.kind === "failed" ? (
              <span className="field-error" role="alert" data-testid="pay-scan-error">
                {scan.text}
              </span>
            ) : (
              <span className="field-hint">{t("pay.scan.hint")}</span>
            )}
            {scanMode.data === "picker" &&
              (scan.kind === "capturing" ? (
                <DashButton key="cancel" variant="plainBlue" size="small" data-testid="pay-scan-screen-cancel" onClick={() => scanner.cancelScreen()}>
                  {t("pay.scan.cancel")}
                </DashButton>
              ) : (
                <DashButton key="screen" variant="plainBlue" size="small" data-testid="pay-scan-screen" disabled={scanner.busy} onClick={() => void scanner.screen()}>
                  {t("pay.scan.screen")}
                </DashButton>
              ))}
          </div>
          {platform ? (
            <PayFromPlatform walletId={walletId} network={network} to={checkedAddr} amount={amount} onSent={() => setAddress("")} />
          ) : (
            <>
          <AmountFields
            shown={shown}
            amount={amount}
            max={max}
            error={fieldError("amount")}
            available={available}
            coins={advanced.coins}
            subtractFee={subtractFee}
            onSubtractFee={setSubtractFee}
          />
          <FormField label={t("pay.toAddress.label")} hint={t("pay.toAddress.labelHint")}>
            {(p) => <input {...p} data-testid="pay-label" value={label} autoComplete="off" onChange={(e) => setLabel(e.target.value)} />}
          </FormField>
          <FormField label={t("pay.toAddress.message")} hint={t("pay.toAddress.messageHint")}>
            {(p) => <input {...p} data-testid="pay-message" value={message} autoComplete="off" onChange={(e) => setMessage(e.target.value)} />}
          </FormField>
            </>
          )}
        </section>
        {multi &&
          more.map((row, i) => (
            // Hidden, not unmounted, while the first address is a Platform one: the rows keep what was typed.
            <div key={row.id} hidden={platform}>
            <MoreRecipient
              row={row}
              number={i + 2}
              network={network}
              shown={shown}
              shieldedRoute={shieldedRoute !== null}
              fieldError={(f) => fieldError(f, i + 1)}
              onChange={(r, amount, settled) => setMore((rows) => rows.map((x) => (x.id === row.id ? { ...x, recipient: r, amount, settled } : x)))}
              onSubtractFee={(on) => setRowSubtractFee(row.id, on)}
              onRemove={() => setMore((rows) => rows.filter((x) => x.id !== row.id))}
            />
            </div>
          ))}
        {!platform && multi && (
          <div className="pay-more-add">
            <DashButton
              variant="tintedBlue"
              size="small"
              data-testid="pay-add-recipient"
              onClick={() => setMore((rows) => [...rows, { id: nextRow.current++, recipient: null, amount: null, settled: true, subtractFee: false }])}
            >
              {t("pay.more.add")}
            </DashButton>
            <span className="field-hint" data-testid="pay-add-recipient-note">
              {t("pay.more.testnetOnly")}
            </span>
          </div>
        )}
        {!platform && (
          <>
            <CoinControl walletId={walletId} network={network} advanced={advanced} onChange={setAdvanced} payAmount={typeof duffs === "number" && more.every((r) => r.amount !== null) ? [duffs, ...more.map((r) => r.amount!)] : null} />
            <UnsignedPsbt
              watchOnly={watchOnly}
              why={reviewBlockedReason(ready, [addrError, fieldError("address"), amount.fiat.error, amount.error, fieldError("amount"), max.error, generalError], !!spec)}
              onCreate={() => review("psbt")}
              buttonRef={psbtButton}
              summaryRef={psbtSummary}
            />
            <PayFooter reviewLabel={watchOnly ? t("pay.toAddress.reviewPsbt") : t("pay.form.review")} problem={generalError} offer={offer === null || !settled ? null : { amount: offer, accept: () => setSubtractFee(true) }} fee={estimate.data && spec ? estimate.data.fee : null} shown={shown} ready={ready} incomplete={!spec} reviewRef={reviewButton} />
          </>
        )}
      </form>
      {!platform && <HeldPlatformSend walletId={walletId} network={network} />}
      {picking && <AddressBookPicker walletId={walletId} network={network} purpose="send" onChoose={chosen} onClose={() => setPicking(false)} />}
      {phase.kind === "confirm" && (
        <ConfirmSheet
          confirmed={phase.confirmed}
          spec={phase.spec}
          coins={advanced}
          walletId={walletId}
          network={network}
          draft={draft}
          unsigned={phase.unsigned}
          returnFocus={backToForm}
          onCancel={sheets.cancel}
          onOutcome={sheets.outcome}
          onSend={sheets.begin}
        />
      )}
      {phase.kind === "sending" && <SendingSheet sending={phase.sending} network={network} />}
      {phase.kind === "result" && (
        <ResultSheet
          outcome={phase.outcome}
          network={network}
          draft={draft}
          returnFocus={backToForm}
          retrying={phase.retrying}
          onRetry={sheets.retry}
          onDone={sheets.done}
        />
      )}
    </>
  );
}
