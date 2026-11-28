import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import type { GiftInvoice } from "../../api/generated/GiftInvoice";
import type { GiftOrderDetails } from "../../api/generated/GiftOrderDetails";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { useBackend, useLockStatus } from "../../api/queries";
import { AmountText, DashButton } from "../../components/basics";
import { useRateCurrencies, useRates } from "../../fiat/rates";
import { useDisplay } from "../../format/display";
import { duffsToMinor, formatMinor } from "../../format/fiat";
import { useT } from "../../i18n";
import { ConfirmSheet } from "../pay/ConfirmSheet";
import { draftSpec, useEstimate, usePaySheets } from "../pay/form";
import { confirmedOf, DEFAULT_ADVANCED, failureText } from "../pay/model";
import { ResultSheet, SendingSheet } from "../pay/ResultSheet";
import { useDraft } from "../pay/useDraft";

export function Checkout({ details, invoice, network, walletId }: {
  details: GiftOrderDetails;
  invoice: GiftInvoice;
  network: NetworkInfo;
  walletId: string;
}) {
  const { order, status } = details;
  const t = useT();
  const client = useQueryClient();
  const backend = useBackend();
  const [relaying, setRelaying] = useState(false);
  const [relayError, setRelayError] = useState<string | null>(null);
  const shown = useDisplay(network);
  const locked = useLockStatus().data?.state === "locked";
  const draft = useDraft(walletId, locked, order.id);
  const [paidHere, setPaidHere] = useState(false);
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const timer = window.setTimeout(() => setNow(Date.now()), Math.max(0, order.expiresAt * 1000 - Date.now()));
    return () => window.clearTimeout(timer);
  }, [order.expiresAt]);
  const refresh = () => {
    void client.invalidateQueries({ queryKey: ["giftCards", walletId, "orders"] });
    void client.invalidateQueries({ queryKey: ["giftCards", walletId, "detail", order.id] });
  };
  const sheets = usePaySheets(`giftCard:${network.dirName}:${walletId}:${order.id}`, walletId, draft, () => {
    setPaidHere(true);
    refresh();
  });
  const { phase } = sheets;
  const review = useRef<HTMLButtonElement>(null);
  const backToReview = () => review.current;
  const pending = paidHere || order.txid !== null;
  const expired = now >= order.expiresAt * 1000;
  const payable = status === "awaitingPayment" && !expired && !pending;
  const spec = payable ? draftSpec({
    address: invoice.address,
    amount: invoice.duffs,
    subtractFeeFromAmount: false,
    label: null,
    message: t("giftCards.orderNote", { id: order.id }),
  }, DEFAULT_ADVANCED) : null;
  const { estimate, general, ready } = useEstimate(walletId, draft, spec, DEFAULT_ADVANCED, locked || sheets.held, shown);
  const issue = general ?? (estimate.isError ? failureText(estimate.error, shown.chain, shown.amountUnit) : null);
  const ratesOn = useRates().data?.settings.enabled === true;
  const usd = useRateCurrencies(payable && ratesOn).data?.find((rate) => rate.code === "USD");
  const age = usd ? Date.now() - usd.rateAt * 1000 : Infinity;
  const marketCents = ratesOn && usd && age >= 0 && age <= 30 * 60_000
    ? duffsToMinor(invoice.duffs, usd.price, "USD") : null;
  const overFaceValue = marketCents !== null && marketCents * 100n > BigInt(order.cents) * 105n;

  return (
    <>
      {payable && (
        <div className="gift-checkout">
          <div className="gift-quote">
            <AmountText text={shown.full(invoice.duffs)} unitName={shown.name} />
            <p className="field-hint">{t("giftCards.beforeFee")}</p>
            {marketCents !== null && <p className={overFaceValue ? "fee-warning" : "field-hint"} role={overFaceValue ? "alert" : undefined}>
              {t(overFaceValue ? "giftCards.overFaceValue" : "giftCards.marketPrice", { market: formatMinor(marketCents, "USD") })}
            </p>}
            {issue && <p className="form-problem" role="alert">{issue}</p>}
            {!ready && !issue && !sheets.held && <p className="field-hint" role="status">{t("giftCards.preparing")}</p>}
          </div>
          <DashButton ref={review} variant="filledBlue" disabled={!ready} onClick={() => {
            if (spec && estimate.data && ready) sheets.review(confirmedOf(spec.recipient, estimate.data, spec.change), spec);
          }}>{t("giftCards.continue")}</DashButton>
        </div>
      )}
      {pending && status !== "complete" && status !== "processing" && (
        <>
          <p className="field-hint">{t("giftCards.sentPending", { id: order.id })}</p>
          {status === "awaitingPayment" && order.txid && !expired && phase.kind !== "result" && (
            <DashButton variant="tintedGray" disabled={relaying} onClick={async () => {
              setRelaying(true);
              setRelayError(null);
              try {
                await backend.giftCardResend(walletId, order.id);
              } catch (error) {
                setRelayError(failureText(error, shown.chain, shown.amountUnit));
              } finally {
                setRelaying(false);
                refresh();
              }
            }}>{t("giftCards.resend")}</DashButton>
          )}
          {relayError && <p role="alert">{relayError}</p>}
        </>
      )}
      {status === "awaitingPayment" && expired && !pending && <p className="field-hint">{t("giftCards.expired")}</p>}
      {phase.kind === "confirm" && payable && (
        <ConfirmSheet confirmed={phase.confirmed} spec={phase.spec} coins={DEFAULT_ADVANCED} walletId={walletId}
          network={network} draft={draft} returnFocus={backToReview} onCancel={sheets.cancel}
          onOutcome={sheets.outcome} onSend={sheets.begin} />
      )}
      {phase.kind === "sending" && <SendingSheet sending={phase.sending} network={network} />}
      {phase.kind === "result" && (
        <ResultSheet outcome={phase.outcome} network={network} draft={draft} returnFocus={backToReview}
          retrying={phase.retrying} allowRetry={!expired} onRetry={sheets.retry} onDone={(show) => {
            sheets.done(show);
            refresh();
          }} />
      )}
    </>
  );
}
