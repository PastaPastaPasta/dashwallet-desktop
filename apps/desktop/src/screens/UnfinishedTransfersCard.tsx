// Unfinished transfers on Home (recovery/model.ts): the card that offers to finish what a wallet
// restore found locked for Shielded or for "Add from Dash wallet", and the sheet with one row per lock.
// The card also carries the rows of this install's own transfers that need the user: a shield or an
// add to Platform to finish or check again, a result not seen yet, and Activity's check to retry. It
// hosts the Shielded send and passphrase sheets and settles a pending move, whatever it shows.

import { useEffect, useState } from "react";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { useBackend, usePendingPlatformFunds, usePoolBalances, useShieldedStatus, useUnfinishedTransfers } from "../api/queries";
import { DashButton } from "../components/basics";
import { CopyValue } from "../components/detail";
import { Sheet } from "../components/sheet";
import { shortId } from "../dashpay/identities";
import { unreadText } from "../dashpay/registration";
import { useModel } from "../onboarding/model";
import { fundFor, fundRowView, FUND_COPY as F } from "../platform/fund";
import { moveFor, MOVE_COPY as M } from "../platform/move";
import { cardView, lockKey, lockKinds, RECOVERY_COPY as C, recoveryFor, resultText, sheetRows, withoutPendingShields, type CardAction, type RecoveryModel } from "../recovery/model";
import { SHIELDED_COPY as S, shieldedErrorText, shieldedFor } from "../shielded/flow";
import { SHIELDED_SYNCING_TEXT, shieldedSyncing } from "../shielded/syncing";
import { shieldedRoute, transferFor } from "../transfer/model";
import { usePlatformHold } from "./PlatformMoveSheet";
import { ShieldedSheet, useFormats } from "./ShieldedSheet";
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

  const shielded = shieldedFor(backend, walletId);
  const fund = fundFor(backend, walletId);
  const move = moveFor(backend, walletId);
  const transfer = transferFor(backend, walletId);
  useModel(shielded);
  useModel(fund);
  const { activity, block } = usePlatformHold(walletId, move);
  // Activity settles a retained `pending` move: when it changes, when a move turns pending (the rows
  // read may already have shown it settled) and when the sheet closes.
  useEffect(() => move.settle(activity.data), [move, activity.data, move.pending, move.open]);
  const platformKnown = usePoolBalances(walletId).platform != null;
  const s = status.data ?? null;
  const shields = s?.pendingShields ?? [];
  const shieldSyncing = s !== null && shieldedSyncing(s);
  const shieldWhy = shieldSyncing ? SHIELDED_SYNCING_TEXT : shielded.running ? S.card.why.busy : null;
  // A lock this version can't finish has no action: Transfer's Add to Platform names it.
  const funds = (usePendingPlatformFunds(walletId).data ?? []).map((p) => ({ p, row: fundRowView(p, fmt.duffs) })).filter(({ row }) => row.action !== null);
  const checkFailed = platformKnown && block === "failed" && !move.running;
  const showRoute = shieldedRoute(shielded.kind);
  const own = shields.length > 0 || funds.length > 0 || shielded.unseen || fund.unseen || move.unseen || checkFailed;

  return (
    <>
      <GrantSheet grants={model.grants} />
      <ShieldedSheet flow={shielded} walletId={walletId} network={network} status={s} />
      {(view || own) && (
        <section className="platform-card card-l1" data-testid="unfinished-card" data-kind={view?.kind} aria-label={C.title}>
          <div className="platform-card-body">
            <h2 className="platform-card-title">{C.title}</h2>
            {view?.note && (
              <p className="platform-card-text" role="status" data-testid="unfinished-note">
                {view.note}
              </p>
            )}
            {view && (
              <p className={view.kind === "failed" ? "form-problem" : "platform-card-text"} role={view.kind === "failed" ? "alert" : "status"} data-testid="unfinished-text">
                {view.text}
              </p>
            )}
            {shields.map((p) => (
              <p key={p.txid} className="platform-card-text" data-testid={p.consumptionUnknown ? "shielded-checking" : "shielded-unfinished"}>
                {p.consumptionUnknown ? (
                  <>
                    <strong>{S.done.checking.title}.</strong> {S.done.checking.text}
                  </>
                ) : (
                  S.card.unfinished(fmt.duffs(p.duffs))
                )}{" "}
                <DashButton variant="plainBlue" size="small" data-testid="shielded-finish" disabled={shieldSyncing || shielded.running} why={shieldWhy} onClick={() => void shielded.resume(p.txid)}>
                  {p.consumptionUnknown ? S.card.checkAgain : S.card.finish}
                </DashButton>
              </p>
            ))}
            {funds.map(({ p, row }) => (
              <p key={p.txid} className="platform-card-text" data-testid={`platform-fund-${row.kind}`} data-state={p.state}>
                {row.kind === "checking" && (
                  <>
                    <strong>{F.done.checking.title}.</strong>{" "}
                  </>
                )}
                {row.text}{" "}
                <DashButton variant="plainBlue" size="small" data-testid="platform-fund-finish" disabled={fund.running} why={fund.running ? F.card.busy : null} onClick={() => void fund.resume(p.txid)}>
                  {row.action === "checkAgain" ? F.card.checkAgain : F.card.finish}
                </DashButton>
              </p>
            ))}
            {shielded.unseen && (
              <p className="platform-card-text" data-testid="shielded-unseen">
                {S.unseen}{" "}
                <DashButton variant="plainBlue" size="small" data-testid="shielded-show" onClick={() => (showRoute === "send" ? shielded.begin("send") : transfer.start(showRoute))}>
                  {S.show}
                </DashButton>
              </p>
            )}
            {fund.unseen && (
              <p className="platform-card-text" data-testid="platform-fund-unseen">
                {F.unseen}{" "}
                <DashButton variant="plainBlue" size="small" data-testid="platform-fund-show" onClick={() => transfer.start("coreToPlatform")}>
                  {F.show}
                </DashButton>
              </p>
            )}
            {move.unseen && (
              <p className="platform-card-text" data-testid="platform-move-unseen">
                {M.unseen}{" "}
                <DashButton variant="plainBlue" size="small" data-testid="platform-move-show" onClick={() => transfer.start("platformToCore")}>
                  {M.show}
                </DashButton>
              </p>
            )}
            {checkFailed && (
              <p className="platform-card-text" data-testid="platform-move-why">
                {M.check.failed}{" "}
                <DashButton variant="plainBlue" size="small" data-testid="platform-move-check-again" onClick={() => void activity.refetch()}>
                  {M.check.again}
                </DashButton>
              </p>
            )}
          </div>
          {view && view.actions.length > 0 && (
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
      <p className="sheet-text">{C.sheet.intro(lockKinds(rows.map((r) => r.lock)))}</p>
      {query.data && query.data.unread.length > 0 && (
        <p className="field-hint" role="status" data-testid="unfinished-unread">
          {unreadText(query.data.unread)}
        </p>
      )}
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
                    {resultText(result, errorText, lock.kind)}
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
