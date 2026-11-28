// Shielded funds (shielded/flow.ts): the steps Transfer shows for moves between balances and Pay
// shows for a send, and the address with its QR that Receive shows.

import { useEffect, useId, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { ShieldedStatus } from "../api/generated/ShieldedStatus";
import { CREDITS_PER_DUFF, creditsToDuffs, SHIELDED_CODES } from "../api/pending";
import { keys, shieldedDuffs, shieldedHold, useBackend, useShieldedBind, useRereadWhenBound, useShieldedQuote, useShieldedStatus, useShieldedSyncInfo } from "../api/queries";
import { useDebounced } from "../app/useDebounced";
import { ui, useUi } from "../app/ui";
import { DashButton } from "../components/basics";
import { CopyValue } from "../components/detail";
import { Sheet } from "../components/sheet";
import { useDisplay } from "../format/display";
import { percentText } from "../format/sync";
import { useT } from "../i18n";
import { feeWarning, feeWarningText } from "../format/feeWarning";
import { useModel } from "../onboarding/model";
import { isCode, SHIELDED_COPY as C, shieldedCanChange, shieldedErrorText, shieldedFor, type ShieldedFlow } from "../shielded/flow";
import { sendMax, sendMaxLine } from "../shielded/sendMax";
import { SHIELDED_SPENDS_COPY, shieldedSpendsOff, spendsOffText } from "../shielded/spends";
import { SHIELDED_SYNC_COPY, scanShare, shieldedReceiveGate, shieldedSyncing, shieldedSyncText, shieldedWait, shieldedWaitText } from "../shielded/syncing";
import { coreMaxNotes, TRANSFER_COPY } from "../transfer/model";
import { preparingText } from "../transfer/preparing";
import { useNowSecs } from "./masternodes/common";
import { Qr } from "./Receive";
import { GrantSheet } from "./tools/common";
import { platformHoldText } from "../api/platformHold";

const MEMO_BYTES = 36;

/** Formats for the sheet: Dash in the display unit with its full decimals (UX-SPEC §5.2, as Pay's sheets), masked in
 * discreet mode. Home's card passes `compact`. */
export function useFormats(network: NetworkInfo, style: "full" | "compact" = "full") {
  const shown = useDisplay(network);
  const discreet = useUi((s) => s.discreet);
  const duffs = (d: number) => (discreet ? shown.masked() : `${shown[style](d)} ${shown.name}`);
  return { shown, duffs, credits: (c: string) => duffs(creditsToDuffs(c)) };
}

function FormStep({ flow, network, status }: { flow: ShieldedFlow; network: NetworkInfo; status: ShieldedStatus | null }) {
  const tr = useT();
  const b = useBackend();
  const id = useId();
  const { shown, duffs: fmt, credits } = useFormats(network);
  const kind = flow.kind === "resume" ? "add" : flow.kind;
  const now = useNowSecs();
  const syncLine = kind === "send" ? shieldedSyncText(status, now) : null;
  const scanning = syncLine === SHIELDED_SYNC_COPY.scanning;
  const [line] = useState(() => sendMaxLine(b, flow));
  useModel(line);
  useEffect(() => () => line.dispose(), [line]);
  const discreet = useUi((s) => s.discreet);
  const [maxText, setMaxText] = useState<string | null>(null);
  // Max's figure is the balance: in discreet mode it shows masked until it is typed over.
  const mask = shown.masked();
  const hidden = discreet && flow.text !== "" && maxText === flow.text;
  const text = flow.text.trim();
  const checked = useDebounced(text, 300);
  const parsed = useQuery({ queryKey: ["parseAmount", shown.unit, checked], queryFn: () => b.parseAmount(checked, shown.unit), enabled: checked !== "", staleTime: Infinity });
  const amount = checked === text && typeof parsed.data === "number" ? parsed.data : null;
  const amountProblem = text === "" || checked !== text || parsed.isPending ? null : amount === null || amount <= 0 ? C.form.invalidAmount : null;
  const memoTooLong = new TextEncoder().encode(flow.memo).length > MEMO_BYTES;
  const to = flow.to.trim();
  const ready = amount !== null && amount > 0 && amountProblem === null && (kind !== "send" || (to !== "" && !memoTooLong));
  const available = shieldedDuffs(status);
  const hold = shieldedHold(status);
  // What Next waits for, said beside it: the same text the card gives while Shielded syncs.
  const syncing = shieldedSyncing(status);
  const spendsOff = shieldedSpendsOff(status);
  const waitText = shieldedWaitText(status);
  const maxRefusal = line.state.kind === "refused" ? (platformHoldText(line.state.error, false) ?? shieldedErrorText(line.state.error, credits, fmt, "send", shieldedWait(status), false, spendsOff)) : null;
  const maxWhy = spendsOff ? spendsOffText(kind) : syncing ? waitText : to === "" ? C.form.why.to : !available ? C.card.why.empty : null;
  const nextWhy = spendsOff
    ? spendsOffText(kind)
    : syncing
    ? waitText
    : amountProblem ?? (amount === null || amount <= 0 ? C.form.why.amount : kind === "send" && to === "" ? C.form.why.to : memoTooLong ? C.form.why.memo : null);
  const amountField = (
    <label className="form-field">
      {tr("transfer.amountIn", { unit: shown.name })}
      <input
        data-testid="shielded-amount"
        inputMode="decimal"
        autoFocus
        placeholder={shown.placeholder}
        value={hidden ? mask : flow.text}
        aria-invalid={amountProblem !== null || maxRefusal !== null}
        aria-describedby={`${id}-amount-problem`}
        onFocus={(e) => hidden && e.target.select()}
        onChange={(e) => {
          line.set(null);
          flow.setText(hidden ? e.target.value.split(mask).join("") : e.target.value);
        }}
      />
    </label>
  );
  return (
    <form
      className="topup-form"
      onSubmit={(e) => {
        e.preventDefault();
        if (!ready || syncing || spendsOff) return;
        if (kind === "add") flow.next({ kind: "add", duffs: amount });
        else {
          const credits = String(BigInt(amount) * CREDITS_PER_DUFF);
          flow.next(kind === "send" ? { kind: "send", credits, to, memo: flow.memo.trim() || null } : { kind: kind === "toPlatform" ? "toPlatform" : "out", credits });
        }
      }}
    >
      <p className="sheet-text">{C.form[kind]}</p>
      {kind !== "add" && available !== null && <p className="field-hint">{C.form.available(fmt(available))}</p>}
      {kind !== "add" && hold && (
        <p className="field-hint" data-testid="shielded-held">
          {C.held(fmt(hold.duffs), hold.count)}
        </p>
      )}
      {kind === "send" && (
        <span className="sr-only" role="status" data-testid="shielded-sync-status">
          {scanning ? syncLine : ""}
        </span>
      )}
      {syncLine && (
        <p className="field-hint" data-testid="shielded-sync" aria-hidden={scanning || undefined}>
          {syncLine}
        </p>
      )}
      {kind === "send" ? (
        <div className="transfer-amount-row">
          {amountField}
          <DashButton
            variant="tintedBlue"
            size="medium"
            type="button"
            data-testid="shielded-max"
            aria-busy={line.maxing}
            disabled={maxWhy !== null || line.maxing || flow.running}
            why={maxWhy}
            whyId={`${id}-max-why`}
            title={line.maxing ? TRANSFER_COPY.why.max : undefined}
            onClick={() =>
              available &&
              void sendMax(line, available).then((duffs) => {
                if (duffs === null) return;
                const filled = shown.full(duffs);
                setMaxText(filled);
                flow.setText(filled);
              })
            }
          >
            {TRANSFER_COPY.max}
            <span className="spinner" aria-hidden="true" style={{ visibility: line.maxing ? "visible" : "hidden" }} />
          </DashButton>
        </div>
      ) : (
        amountField
      )}
      {kind === "send" && maxWhy && (
        <p className="field-hint" id={`${id}-max-why`} data-testid="shielded-max-why">
          {maxWhy}
        </p>
      )}
      <p className="form-problem" id={`${id}-amount-problem`} role="alert" data-testid="shielded-amount-problem">
        {amountProblem ?? maxRefusal}
      </p>
      {kind === "send" && (
        <>
          <label className="form-field">
            {C.form.to}
            <input
              data-testid="shielded-to"
              spellCheck={false}
              autoComplete="off"
              value={flow.to}
              onChange={(e) => {
                line.set(null);
                flow.setTo(e.target.value);
              }}
            />
          </label>
          <label className="form-field">
            {C.form.memo}
            <input data-testid="shielded-memo" aria-invalid={memoTooLong} aria-describedby={`${id}-memo-hint`} value={flow.memo} onChange={(e) => flow.setMemo(e.target.value)} />
          </label>
          <p className={memoTooLong ? "form-problem" : "field-hint"} id={`${id}-memo-hint`} role={memoTooLong ? "alert" : undefined}>
            {C.form.memoHint}
          </p>
        </>
      )}
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" type="button" data-testid="shielded-cancel" onClick={() => flow.close()}>
          {C.form.cancel}
        </DashButton>
        <DashButton variant="filledBlue" size="large" type="submit" data-testid="shielded-next" disabled={!ready || syncing || spendsOff} why={nextWhy}>
          {C.form.next}
        </DashButton>
      </div>
    </form>
  );
}

/** A confirm row; an amount reads right-aligned in tabular digits, so the decimals line up; an `address` does not. */
function Row({ label, value, testId, total, address }: { label: string; value: string; testId: string; total?: boolean; address?: boolean }) {
  return (
    <div className={`detail-row ${total ? "topup-total" : ""}`}>
      <dt className="detail-label">{label}</dt>
      <dd className={`detail-value ${address ? "" : "detail-amount"}`} data-testid={testId}>
        <bdi dir="ltr">{value}</bdi>
      </dd>
    </div>
  );
}

function ConfirmStep({ flow, walletId, status, fmt }: { flow: ShieldedFlow; walletId: string; status: ShieldedStatus | null; fmt: ReturnType<typeof useFormats> }) {
  const tr = useT();
  const quote = useShieldedQuote(walletId, flow.request);
  const q = quote.data;
  const coreMax = flow.request?.kind === "add" && !!flow.request.all;
  const { duffs, credits } = fmt;
  const kind = flow.kind;
  const syncing = shieldedSyncing(status);
  const spendsOff = shieldedSpendsOff(status);
  const spendBlocked = spendsOff && kind !== "resume";
  // DEC-1057: a send or move from a wallet with an unrecorded spend needs the user's confirmation
  // first, and passes it to the engine; a shield does not.
  const unrecorded = kind !== "add" && (status?.unrecordedSpends.length ?? 0) > 0;
  const [asking, setAsking] = useState(false);
  // The fees against what is added or sent: for an add, the Core fee and the pool fee.
  const warning = !q ? null : q.kind === "add" ? feeWarning(BigInt(q.quote.feeDuffs) * CREDITS_PER_DUFF + BigInt(q.quote.poolFeeCredits), BigInt(q.quote.duffs) * CREDITS_PER_DUFF) : feeWarning(BigInt(q.quote.feeCredits), BigInt(q.quote.credits));
  return (
    <>
      {quote.isError ? (
        <p className="form-problem" role="alert" data-testid="shielded-quote-error">
          {platformHoldText(quote.error, false) ?? shieldedErrorText(quote.error, credits, duffs, kind, shieldedWait(status), coreMax, spendsOff)}
        </p>
      ) : !q ? (
        <p className="sheet-text" role="status">
          {C.confirm.quoting}
        </p>
      ) : (
        <dl className="detail-list topup-quote" data-testid="shielded-quote">
          {q.kind === "add" ? (
            <>
              <Row label={C.confirm.lock} value={duffs(q.quote.duffs)} testId="shielded-quote-amount" />
              <Row label={C.confirm.coreFee} value={duffs(q.quote.feeDuffs)} testId="shielded-quote-fee" />
              <Row label={C.confirm.totalFromWallet} value={duffs(q.quote.totalDuffs)} testId="shielded-quote-total" total />
              <Row label={C.confirm.poolFee} value={credits(q.quote.poolFeeCredits)} testId="shielded-quote-pool-fee" />
              <Row label={C.confirm.gain} value={credits(q.quote.credits)} testId="shielded-quote-gain" />
            </>
          ) : (
            <>
              <Row label={C.confirm.amount} value={credits(q.quote.credits)} testId="shielded-quote-amount" />
              <Row label={C.confirm.fee} value={credits(q.quote.feeCredits)} testId="shielded-quote-fee" />
              <Row label={C.confirm.total} value={credits(q.quote.totalCredits)} testId="shielded-quote-total" total />
              {q.quote.expectedDuffs !== null && <Row label={C.confirm.receive} value={duffs(q.quote.expectedDuffs)} testId="shielded-quote-receive" />}
              {flow.request?.kind === "send" && <Row label={C.confirm.to} value={flow.request.to} testId="shielded-quote-to" address />}
            </>
          )}
        </dl>
      )}
      {q?.kind === "add" && coreMax && (
        <div className="core-max-confirm" data-testid="core-max-confirm">
          <p className="sheet-text">{tr("transfer.shielded.confirm.maxAll", { total: duffs(q.quote.totalDuffs), fee: duffs(q.quote.feeDuffs) })}</p>
          {coreMaxNotes(q.quote.max, duffs).map((note) => (
            <p key={note} className="field-hint" data-testid="core-max-note">
              {note}
            </p>
          ))}
        </div>
      )}
      {q?.kind === "add" && (
        <p className="field-hint" data-testid="shielded-pool-fee-note">
          {C.confirm.poolFeeFixed(credits(q.quote.poolFeeCredits))}
        </p>
      )}
      {warning && (
        <p className="fee-warning" role="alert" data-testid="shielded-fee-warning">
          {feeWarningText(warning)}
        </p>
      )}
      {kind !== "add" && (
        <p className="field-hint" data-testid="shielded-slow">
          {C.confirm.slow} {status && !status.proverReady ? C.confirm.slowFirst : ""}
        </p>
      )}
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" data-testid="shielded-back" disabled={flow.running} onClick={() => flow.back()}>
          {C.confirm.back}
        </DashButton>
        <DashButton variant="filledBlue" size="large" data-testid="shielded-go" why={spendBlocked ? spendsOffText(kind) : syncing ? shieldedWaitText(status) : quote.isError ? C.confirm.quoteFailed : flow.running ? C.running.title : C.confirm.quoting} disabled={!q || quote.isError || quote.isFetching || flow.running || syncing || spendBlocked} onClick={() => q && (unrecorded ? setAsking(true) : void flow.run(q.quote.grant))}>
          {C.confirm.go}
        </DashButton>
      </div>
      {asking && q && (
        <Sheet title={C.unrecorded.title} width="small" testId="shielded-unrecorded-dialog" onClose={() => setAsking(false)}>
          <p className="form-problem" role="alert" data-testid="shielded-unrecorded-text">
            {C.unrecorded.text}
          </p>
          <div className="sheet-actions">
            <DashButton variant="tintedGray" size="large" data-testid="shielded-unrecorded-back" onClick={() => setAsking(false)}>
              {C.unrecorded.back}
            </DashButton>
            <DashButton
              variant="filledBlue"
              size="large"
              data-testid="shielded-unrecorded-go"
              onClick={() => {
                setAsking(false);
                void flow.run(q.quote.grant, true);
              }}
            >
              {C.unrecorded.go[kind === "send" ? "send" : "out"]}
            </DashButton>
          </div>
        </Sheet>
      )}
    </>
  );
}

function Outcome({ flow, fmt }: { flow: ShieldedFlow; fmt: ReturnType<typeof useFormats> }) {
  const r = flow.result;
  if (!r) return null;
  let title: string;
  let text: string;
  if (r.kind === "shield") {
    const s = r.outcome.state;
    if (s.kind === "applied") ({ title, text } = { title: C.done.add.title, text: C.done.add.text(fmt.credits(s.balance)) });
    else if (s.kind === "proofWaiting") return null;
    else if (s.kind === "consumptionUnknown") ({ title, text } = C.done.checking);
    else ({ title, text } = { title: C.done.notApplied.title, text: C.done.notApplied.text(s.error) });
  } else if (r.outcome.state === "pending") {
    return null;
  } else if (flow.kind === "toPlatform") {
    ({ title } = C.done.toPlatform);
    text = C.done.toPlatform.text(r.outcome.to);
  } else if (flow.kind === "out") {
    ({ title } = C.done.out);
    text = C.done.out.text(fmt.credits(r.outcome.credits));
  } else {
    ({ title } = C.done.send);
    text = C.done.send.text(fmt.credits(r.outcome.credits));
  }
  return (
    <div role="status">
      <p className="topup-title" data-testid="shielded-outcome-title">
        {title}
      </p>
      <p className="sheet-text shielded-outcome-text" data-testid="shielded-outcome-text">
        {text}
      </p>
      <div className="sheet-actions">
        <DashButton variant="filledBlue" size="large" data-testid="shielded-done" onClick={() => flow.close()}>
          {C.close}
        </DashButton>
      </div>
    </div>
  );
}

/** The steps of a Shielded move: its form, confirmation, progress and outcome. */
export function ShieldedBody({ flow, walletId, network, status }: { flow: ShieldedFlow; walletId: string; network: NetworkInfo; status: ShieldedStatus | null }) {
  useModel(flow);
  const fmt = useFormats(network);
  const { step } = flow;
  return (
    <div data-testid="shielded-body" data-step={step} data-kind={flow.kind}>
      {step === "form" && <FormStep flow={flow} network={network} status={status} />}
      {step === "confirm" && <ConfirmStep flow={flow} walletId={walletId} status={status} fmt={fmt} />}
      {step === "running" && (
        <div role="status" className="topup-running">
          <p className="topup-title">
            {C.running.title}
            <span className="spinner" aria-hidden="true" />
          </p>
          <p className="sheet-text" data-testid="shielded-progress">
            {flow.kind === "add" || flow.kind === "resume" ? C.running.lock : C.running.prove}
          </p>
          {flow.preparing.shown && (
            <p className="sheet-text" data-testid="transfer-preparing">
              {preparingText()}
            </p>
          )}
        </div>
      )}
      {step === "done" && <Outcome flow={flow} fmt={fmt} />}
      {step === "refused" && (
        <>
          <p className="form-problem" role="alert" data-testid="shielded-refusal">
            {shieldedErrorText(flow.error, fmt.credits, fmt.duffs, flow.kind, shieldedWait(status), flow.request?.kind === "add" && !!flow.request.all, shieldedSpendsOff(status))}
          </p>
          <div className="sheet-actions">
            {shieldedCanChange(flow.error) && flow.request?.kind !== "resume" && (
              <DashButton variant="tintedGray" size="large" data-testid="shielded-refused-back" onClick={() => flow.back()}>
                {C.confirm.back}
              </DashButton>
            )}
            <DashButton variant="filledBlue" size="large" data-testid="shielded-close" onClick={() => flow.close()}>
              {C.close}
            </DashButton>
          </div>
        </>
      )}
    </div>
  );
}

/** Send from Shielded (Pay's link): the moves between the wallet's own balances are Transfer's
 * (TransferSheet.tsx). The flow's passphrase sheet is always here, whichever sheet asks. */
export function ShieldedSheet({ flow, walletId, network, status }: { flow: ShieldedFlow; walletId: string; network: NetworkInfo; status: ShieldedStatus | null }) {
  useModel(flow);
  if (!flow.open || flow.kind !== "send") return <GrantSheet grants={flow.grants} />;
  return (
    <>
      <Sheet title={C.titles.send} width="small" testId="shielded-sheet" onClose={() => flow.close()}>
        <ShieldedBody flow={flow} walletId={walletId} network={network} status={status} />
      </Sheet>
      <GrantSheet grants={flow.grants} />
    </>
  );
}

/** Receive in Shielded's content (Receive's Shielded tab, the card's body). The address is the
 * viewing key's, so it shows as soon as the key is bound, while the first scan still runs, with that
 * scan's progress under it. Without the key (the first bind needs an unlock, or the wallet is
 * watch-only) it says why and shows no address. `enabled`: read the address now. */
export function useShieldedReceiveContent(walletId: string, network: NetworkInfo, enabled = true) {
  const tr = useT();
  const b = useBackend();
  const fmt = useFormats(network);
  // The sync read answers while a pass holds the note store.
  const infoRead = useShieldedSyncInfo(walletId, enabled);
  const status = infoRead.data ?? null;
  const gate = shieldedReceiveGate(status);
  const spendsOff = shieldedSpendsOff(status);
  useShieldedBind(walletId, status, enabled);
  const settled = infoRead.data !== undefined || infoRead.isError;
  const address = useQuery({ queryKey: keys.shieldedAddress(walletId), queryFn: () => b.shieldedReceiveAddress(walletId), retry: false, enabled: enabled && settled && gate === "address" && !spendsOff });
  useRereadWhenBound(status?.account === "bound", enabled, address.isError, address.refetch);
  const firstScan = !!status?.active && !status.established;
  const percent = firstScan ? scanShare(status?.scan) : null;
  const scanning = firstScan && (gate === "address" || gate === "binding");
  const progress = scanning ? (
    <p className="field-hint" role="status" data-testid="shielded-receive-progress">
      {SHIELDED_SYNC_COPY.progress(percent === null ? null : percentText(percent))}
    </p>
  ) : null;
  const waiting = address.isError && isCode(address.error, SHIELDED_CODES.notSynced);
  const loading = (
    <p className="sheet-text" role="status">
      {C.receive.loading}
    </p>
  );
  const before = (() => {
    if (spendsOff)
      return (
        <p className="field-hint" data-testid="shielded-receive-spends-off">
          {SHIELDED_SPENDS_COPY.receive}
        </p>
      );
    if (gate === "unlock") {
      // Unlocked for mixing only: the same full unlock, named as Pay names it.
      const mixingOnly = status?.account === "mixingOnly";
      return (
        <div className="shielded-receive-why" data-testid="shielded-receive-locked">
          <p className="sheet-text">{mixingOnly ? C.receive.mixingOnly : C.receive.locked}</p>
          <DashButton variant="tintedBlue" size="small" data-testid="shielded-receive-unlock" onClick={() => ui.set({ dialog: "unlock" })}>
            {mixingOnly ? tr("shell.status.unlockFully") : C.receive.unlock}
          </DashButton>
        </div>
      );
    }
    if (gate === "watchOnly")
      return (
        <p className="sheet-text" data-testid="shielded-receive-watch-only">
          {C.receive.watchOnly}
        </p>
      );
    if (gate === "binding" || waiting) return progress ?? loading;
    if (address.isError)
      return (
        <p className="form-problem" role="alert" data-testid="shielded-receive-error">
          {platformHoldText(address.error, false) ?? shieldedErrorText(address.error, fmt.credits, fmt.duffs)}
        </p>
      );
    if (address.data) return <Qr text={address.data.address} label={tr("transfer.shielded.receive.qr", { address: address.data.address })} testId="shielded-receive-qr" />;
    return loading;
  })();
  const shown = !spendsOff && gate === "address" && !address.isError && address.data;
  return {
    text: <p className="sheet-text">{C.receive.text}</p>,
    before,
    after: shown ? (
      <>
        <p className="withdraw-to" data-testid="shielded-receive-address">
          <CopyValue value={address.data.address} action={tr("transfer.shielded.receive.copyAddress")} mono />
        </p>
        {scanning && (
          <div className="shielded-receive-scan" data-testid="shielded-receive-scan">
            {progress}
            <p className="field-hint">{C.receive.early}</p>
          </div>
        )}
      </>
    ) : null,
  };
}

export function ShieldedReceiveBody({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const content = useShieldedReceiveContent(walletId, network);
  return <>{content.text}<div className="receive-body" data-testid="shielded-receive">{content.before}{content.after}</div></>;
}

/** Receive in Shielded (Receive's page): collapsed like Receive on Platform; open, the Shielded address
 * and its QR, or why there is none yet. There while Shielded is active on this network. */
export function ShieldedReceiveCard({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const status = useShieldedStatus(walletId).data ?? null;
  const [open, setOpen] = useState(false);
  const id = useId();
  if (!status?.active) return null;
  return (
    <section className="disclosure card-l1 platform-receive" data-testid="shielded-receive-section" aria-labelledby={`${id}-toggle`}>
      <button type="button" id={`${id}-toggle`} className="disclosure-toggle" data-testid="shielded-receive-open" aria-expanded={open} aria-controls={`${id}-body`} onClick={() => setOpen(!open)}>
        {C.titles.receive}
      </button>
      {open && (
        <div className="platform-receive-body" id={`${id}-body`} data-testid="shielded-receive-body">
          <ShieldedReceiveBody walletId={walletId} network={network} />
        </div>
      )}
    </section>
  );
}

/** Send from Shielded (Pay's link): the Shielded send sheet, with its own command and grant. There
 * while Shielded is active on this network; it waits, and says why, as the card's Send did. */
export function ShieldedSendLink({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const backend = useBackend();
  const flow = shieldedFor(backend, walletId);
  useModel(flow);
  const status = useShieldedStatus(walletId, flow.open).data ?? null;
  const sheet = <ShieldedSheet flow={flow} walletId={walletId} network={network} status={status} />;
  if (!status?.active) return sheet;
  const why = sendWhy(flow, status);
  return (
    <>
      <p className="pay-shielded" data-testid="pay-shielded">
        <DashButton variant="plainBlue" size="medium" data-testid="shielded-send" disabled={why !== null} why={why} onClick={() => flow.begin("send")}>
          {C.titles.send}
        </DashButton>
      </p>
      {sheet}
    </>
  );
}

/** Why Send from Shielded waits: this build cannot spend from Shielded, syncing, a call running, another kind's result not yet seen (a
 * Transfer move, opened from Transfer), or nothing to send; `null` when it can open. */
function sendWhy(flow: ShieldedFlow, status: ShieldedStatus): string | null {
  if (shieldedSpendsOff(status) && !(flow.kind === "send" && (flow.running || flow.unseen))) return SHIELDED_SPENDS_COPY.unavailable;
  return shieldedWaitText(status) ?? (flow.running ? C.card.why.busy : flow.unseen && flow.kind !== "send" ? C.unseenSend : !shieldedDuffs(status) ? C.card.why.empty : null);
}

/** Pay's Shielded address: Send from Shielded, opened with the address and amount typed in Pay (the
 * page's link holds the sheet). It waits, and says why, as the link does. */
export function ShieldedPayButton({ walletId, status, to, text }: { walletId: string; status: ShieldedStatus; to: string; text: string }) {
  const flow = shieldedFor(useBackend(), walletId);
  useModel(flow);
  const why = sendWhy(flow, status);
  return (
    <DashButton variant="tintedBlue" size="small" data-testid="pay-shielded-route" disabled={why !== null} why={why} onClick={() => flow.sendTo(to, text)}>
      {C.titles.send}
    </DashButton>
  );
}
