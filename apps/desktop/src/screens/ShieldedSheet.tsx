// Shielded funds on Home (shielded/flow.ts): the card with the balance and its actions, the sheet
// for Add to Shielded, Send and Move to Dash wallet (amount → quote → confirm → progress → outcome),
// and Receive's address with its QR. The card says "not active on this network yet" while the
// network does not take shielded transitions, and is not there while dw-app has no such commands.

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
import { GrantSheet } from "./tools/common";

const MEMO_BYTES = 36;

/** Formats for the sheet: Dash in the display unit, masked in discreet mode. */
function useFormats(network: NetworkInfo) {
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
        <DashButton variant="filledBlue" size="large" data-testid="shielded-go" why={syncing ? SHIELDED_SYNCING_TEXT : quote.isError ? C.confirm.quoteFailed : flow.running ? C.running.title : C.confirm.quoting} disabled={!q || quote.isError || quote.isFetching || flow.running || syncing} onClick={() => q && void flow.run(q.quote.grant)}>
          {C.confirm.go}
        </DashButton>
      </div>
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

export function ShieldedSheet({ flow, walletId, network, status }: { flow: ShieldedFlow; walletId: string; network: NetworkInfo; status: ShieldedStatus | null }) {
  useModel(flow);
  const fmt = useFormats(network);
  if (!flow.open) return <GrantSheet grants={flow.grants} />;
  const { step } = flow;
  return (
    <>
      <Sheet title={C.titles[flow.kind]} width="small" testId="shielded-sheet" onClose={() => flow.close()}>
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
      </Sheet>
      <GrantSheet grants={flow.grants} />
    </>
  );
}

function ReceiveSheet({ walletId, network, syncing, onClose }: { walletId: string; network: NetworkInfo; syncing: boolean; onClose: () => void }) {
  const b = useBackend();
  const fmt = useFormats(network);
  const address = useQuery({ queryKey: keys.shieldedAddress(walletId), queryFn: () => b.shieldedReceiveAddress(walletId), retry: false });
  return (
    <Sheet title={C.titles.receive} width="small" testId="shielded-receive-sheet" onClose={onClose}>
      <p className="sheet-text">{C.receive.text}</p>
      {address.isError ? (
        <p className="form-problem" role="alert" data-testid="shielded-receive-error">
          {isCode(address.error, SHIELDED_CODES.notSynced) ? C.receive.notSynced : shieldedErrorText(address.error, fmt.credits, fmt.duffs, undefined, syncing)}
        </p>
      ) : address.data ? (
        <div className="receive-body" data-testid="shielded-receive">
          <Qr text={address.data.address} label={`QR code of ${address.data.address}`} testId="shielded-receive-qr" />
          <p className="withdraw-to" data-testid="shielded-receive-address">
            <CopyValue value={address.data.address} label="Shielded address" mono />
          </p>
        </div>
      ) : (
        <p className="sheet-text" role="status">
          {C.receive.loading}
        </p>
      )}
      <div className="sheet-actions">
        <DashButton variant="filledBlue" size="large" data-testid="shielded-receive-close" onClick={onClose}>
          {C.close}
        </DashButton>
      </div>
    </Sheet>
  );
}

/** Home's Shielded card: the balance and the actions; not there while dw-app has no such commands. */
export function ShieldedCard({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const backend = useBackend();
  const status = useShieldedStatus(walletId);
  const flow = shieldedFor(backend, walletId);
  useModel(flow);
  const [receiving, setReceiving] = useState(false);
  const fmt = useFormats(network);
  const s = status.data ?? null;
  const sheet = <ShieldedSheet flow={flow} walletId={walletId} network={network} status={s} />;
  if (s === null) return sheet;
  const balance = shieldedDuffs(s);
  const busy = flow.running;
  const syncing = shieldedSyncing(s);
  const why = (empty: boolean) => (syncing ? SHIELDED_SYNCING_TEXT : busy ? C.card.why.busy : empty ? C.card.why.empty : null);
  // One visible line per distinct reason on the card: each disabled action is described by its own.
  const reasons = {
    receive: syncing ? SHIELDED_SYNCING_TEXT : null,
    send: why(!balance),
    add: why(false),
    out: why(!balance),
    finish: syncing ? SHIELDED_SYNCING_TEXT : busy ? C.card.why.busy : null,
  };
  const lines = s.active ? [...new Set(Object.values(reasons).filter((r): r is string => !!r))] : [];
  const lineId = (r: string | null) => (r ? `shielded-card-why-${lines.indexOf(r)}` : undefined);
  return (
    <>
      <section className="shielded-card card-l1" data-testid="shielded-card" data-active={s.active} aria-label={C.card.title}>
        <div className="shielded-card-body">
          <h2 className="platform-card-title">{C.card.title}</h2>
          <p className="platform-card-text" data-testid="shielded-card-text">
            {!s.active ? C.card.inactive : balance === null ? C.card.syncing : C.card.text}
          </p>
          {s.active && BigInt(s.pending) > 0n && (
            <p className="platform-card-text" data-testid="shielded-card-pending">
              {C.card.pending(fmt.credits(s.pending))}
            </p>
          )}
          {s.active && s.unresolvedSpends.length > 0 && (
            <p className="platform-card-text" data-testid="shielded-unresolved">
              {C.card.unresolved}
            </p>
          )}
          {s.pendingShields.map((p) => (
            <p key={p.txid} className="platform-card-text" data-testid="shielded-unfinished">
              {C.card.unfinished(fmt.duffs(p.duffs))}{" "}
              <DashButton variant="plainBlue" size="small" data-testid="shielded-finish" disabled={syncing || busy} why={reasons.finish} whyId={lineId(reasons.finish)} onClick={() => void flow.resume(p.txid)}>
                {C.card.finish}
              </DashButton>
            </p>
          ))}
          {lines.map((r, i) => (
            <p key={r} id={`shielded-card-why-${i}`} className="field-hint" data-testid="shielded-card-why">
              {r}
            </p>
          ))}
          {flow.unseen && (
            <p className="platform-card-text" data-testid="shielded-unseen">
              {C.unseen}{" "}
              <DashButton variant="plainBlue" size="small" data-testid="shielded-show" onClick={() => flow.begin(flow.kind)}>
                {C.show}
              </DashButton>
            </p>
          )}
        </div>
        {s.active && (
          <div className="shielded-card-actions">
            <DashButton variant="tintedBlue" size="small" data-testid="shielded-receive-open" disabled={syncing} why={reasons.receive} whyId={lineId(reasons.receive)} onClick={() => setReceiving(true)}>
              {C.card.receive}
            </DashButton>
            <DashButton variant="tintedBlue" size="small" data-testid="shielded-send" disabled={syncing || busy || !balance} why={reasons.send} whyId={lineId(reasons.send)} onClick={() => flow.begin("send")}>
              {C.card.send}
            </DashButton>
            <DashButton variant="tintedBlue" size="small" data-testid="shielded-add" disabled={syncing || busy} why={reasons.add} whyId={lineId(reasons.add)} onClick={() => flow.begin("add")}>
              {C.card.add}
            </DashButton>
            <DashButton variant="tintedBlue" size="small" data-testid="shielded-out" disabled={syncing || busy || !balance} why={reasons.out} whyId={lineId(reasons.out)} onClick={() => flow.begin("out")}>
              {C.card.out}
            </DashButton>
          </div>
        )}
      </section>
      {sheet}
      {receiving && <ReceiveSheet walletId={walletId} network={network} syncing={syncing} onClose={() => setReceiving(false)} />}
    </>
  );
}
