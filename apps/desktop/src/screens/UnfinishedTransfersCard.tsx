// Unfinished transfers on Home (recovery/model.ts): the card that offers to finish what a wallet
// restore found locked for Shielded, and the sheet with one row per lock.

import { useEffect, useState } from "react";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { useBackend, useShieldedStatus, useUnfinishedTransfers } from "../api/queries";
import { DashButton } from "../components/basics";
import { CopyValue } from "../components/detail";
import { Sheet } from "../components/sheet";
import { shortId } from "../dashpay/identities";
import { useModel } from "../onboarding/model";
import { cardView, lockKey, RECOVERY_COPY as C, recoveryFor, resultText, sheetRows, withoutPendingShields, type CardAction, type RecoveryModel } from "../recovery/model";
import { shieldedErrorText } from "../shielded/flow";
import { useFormats } from "./ShieldedSheet";
import { GrantSheet } from "./tools/common";

const ACTION_LABEL: Record<CardAction, string> = { finish: C.finish, finishAll: C.finishAll, checkAgain: C.checkAgain, details: C.details, retry: C.retry, dismiss: C.dismiss };

export function UnfinishedTransfersCard({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const backend = useBackend();
  const model = recoveryFor(backend, walletId);
  useModel(model);
  const query = useUnfinishedTransfers(walletId);
  const fmt = useFormats(network);
  const [details, setDetails] = useState(false);
  const errorText = (e: unknown) => shieldedErrorText(e, fmt.credits, fmt.duffs);
  const status = useShieldedStatus(walletId);
  const data = query.isSuccess ? withoutPendingShields(query.data, status.data?.pendingShields) : null;
  useEffect(() => model.settle(data), [model, data]);
  const view = cardView(data, model.state, fmt.duffs, errorText);
  const checking = data?.locks.filter((l) => l.origin === "restored" && l.state === "checking") ?? [];
  const run: Record<CardAction, () => void> = {
    finish: () => view?.lock && void model.finish(view.lock),
    finishAll: () => void model.finishAll(),
    checkAgain: () => void model.checkAgain(checking),
    details: () => setDetails(true),
    retry: () => void model.retry(),
    dismiss: () => model.dismiss(),
  };
  return (
    <>
      <GrantSheet grants={model.grants} />
      {view && (
        <section className="platform-card card-l1" data-testid="unfinished-card" data-kind={view.kind} aria-label={C.title}>
          <div className="platform-card-body">
            <h2 className="platform-card-title">{C.title}</h2>
            {view.note && (
              <p className="platform-card-text" role="status" data-testid="unfinished-note">
                {view.note}
              </p>
            )}
            <p className={view.kind === "failed" ? "form-problem" : "platform-card-text"} role={view.kind === "failed" ? "alert" : "status"} data-testid="unfinished-text">
              {view.text}
            </p>
          </div>
          {view.actions.length > 0 && (
            <div className="shielded-card-actions">
              {view.actions.map((a) => (
                <DashButton key={a} variant="tintedBlue" size="small" data-testid={`unfinished-${a}`} onClick={run[a]}>
                  {ACTION_LABEL[a]}
                </DashButton>
              ))}
            </div>
          )}
        </section>
      )}
      {details && <UnfinishedTransfersSheet model={model} walletId={walletId} network={network} onClose={() => setDetails(false)} />}
    </>
  );
}

export function UnfinishedTransfersSheet({ model, walletId, network, onClose }: { model: RecoveryModel; walletId: string; network: NetworkInfo; onClose: () => void }) {
  useModel(model);
  const query = useUnfinishedTransfers(walletId);
  const fmt = useFormats(network);
  const errorText = (e: unknown) => shieldedErrorText(e, fmt.credits, fmt.duffs);
  const status = useShieldedStatus(walletId);
  const rows = sheetRows(query.isSuccess ? withoutPendingShields(query.data, status.data?.pendingShields) : null, model.results);
  const failed = model.state.kind === "failed" ? model.state.error : null;
  return (
    <Sheet title={C.sheet.title} width="medium" testId="unfinished-sheet" onClose={onClose}>
      <p className="sheet-text">{C.sheet.intro}</p>
      <dl className="detail-list card-l1" data-testid="unfinished-rows">
        {rows.map(({ lock, result }) => {
          const key = lockKey(lock);
          const action = result?.kind === "applied" ? null : lock.state === "ready" ? "finish" : lock.state === "checking" ? "checkAgain" : null;
          return (
            <div className="detail-row" key={key} data-testid="unfinished-row" data-state={result?.kind === "applied" ? "applied" : lock.state}>
              <dt className="detail-label">{C.sheet.kind[lock.kind]}</dt>
              <dd className="detail-value">
                <span data-testid="unfinished-row-amount">{fmt.duffs(lock.duffs)}</span>{" "}
                <span className="chip" data-testid="unfinished-row-state">
                  {C.sheet.state[lock.state]}
                </span>
                <br />
                <CopyValue value={lock.txid} display={shortId(lock.txid)} label="transaction id" mono />
                {result && (
                  <p className={result.kind === "notApplied" ? "form-problem" : "field-hint"} role="status" data-testid="unfinished-row-result">
                    {resultText(result, errorText)}
                  </p>
                )}
                {action && (
                  <div>
                    <DashButton
                      variant="plainBlue"
                      size="small"
                      data-testid={`unfinished-row-${action}`}
                      disabled={model.running}
                      onClick={() => void (action === "finish" ? model.finish(lock) : model.checkAgain([lock]))}
                    >
                      {action === "finish" ? C.finish : C.checkAgain}
                    </DashButton>
                  </div>
                )}
              </dd>
            </div>
          );
        })}
      </dl>
      {failed !== null && rows.every((r) => r.result?.kind !== "notApplied") && (
        <p className="form-problem" role="alert" data-testid="unfinished-sheet-error">
          {errorText(failed)}
        </p>
      )}
      <p className="field-hint" data-testid="unfinished-footer">
        {C.sheet.footer}
      </p>
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" data-testid="unfinished-close" onClick={onClose}>
          {C.sheet.close}
        </DashButton>
      </div>
    </Sheet>
  );
}
