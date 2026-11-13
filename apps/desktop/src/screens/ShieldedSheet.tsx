// Shielded funds (shielded/flow.ts): Home's attention rows; the steps Transfer shows for moves
// between balances and Pay shows for a send; and the address with its QR that Receive shows.

import { useId, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { ShieldedStatus } from "../api/generated/ShieldedStatus";
import { CREDITS_PER_DUFF, creditsToDuffs, SHIELDED_CODES } from "../api/pending";
import { keys, shieldedDuffs, useBackend, useShieldedQuote, useShieldedStatus } from "../api/queries";
import { useDebounced } from "../app/useDebounced";
import { useUi } from "../app/ui";
import { DashButton } from "../components/basics";
import { CopyValue } from "../components/detail";
import { Sheet } from "../components/sheet";
import { useDisplay } from "../format/display";
import { feeWarning, feeWarningText } from "../format/feeWarning";
import { useModel } from "../onboarding/model";
import { isCode, SHIELDED_COPY as C, shieldedCanChange, shieldedErrorText, shieldedFor, type ShieldedFlow } from "../shielded/flow";
import { SHIELDED_SYNCING_TEXT, shieldedSyncing } from "../shielded/syncing";
import { Qr } from "./Receive";
import { shieldedRoute, transferFor } from "../transfer/model";
import { GrantSheet } from "./tools/common";

const MEMO_BYTES = 36;

/** Formats for the sheet: Dash in the display unit, masked in discreet mode. */
export function useFormats(network: NetworkInfo) {
  const shown = useDisplay(network);
  const discreet = useUi((s) => s.discreet);
  const duffs = (d: number) => (discreet ? shown.masked() : `${shown.compact(d)} ${shown.name}`);
  return { shown, duffs, credits: (c: string) => duffs(creditsToDuffs(c)) };
}

function FormStep({ flow, network, status }: { flow: ShieldedFlow; network: NetworkInfo; status: ShieldedStatus | null }) {
  const b = useBackend();
  const id = useId();
  const { shown, duffs: fmt } = useFormats(network);
  const kind = flow.kind === "resume" ? "add" : flow.kind;
  const text = flow.text.trim();
  const checked = useDebounced(text, 300);
  const parsed = useQuery({ queryKey: ["parseAmount", shown.unit, checked], queryFn: () => b.parseAmount(checked, shown.unit), enabled: checked !== "", staleTime: Infinity });
  const amount = checked === text && typeof parsed.data === "number" ? parsed.data : null;
  const amountProblem = text === "" || checked !== text || parsed.isPending ? null : amount === null || amount <= 0 ? C.form.invalidAmount : null;
  const memoTooLong = new TextEncoder().encode(flow.memo).length > MEMO_BYTES;
  const to = flow.to.trim();
  const ready = amount !== null && amount > 0 && amountProblem === null && (kind !== "send" || (to !== "" && !memoTooLong));
  const available = shieldedDuffs(status);
  // What Next waits for, said beside it: the same text the card gives while Shielded syncs.
  const syncing = shieldedSyncing(status);
  const nextWhy = syncing
    ? SHIELDED_SYNCING_TEXT
    : amountProblem ?? (amount === null || amount <= 0 ? C.form.why.amount : kind === "send" && to === "" ? C.form.why.to : memoTooLong ? C.form.why.memo : null);
  return (
    <form
      className="topup-form"
      onSubmit={(e) => {
        e.preventDefault();
        if (!ready || syncing) return;
        if (kind === "add") flow.next({ kind: "add", duffs: amount });
        else {
          const credits = String(BigInt(amount) * CREDITS_PER_DUFF);
          flow.next(kind === "send" ? { kind: "send", credits, to, memo: flow.memo.trim() || null } : { kind: "out", credits });
        }
      }}
    >
      <p className="sheet-text">{C.form[kind]}</p>
      {kind !== "add" && available !== null && <p className="field-hint">{C.form.available(fmt(available))}</p>}
      <label className="form-field">
        {C.form.amount} ({shown.name})
        <input
          data-testid="shielded-amount"
          inputMode="decimal"
          autoFocus
          placeholder={shown.placeholder}
          value={flow.text}
          aria-invalid={amountProblem !== null}
          aria-describedby={`${id}-amount-problem`}
          onChange={(e) => flow.setText(e.target.value)}
        />
      </label>
      <p className="form-problem" id={`${id}-amount-problem`} role="alert" data-testid="shielded-amount-problem">
        {amountProblem}
      </p>
      {kind === "send" && (
        <>
          <label className="form-field">
            {C.form.to}
            <input data-testid="shielded-to" spellCheck={false} autoComplete="off" value={flow.to} onChange={(e) => flow.setTo(e.target.value)} />
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
        <DashButton variant="filledBlue" size="large" type="submit" data-testid="shielded-next" disabled={!ready || syncing} why={nextWhy}>
          {C.form.next}
        </DashButton>
      </div>
    </form>
  );
}

function Row({ label, value, testId, total }: { label: string; value: string; testId: string; total?: boolean }) {
  return (
    <div className={`detail-row ${total ? "topup-total" : ""}`}>
      <dt className="detail-label">{label}</dt>
      <dd className="detail-value" data-testid={testId}>
        {value}
      </dd>
    </div>
  );
}

function ConfirmStep({ flow, walletId, status, fmt }: { flow: ShieldedFlow; walletId: string; status: ShieldedStatus | null; fmt: ReturnType<typeof useFormats> }) {
  const quote = useShieldedQuote(walletId, flow.request);
  const q = quote.data;
  const { duffs, credits } = fmt;
  const kind = flow.kind;
  const syncing = shieldedSyncing(status);
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
          {shieldedErrorText(quote.error, credits, duffs, kind, syncing)}
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
              {flow.request?.kind === "send" && <Row label={C.confirm.to} value={flow.request.to} testId="shielded-quote-to" />}
            </>
          )}
        </dl>
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
        <DashButton variant="filledBlue" size="large" data-testid="shielded-go" why={syncing ? SHIELDED_SYNCING_TEXT : quote.isError ? C.confirm.quoteFailed : flow.running ? C.running.title : C.confirm.quoting} disabled={!q || quote.isError || quote.isFetching || flow.running || syncing} onClick={() => q && (unrecorded ? setAsking(true) : void flow.run(q.quote.grant))}>
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
    else if (s.kind === "proofWaiting") ({ title, text } = C.done.waiting);
    else if (s.kind === "consumptionUnknown") ({ title, text } = C.done.checking);
    else ({ title, text } = { title: C.done.notApplied.title, text: C.done.notApplied.text(s.error) });
  } else if (r.outcome.state === "pending") {
    ({ title, text } = C.done.unconfirmed);
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
      <p className="sheet-text" data-testid="shielded-outcome-text">
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
            {flow.kind === "send" || flow.kind === "out" ? C.running.prove : C.running.lock}
          </p>
        </div>
      )}
      {step === "done" && <Outcome flow={flow} fmt={fmt} />}
      {step === "refused" && (
        <>
          <p className="form-problem" role="alert" data-testid="shielded-refusal">
            {shieldedErrorText(flow.error, fmt.credits, fmt.duffs, flow.kind, shieldedSyncing(status))}
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

/** The Shielded address with its QR (Receive's Shielded section). */
export function useShieldedReceiveContent(walletId: string, network: NetworkInfo, enabled = true) {
  const b = useBackend();
  const fmt = useFormats(network);
  const address = useQuery({ queryKey: keys.shieldedAddress(walletId), queryFn: () => b.shieldedReceiveAddress(walletId), retry: false, enabled });
  return {
    text: <p className="sheet-text">{C.receive.text}</p>,
    before: <>
      {address.isError ? (
        <p className="form-problem" role="alert" data-testid="shielded-receive-error">
          {isCode(address.error, SHIELDED_CODES.notSynced) ? C.receive.notSynced : shieldedErrorText(address.error, fmt.credits, fmt.duffs)}
        </p>
      ) : address.data ? (
        <Qr text={address.data.address} label={`QR code of ${address.data.address}`} testId="shielded-receive-qr" />
      ) : (
        <p className="sheet-text" role="status">
          {C.receive.loading}
        </p>
      )}
    </>,
    after: !address.isError && address.data ? (
      <p className="withdraw-to" data-testid="shielded-receive-address">
        <CopyValue value={address.data.address} label="Shielded address" mono />
      </p>
    ) : null,
  };
}

export function ShieldedReceiveBody({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const content = useShieldedReceiveContent(walletId, network);
  return <>{content.text}<div className="receive-body" data-testid="shielded-receive">{content.before}{content.after}</div></>;
}

/** Home's Shielded attention rows, with the send and grant sheets kept mounted. */
export function ShieldedCard({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const backend = useBackend();
  const status = useShieldedStatus(walletId);
  const flow = shieldedFor(backend, walletId);
  const transfer = transferFor(backend, walletId);
  useModel(flow);
  const fmt = useFormats(network);
  const whyId = useId();
  const s = status.data ?? null;
  const busy = flow.running;
  const syncing = s !== null && shieldedSyncing(s);
  const finishWhy = syncing ? SHIELDED_SYNCING_TEXT : busy ? C.card.why.busy : null;
  const whyLine = finishWhy && s !== null && s.pendingShields.length > 0 ? finishWhy : null;
  const pending = s !== null && s.active && BigInt(s.pending) > 0n;
  const attention = s !== null && (pending || s.unrecordedSpends.length > 0 || (s.active && s.unresolvedSpends.length > 0) || s.pendingShields.length > 0 || flow.unseen);
  const showRoute = shieldedRoute(flow.kind);
  // The sheet keeps its slot whether or not the section shows, so an open grant prompt is not remounted.
  return (
    <>
      {s !== null && attention && (
        <section className="shielded-card card-l1" data-testid="shielded-card" data-active={s.active} aria-label={C.card.title}>
          <div className="shielded-card-body">
            <h2 className="platform-card-title">{C.card.title}</h2>
            {pending && (
              <p className="platform-card-text" data-testid="shielded-card-pending">
                {C.card.pending(fmt.credits(s.pending))}
              </p>
            )}
            {s.unrecordedSpends.length > 0 && (
              <p className="form-problem" role="alert" data-testid="shielded-unrecorded">
                {C.card.unrecorded}
              </p>
            )}
            {s.active && s.unresolvedSpends.length > 0 && (
              <p className="platform-card-text" data-testid="shielded-unresolved">
                {C.card.unresolved}
              </p>
            )}
            {s.pendingShields.map((p) => (
              <p key={p.txid} className="platform-card-text" data-testid={p.consumptionUnknown ? "shielded-checking" : "shielded-unfinished"}>
                {p.consumptionUnknown ? (
                  <>
                    <strong>{C.done.checking.title}.</strong> {C.done.checking.text}
                  </>
                ) : (
                  C.card.unfinished(fmt.duffs(p.duffs))
                )}{" "}
                <DashButton variant="plainBlue" size="small" data-testid="shielded-finish" disabled={syncing || busy} why={finishWhy} whyId={whyLine ? whyId : undefined} onClick={() => void flow.resume(p.txid)}>
                  {p.consumptionUnknown ? C.card.checkAgain : C.card.finish}
                </DashButton>
              </p>
            ))}
            {whyLine && (
              <p id={whyId} className="field-hint" data-testid="shielded-card-why">
                {whyLine}
              </p>
            )}
            {flow.unseen && (
              <p className="platform-card-text" data-testid="shielded-unseen">
                {C.unseen}{" "}
                <DashButton variant="plainBlue" size="small" data-testid="shielded-show" onClick={() => (showRoute === "send" ? flow.begin("send") : transfer.start(showRoute))}>
                  {C.show}
                </DashButton>
              </p>
            )}
          </div>
        </section>
      )}
      <ShieldedSheet flow={flow} walletId={walletId} network={network} status={s} />
    </>
  );
}

/** Receive in Shielded (Receive's page): collapsed like Receive on Platform; open, the Shielded address
 * and its QR. There while Shielded is active on this network; it waits while Shielded syncs. */
export function ShieldedReceiveCard({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const status = useShieldedStatus(walletId).data ?? null;
  const [open, setOpen] = useState(false);
  const id = useId();
  if (!status?.active) return null;
  const syncing = shieldedSyncing(status);
  return (
    <section className="disclosure card-l1 platform-receive" data-testid="shielded-receive-section" aria-labelledby={`${id}-toggle`}>
      <button
        type="button"
        id={`${id}-toggle`}
        className="disclosure-toggle"
        data-testid="shielded-receive-open"
        aria-expanded={open && !syncing}
        aria-controls={`${id}-body`}
        disabled={syncing}
        title={syncing ? SHIELDED_SYNCING_TEXT : undefined}
        aria-describedby={syncing ? `${id}-why` : undefined}
        onClick={() => setOpen(!open)}
      >
        {C.titles.receive}
      </button>
      {syncing && (
        <p className="field-hint disclosure-why" id={`${id}-why`} data-testid="shielded-receive-why">
          {SHIELDED_SYNCING_TEXT}
        </p>
      )}
      {open && !syncing && (
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
  const status = useShieldedStatus(walletId).data ?? null;
  const flow = shieldedFor(backend, walletId);
  useModel(flow);
  const sheet = <ShieldedSheet flow={flow} walletId={walletId} network={network} status={status} />;
  if (!status?.active) return sheet;
  const syncing = shieldedSyncing(status);
  const why = syncing ? SHIELDED_SYNCING_TEXT : flow.running ? C.card.why.busy : !shieldedDuffs(status) ? C.card.why.empty : null;
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
