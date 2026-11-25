// Tools ▸ Coin Selection (QT-068…075): Pay's coin table (`components/coins.tsx`, D19) with
// dash-qt's columns, list or tree, sorting, CoinJoin coins hidden by default, locks, copy and the
// selection's summary. "Pay with these coins" opens Pay with the selection (QT-068).

import { useEffect, useRef } from "react";
import type { QueryClient } from "@tanstack/react-query";
import { errorMessage, type Backend } from "../../api/backend";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { Utxo } from "../../api/generated/Utxo";
import { keys, useUtxos, useWalletCatchUp } from "../../api/queries";
import { useRestoredGate } from "../../api/restoreGate";
import { navigate, openPayForm } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { CoinSummary, CoinTable } from "../../components/coins";
import { coinsEmptyCopy, EMPTY, listDisplay } from "../../components/emptyStates";
import { EmptyState, focusWhenIdle, LoadingState } from "../../components/history";
import type { Unit } from "../../format/amount";
import { useDisplay } from "../../format/display";
import { useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import { CoinSelectionModel, CoinText, copyText, type CopyField } from "../../tools/coins";
import { outpointKey } from "../pay/model";
import { Outcome, RowMenu, type MenuItem } from "./common";

const COPY: [CopyField, string, "copyAddress" | "copyLabel" | "copyAmount" | "copyOutpoint"][] = [
  ["address", "copy-address", "copyAddress"],
  ["label", "copy-label", "copyLabel"],
  ["amount", "copy-amount", "copyAmount"],
  ["outpoint", "copy-transaction-id-and-output-index", "copyOutpoint"],
];

/** dash-qt's coin context menu: the copies, then Lock or Unlock. */
function CoinMenu({ u, model, unit }: { u: Utxo; model: CoinSelectionModel; unit: Unit }) {
  const t = useT();
  const key = outpointKey(u.outpoint);
  const lock: MenuItem = u.userLocked
    ? { id: "unlock-unspent", title: t("tools.coins.unlockUnspent"), run: () => void model.unlock([key]) }
    : { id: "lock-unspent", title: t("tools.coins.lockUnspent"), run: () => void model.lock([key]), disabled: u.reserved };
  return (
    <RowMenu
      label={t("tools.coins.actions")}
      name={t("tools.coins.actionsFor", { amount: copyText(u, "amount", unit), address: u.address })}
      items={[...COPY.map(([field, id, message]) => ({ id, title: t(`tools.coins.${message}`), copy: copyText(u, field, unit) })), lock]}
      testId={`coin-menu-${key}`}
    />
  );
}

/** The tab's model; the page keeps one per window wallet, so the view (mode, sort, CoinJoin
 * coins) and the selection outlive a tab change (D8). */
export function newCoinSelection(b: Backend, client: QueryClient, walletId: string): CoinSelectionModel {
  return new CoinSelectionModel(b, walletId, async () => {
    // A lock changes the coins and the balance: every coin read of the wallet goes stale.
    await client.invalidateQueries({ queryKey: keys.coinsAll(walletId) });
    return client.getQueryData<Utxo[]>(keys.utxos(walletId)) ?? [];
  });
}

export function CoinSelectionTab({ model, walletId, network }: { model: CoinSelectionModel; walletId: string; network: NetworkInfo }) {
  const t = useT();
  const utxos = useUtxos(walletId, true);
  const walletRestored = useRestoredGate();
  const { arrival, recheck } = useWalletCatchUp(walletId);
  const coins = listDisplay({
    rowCount: utxos.data?.length ?? 0,
    pending: utxos.isPending,
    failed: utxos.isError,
    hasSnapshot: utxos.data !== undefined,
    refreshing: utxos.isFetching,
    arrival,
  });
  const shown = useDisplay(network);
  useModel(model);
  useEffect(() => {
    if (utxos.data) model.setCoins(utxos.data);
  }, [model, utxos.data]);

  const filterRef = useRef<HTMLButtonElement>(null);
  if (coins.kind === "arriving" && coins.why === "pending") return <LoadingState testId="coins-loading" text={t("tools.coins.loading")} />;
  if (coins.kind === "failed")
    return (
      <>
        <Outcome ok={false} text={t("tools.coins.loadFailed", { error: errorMessage(utxos.error) })} testId="coins-error" />
        <DashButton variant="plainBlue" data-testid="coins-retry" onClick={() => void utxos.refetch()}>
          {t("tools.common.tryAgain")}
        </DashButton>
      </>
    );
  const coinCopy = coinsEmptyCopy(coins.kind === "arriving" && coins.why !== "pending" ? coins.why : "settled");
  const selected = new Set(model.selected);
  return (
    <div className="tool-stack" data-testid="tools-coinSelection">
      <div className="tool-actions">
        <div className="segmented" role="group" aria-label={t("tools.coins.view")} data-testid="coins-mode">
          {(["list", "tree"] as const).map((m) => (
            <button key={m} ref={m === "list" ? filterRef : undefined} type="button" className="segment" aria-pressed={model.mode === m} data-testid={`coins-mode-${m}`} onClick={() => model.setMode(m)}>
              {m === "list" ? t("tools.coins.listMode") : t("tools.coins.treeMode")}
            </button>
          ))}
        </div>
        <DashButton variant="tintedGray" size="small" data-testid="coins-coinjoin" onClick={() => model.setShowCoinJoin(!model.showCoinJoin)}>
          {model.coinJoinToggleTitle}
        </DashButton>
        <DashButton variant="tintedGray" size="small" data-testid="coins-select-all" onClick={() => model.toggleAll()}>
          {t("tools.coins.selectAll")}
        </DashButton>
        <DashButton variant="tintedGray" size="small" data-testid="coins-toggle-locks" onClick={() => void model.toggleLocks()}>
          {t("tools.coins.toggleLocks")}
        </DashButton>
        {model.lockedText && (
          <span className="tool-note" data-testid="coins-locked">
            {model.lockedText}
          </span>
        )}
      </div>
      {model.error && <Outcome ok={false} text={model.error} testId="coins-problem" />}
      {model.unselectedNotice && (
        <p className="form-problem" role="status" data-testid="coins-unselected">
          {CoinText.unselected}
        </p>
      )}
      {model.visible.length === 0 ? (
        (utxos.data?.length ?? 0) === 0 ? (
          <EmptyState
            testId="coins-none"
            art={coinCopy.art}
            title={coinCopy.title}
            message={coinCopy.message}
            action={
              coinCopy.action && (
                <DashButton variant="filledBlue" data-testid={coinCopy.action.testId} onClick={() => (coins.kind === "empty" ? navigate("receive") : recheck())}>
                  {coinCopy.action.label}
                </DashButton>
              )
            }
          />
        ) : (
          <EmptyState
            testId="coins-none"
            art={EMPTY.coinsHidden.art}
            title={EMPTY.coinsHidden.title}
            message={EMPTY.coinsHidden.message}
            action={
              <DashButton
                variant="filledBlue"
                data-testid={EMPTY.coinsHidden.action.testId}
                onClick={() => {
                  model.setShowCoinJoin(true);
                  focusWhenIdle(filterRef);
                }}
              >
                {EMPTY.coinsHidden.action.label}
              </DashButton>
            }
          />
        )
      ) : (
        <div className="table-card">
          <CoinTable
            coins={model.visible}
            shown={shown}
            selected={selected}
            onToggle={(key) => model.toggle(key)}
            detail={{
              sort: model.sort,
              onSort: (c) => model.sortBy(c),
              groups: model.mode === "tree" ? model.groups : null,
              onToggleGroup: (g) => model.toggleCoins(g.coins),
              actions: (u) => <CoinMenu u={u} model={model} unit={shown.amountUnit} />,
            }}
          />
        </div>
      )}
      {model.summary && (
        <section className="tool-card card-l1" aria-label={t("tools.coins.selection")} data-testid="coins-selection">
          <CoinSummary summary={model.summary} shown={shown} />
          <p className="tool-note">{CoinText.tolerance(model.summary.feeTolerancePerInput)}</p>
        </section>
      )}
      {model.summaryUnavailable && (
        <p className="placeholder-note" data-testid="coins-summary-unavailable">
          {CoinText.unavailable}
        </p>
      )}
      <div className="tool-actions">
        <DashButton variant="filledBlue" size="small" data-testid="coins-pay" disabled={walletRestored !== false || model.selected.length === 0} onClick={() => openPayForm({ payCoins: model.selected })}>
          {t("tools.coins.payWithCoins")}
        </DashButton>
      </div>
      {walletRestored === true && (
        <p className="placeholder-note" role="status" data-testid="coins-restored-gate">
          {t("storeUpgrade.restore.banner")}
        </p>
      )}
    </div>
  );
}
