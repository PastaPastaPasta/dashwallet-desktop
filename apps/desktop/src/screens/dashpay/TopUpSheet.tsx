// The top-up sheet (DASHPAY F8, IOS-131, AND-078; DP1-10 T1–T8): the amount (iOS presets, a
// custom amount), the quote's confirm step with Android's copy, progress, the outcome, and a
// refusal routed to Tools ▸ Repair when the funding's outcome is unknown. The flow is
// dashpay/topup.ts; the sheet can close while the call runs and reopen to its result (D14).

import { useId } from "react";
import { useQuery } from "@tanstack/react-query";
import type { CostTable } from "../../api/generated/CostTable";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { TopUpFromPlatformQuote } from "../../api/generated/TopUpFromPlatformQuote";
import { CREDITS_PER_DUFF, creditsToDuffs } from "../../api/pending";
import { useBackend, useBalance, usePlatformActivity, usePlatformAddresses, usePoolBalances } from "../../api/queries";
import { useDebounced } from "../../app/useDebounced";
import { navigate, useUi } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { creditBalance, creditCount, estimateText } from "../../dashpay/profile";
import { useTopUpQuote } from "../../dashpay/profileQueries";
import { amountCanChange, minTopUp, outcomeText, platformAmount, PRESETS, routesToRepair, TOPUP_COPY, topUpCredits, topUpErrorText, type TopUpFlow } from "../../dashpay/topup";
import {
  canPay,
  FROM_PLATFORM_COPY,
  fromPlatformErrorText,
  liveOutcome,
  offersMove,
  platformHold,
  maxMasked,
  platformOutcomeText,
  SOURCE_COPY,
  topUpSources,
  type PlatformHold,
  type SourceRow,
} from "../../dashpay/topupSource";
import { transferFor } from "../../transfer/model";
import { useDisplay } from "../../format/display";
import { useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import { GrantSheet } from "../tools/common";

/** "Move to Dash wallet" for a held top-up: Home's transfer, at the move to the Dash wallet. */
function useMoveToCore(flow: TopUpFlow, walletId: string) {
  const b = useBackend();
  return () => {
    flow.close();
    navigate("home");
    transferFor(b, walletId).start("platformToCore");
  };
}

/** The sources as the picker shows them, from the balances and the Platform hold. */
function useSources(walletId: string, network: NetworkInfo, amount: (duffs: number) => string, discreet: boolean) {
  const balance = useBalance(walletId);
  const platform = usePlatformAddresses(walletId);
  const activity = usePlatformActivity(walletId);
  const pools = usePoolBalances(walletId);
  const shielded = pools.shielded;
  const shieldedDuffs = typeof shielded === "number" ? shielded : shielded === "absent" ? null : undefined;
  const hold = platformHold(activity.data);
  const rows = topUpSources({
    coreDuffs: balance.data?.confirmed,
    // Holds come from Activity: unread, nothing is known about them yet.
    platform: platform.isError || activity.isError ? "unread" : activity.data === undefined ? undefined : platform.data,
    hold,
    mainnet: network.kind === "mainnet",
    shieldedDuffs,
    amount,
    discreet,
  });
  return { rows, hold };
}

/** "Pay with" (C3-8): each source with its balance, or why it cannot pay. Dash wallet first. A chosen
 * source that can no longer pay stays chosen with its reason: Continue waits for another choice. */
function PayWith({ flow, walletId, rows, hold }: { flow: TopUpFlow; walletId: string; rows: SourceRow[]; hold: PlatformHold }) {
  useT();
  const id = useId();
  const moveToCore = useMoveToCore(flow, walletId);
  return (
    <fieldset className="topup-choices topup-sources" aria-label={SOURCE_COPY.title} data-testid="topup-pay-with">
      <legend className="detail-label">{SOURCE_COPY.title}</legend>
      {rows.map((r) => (
        <label key={r.source} className="topup-choice" data-testid={`topup-source-${r.source}`} data-disabled={r.reason !== null || undefined}>
          <input
            type="radio"
            name={`${id}-source`}
            checked={flow.source === r.source}
            disabled={(r.reason !== null && flow.source !== r.source) || flow.running}
            onChange={() => flow.chooseSource(r.source)}
          />
          <span className="topup-choice-amount">{r.label}</span>
          {r.balance !== null && (
            <span className="topup-choice-buys">
              <bdi dir="ltr">{r.balance}</bdi>
            </span>
          )}
          {r.reason !== null && <span className="topup-choice-buys topup-source-reason">{r.reason}</span>}
        </label>
      ))}
      {offersMove(hold) && (
        <DashButton variant="tintedBlue" size="small" type="button" data-testid="topup-source-move-to-core" onClick={moveToCore}>
          {FROM_PLATFORM_COPY.held.action}
        </DashButton>
      )}
    </fieldset>
  );
}

/** T1: the presets and "Other amount", each with what it buys. */
function AmountStep({ flow, walletId, costs, network, amount, discreet }: { flow: TopUpFlow; walletId: string; costs: CostTable | undefined; network: NetworkInfo; amount: (duffs: number) => string; discreet: boolean }) {
  useT();
  const b = useBackend();
  const id = useId();
  const shown = useDisplay(network);
  const { choice } = flow;
  const text = choice.kind === "other" ? choice.text.trim() : "";
  const checked = useDebounced(text, 300);
  const parsed = useQuery({ queryKey: ["parseAmount", shown.unit, checked], queryFn: () => b.parseAmount(checked, shown.unit), enabled: checked !== "", staleTime: Infinity });
  const sources = useSources(walletId, network, amount, discreet);
  const platform = flow.source === "platform";
  // Platform's own floor is the engine's (`credits.send_out_of_range`), far below Core's.
  const min = platform ? 1 : minTopUp(costs);
  const otherDuffs = choice.kind === "other" && checked === text && typeof parsed.data === "number" ? parsed.data : null;
  const otherProblem =
    choice.kind !== "other" || text === "" || checked !== text || parsed.isPending
      ? null
      : parsed.data === null || parsed.data === undefined
        ? TOPUP_COPY.amount.invalid
        : parsed.data < min
          ? TOPUP_COPY.amount.below(amount(min))
          : null;
  const max = choice.kind === "max";
  const duffs = choice.kind === "preset" ? choice.duffs : choice.kind === "other" && otherProblem === null ? otherDuffs : null;
  // From Platform the identity gains the amount itself; Platform's fee is taken besides it.
  const buys = (d: number) => (costs ? estimateText(platform ? BigInt(d) * CREDITS_PER_DUFF : topUpCredits(d, costs), costs) : null);
  const payable = canPay(sources.rows, flow.source);
  return (
    <form
      className="topup-form"
      onSubmit={(e) => {
        e.preventDefault();
        if (payable && (duffs !== null || max)) flow.next(max ? null : duffs);
      }}
    >
      <p className="sheet-text">{TOPUP_COPY.amount.intro}</p>
      <PayWith flow={flow} walletId={walletId} rows={sources.rows} hold={sources.hold} />
      <fieldset className="topup-choices" aria-label={TOPUP_COPY.amount.field}>
        <legend className="detail-label">{TOPUP_COPY.amount.field}</legend>
        {PRESETS.map((p) => (
          <label key={p} className="topup-choice" data-testid={`topup-preset-${p}`}>
            <input type="radio" name={`${id}-amount`} checked={choice.kind === "preset" && choice.duffs === p} onChange={() => flow.choose({ kind: "preset", duffs: p })} />
            <span className="topup-choice-amount">
              <bdi dir="ltr">{amount(p)}</bdi>
            </span>
            {buys(p) && <span className="topup-choice-buys">{buys(p)}</span>}
          </label>
        ))}
        <label className="topup-choice" data-testid="topup-other">
          <input type="radio" name={`${id}-amount`} checked={choice.kind === "other"} onChange={() => flow.choose({ kind: "other", text: "" })} />
          <span className="topup-choice-amount">{TOPUP_COPY.amount.other}</span>
          {otherDuffs !== null && otherProblem === null && buys(otherDuffs) && <span className="topup-choice-buys">{buys(otherDuffs)}</span>}
        </label>
        {flow.source === "platform" && (
          <label className="topup-choice" data-testid="topup-max">
            <input type="radio" name={`${id}-amount`} checked={max} onChange={() => flow.choose({ kind: "max" })} />
            <span className="topup-choice-amount">{SOURCE_COPY.max}</span>
          </label>
        )}
      </fieldset>
      {choice.kind === "other" && (
        <label className="form-field">
          {TOPUP_COPY.amount.fieldUnit(shown.name)}
          <input
            data-testid="topup-other-field"
            inputMode="decimal"
            autoFocus
            placeholder={shown.placeholder}
            value={choice.text}
            aria-invalid={otherProblem !== null}
            aria-describedby={`${id}-problem`}
            onChange={(e) => flow.choose({ kind: "other", text: e.target.value })}
          />
        </label>
      )}
      <p className="form-problem" id={`${id}-problem`} role="alert" data-testid="topup-amount-problem">
        {otherProblem}
      </p>
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" type="button" data-testid="topup-cancel" onClick={() => flow.close()}>
          {TOPUP_COPY.amount.cancel}
        </DashButton>
        <DashButton variant="filledBlue" size="large" type="submit" data-testid="topup-next" disabled={!payable || (duffs === null && !max)} why={payable ? TOPUP_COPY.amount.first : null}>
          {TOPUP_COPY.amount.next}
        </DashButton>
      </div>
    </form>
  );
}

/** T2: the quote, Android's confirm copy, Confirm & pay under the quote's grant. */
function ConfirmStep({ flow, walletId, costs, amount, discreet }: { flow: TopUpFlow; walletId: string; costs: CostTable | undefined; amount: (duffs: number) => string; discreet: boolean }) {
  useT();
  const quote = useTopUpQuote(walletId, flow.identity, flow.duffs);
  const q = quote.data;
  return (
    <>
      <p className="sheet-text">{TOPUP_COPY.confirm.message}</p>
      {quote.isError ? (
        <p className="form-problem" role="alert" data-testid="topup-quote-error">
          {topUpErrorText(quote.error, amount, discreet)}
        </p>
      ) : !q ? (
        <p className="sheet-text" role="status">
          {TOPUP_COPY.confirm.quoting}
        </p>
      ) : (
        <>
          <dl className="detail-list topup-quote" data-testid="topup-quote">
            <div className="detail-row">
              <dt className="detail-label">{TOPUP_COPY.confirm.amount}</dt>
              <dd className="detail-value">
                <bdi dir="ltr">{amount(flow.duffs ?? 0)}</bdi>
              </dd>
            </div>
            <div className="detail-row">
              <dt className="detail-label">{TOPUP_COPY.confirm.fee}</dt>
              <dd className="detail-value">
                <bdi dir="ltr">{amount(q.feeDuffs)}</bdi>
              </dd>
            </div>
            <div className="detail-row topup-total">
              <dt className="detail-label">{TOPUP_COPY.confirm.total}</dt>
              <dd className="detail-value">
                <bdi dir="ltr">{amount(q.totalDuffs)}</bdi>
              </dd>
            </div>
          </dl>
          {costs && <p className="sheet-text">{TOPUP_COPY.confirm.adds(creditCount(q.credits), estimateText(q.credits, costs))}</p>}
        </>
      )}
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" data-testid="topup-back" disabled={flow.running} onClick={() => flow.back()}>
          {TOPUP_COPY.confirm.back}
        </DashButton>
        <DashButton variant="filledBlue" size="large" data-testid="topup-pay" disabled={!q || flow.running} onClick={() => q && void flow.pay(q.grant)}>
          {TOPUP_COPY.confirm.pay}
        </DashButton>
      </div>
    </>
  );
}

/** T2 from Platform: the quote, the D-C3 confirm copy, Confirm & pay under the quote's grant. */
function PlatformConfirmStep({ flow, walletId, name, amount, discreet, masked }: { flow: TopUpFlow; walletId: string; name: string; amount: (duffs: number) => string; discreet: boolean; masked: string }) {
  useT();
  const b = useBackend();
  const asked = platformAmount(flow.duffs);
  const quote = useQuery<TopUpFromPlatformQuote>({
    queryKey: ["dashpay", "topUpFromPlatformQuote", walletId, flow.identity, asked],
    queryFn: () => b.creditsTopUpFromPlatformQuote(walletId, flow.identity!, asked),
    enabled: flow.identity !== null,
    retry: false,
    staleTime: 0,
    gcTime: 0,
  });
  // Only a current answer pays: never one cached from before, one being read again, or one a
  // later read refused.
  const q = quote.isSuccess && !quote.isFetching ? quote.data : undefined;
  const hidden = maxMasked(discreet, flow.duffs === null, masked);
  const credits = (c: string) => hidden ?? amount(creditsToDuffs(c));
  return (
    <>
      {quote.isError ? (
        <p className="form-problem" role="alert" data-testid="topup-quote-error">
          {fromPlatformErrorText(quote.error, amount, discreet)}
        </p>
      ) : !q ? (
        <p className="sheet-text" role="status">
          {TOPUP_COPY.confirm.quoting}
        </p>
      ) : (
        <>
          <p className="sheet-text topup-platform-message" data-testid="topup-platform-message">
            {FROM_PLATFORM_COPY.confirm(name, credits(q.credits), credits(String(BigInt(q.credits) + BigInt(q.feeCredits))), credits(q.feeCredits), q.inputs[0]?.address ?? "")}
          </p>
          <dl className="detail-list topup-quote" data-testid="topup-quote">
            <div className="detail-row">
              <dt className="detail-label">{TOPUP_COPY.confirm.amount}</dt>
              <dd className="detail-value">
                <bdi dir="ltr">{credits(q.credits)}</bdi>
              </dd>
            </div>
            <div className="detail-row">
              <dt className="detail-label">{FROM_PLATFORM_COPY.fee}</dt>
              <dd className="detail-value">
                <bdi dir="ltr">{credits(q.feeCredits)}</bdi>
              </dd>
            </div>
            <div className="detail-row topup-total">
              <dt className="detail-label">{TOPUP_COPY.confirm.total}</dt>
              <dd className="detail-value">
                <bdi dir="ltr">{credits(String(BigInt(q.credits) + BigInt(q.feeCredits)))}</bdi>
              </dd>
            </div>
          </dl>
        </>
      )}
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" data-testid="topup-back" disabled={flow.running} onClick={() => flow.back()}>
          {TOPUP_COPY.confirm.back}
        </DashButton>
        <DashButton variant="filledBlue" size="large" data-testid="topup-pay" disabled={!q || flow.running} onClick={() => q && void flow.payFromPlatform(q)}>
          {TOPUP_COPY.confirm.pay}
        </DashButton>
      </div>
    </>
  );
}

export function TopUpSheet({ flow, walletId, network, costs, name }: { flow: TopUpFlow; walletId: string; network: NetworkInfo; costs: CostTable | undefined; name: string }) {
  useT();
  useModel(flow);
  const shown = useDisplay(network);
  const discreet = useUi((s) => s.discreet);
  const amount = (duffs: number) => `${shown.compact(duffs)} ${shown.name}`;
  const activity = usePlatformActivity(walletId);
  const moveToCore = useMoveToCore(flow, walletId);
  if (!flow.open) return <GrantSheet grants={flow.grants} />;
  const { step } = flow;
  const balance = (credits: string) => creditBalance(credits, costs, discreet, amount, shown.masked());
  // A top-up from Platform answered `pending` may since have failed by height: Activity says.
  const fromPlatform = step === "done" && flow.platformOutcome ? platformOutcomeText(liveOutcome(flow.platformOutcome, activity.data), balance, maxMasked(discreet, flow.duffs === null, shown.masked())) : null;
  const done = fromPlatform ?? (step === "done" && flow.outcome ? outcomeText(flow.outcome, balance) : null);
  const errorText = flow.source === "platform" ? fromPlatformErrorText : topUpErrorText;
  return (
    <>
      <Sheet title={TOPUP_COPY.title} width="small" testId="topup-sheet" onClose={() => flow.close()}>
        <div data-testid="topup-body" data-step={step} data-source={flow.source} data-outcome={flow.platformOutcome?.state ?? flow.outcome?.state.kind}>
          {step === "amount" && <AmountStep flow={flow} walletId={walletId} costs={costs} network={network} amount={amount} discreet={discreet} />}
          {step === "confirm" && flow.source === "platform" && <PlatformConfirmStep flow={flow} walletId={walletId} name={name} amount={amount} discreet={discreet} masked={shown.masked()} />}
          {step === "confirm" && flow.source !== "platform" && <ConfirmStep flow={flow} walletId={walletId} costs={costs} amount={amount} discreet={discreet} />}
          {step === "running" && (
            <div role="status" className="topup-running">
              <p className="topup-title">
                {TOPUP_COPY.running.title}
                <span className="spinner" aria-hidden="true" />
              </p>
              <p className="sheet-text">{TOPUP_COPY.running.text}</p>
            </div>
          )}
          {done && (
            <div role="status">
              <p className="topup-title" data-testid="topup-outcome-title">
                {done.title}
              </p>
              <p className="sheet-text" data-testid="topup-outcome-text">
                {done.text}
              </p>
              <div className="sheet-actions">
                {fromPlatform?.move && (
                  <DashButton
                    variant="tintedBlue"
                    size="large"
                    data-testid="topup-move-to-core"
                    onClick={moveToCore}
                  >
                    {FROM_PLATFORM_COPY.held.action}
                  </DashButton>
                )}
                <DashButton variant="filledBlue" size="large" data-testid="topup-done" onClick={() => flow.close()}>
                  {TOPUP_COPY.done}
                </DashButton>
              </div>
            </div>
          )}
          {step === "refused" && (
            <>
              <p className="form-problem" role="alert" data-testid="topup-refusal">
                {errorText(flow.error, amount, discreet)}
              </p>
              <div className="sheet-actions">
                {amountCanChange(flow.error) && (
                  <DashButton variant="tintedGray" size="large" data-testid="topup-refused-back" onClick={() => flow.back()}>
                    {TOPUP_COPY.confirm.back}
                  </DashButton>
                )}
                {routesToRepair(flow.error) && (
                  <DashButton
                    variant="tintedBlue"
                    size="large"
                    data-testid="topup-repair"
                    onClick={() => {
                      flow.close();
                      navigate("tools", { toolsTab: "repair" });
                    }}
                  >
                    {TOPUP_COPY.repair}
                  </DashButton>
                )}
                <DashButton variant="filledBlue" size="large" data-testid="topup-close" onClick={() => flow.close()}>
                  {TOPUP_COPY.close}
                </DashButton>
              </div>
            </>
          )}
        </div>
      </Sheet>
      <GrantSheet grants={flow.grants} />
    </>
  );
}
