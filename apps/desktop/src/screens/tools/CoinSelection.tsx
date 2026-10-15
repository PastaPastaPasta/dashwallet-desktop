// Tools ▸ Coin Selection (QT-068…075): Pay's coin table (`components/coins.tsx`, D19) with
// dash-qt's columns, list or tree, sorting, CoinJoin coins hidden by default, locks, copy and the
// selection's summary. "Pay with these coins" opens Pay with the selection (QT-068).

import { useEffect } from "react";
import type { QueryClient } from "@tanstack/react-query";
import { errorMessage, type Backend } from "../../api/backend";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { Utxo } from "../../api/generated/Utxo";
import { keys, useUtxos } from "../../api/queries";
import { openPayForm } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { CoinSummary, CoinTable } from "../../components/coins";
import { LoadingState } from "../../components/history";
import type { Unit } from "../../format/amount";
import { useDisplay } from "../../format/display";
import { useModel } from "../../onboarding/model";
import { CoinSelectionModel, CoinText, copyText, type CopyField } from "../../tools/coins";
import { outpointKey } from "../pay/model";
import { Outcome, RowMenu, type MenuItem } from "./common";

const COPY: [CopyField, string][] = [
  ["address", "Copy address"],
  ["label", "Copy label"],
  ["amount", "Copy amount"],
  ["outpoint", "Copy transaction ID and output index"],
];

/** dash-qt's coin context menu: the copies, then Lock or Unlock. */
function CoinMenu({ u, model, unit }: { u: Utxo; model: CoinSelectionModel; unit: Unit }) {
  const key = outpointKey(u.outpoint);
  const lock: MenuItem = u.userLocked
    ? { title: "Unlock unspent", run: () => void model.unlock([key]) }
    : { title: "Lock unspent", run: () => void model.lock([key]), disabled: u.reserved };
  return <RowMenu label="Actions" name={`Actions for ${copyText(u, "amount", unit)} at ${u.address}`} items={[...COPY.map(([field, title]) => ({ title, copy: copyText(u, field, unit) })), lock]} testId={`coin-menu-${key}`} />;
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
  const utxos = useUtxos(walletId, true);
  const shown = useDisplay(network);
  useModel(model);
  useEffect(() => {
    if (utxos.data) model.setCoins(utxos.data);
  }, [model, utxos.data]);

  if (utxos.isPending) return <LoadingState testId="coins-loading" text="Loading coins" />;
  if (utxos.isError) return <Outcome ok={false} text={`Could not load the coins: ${errorMessage(utxos.error)}`} testId="coins-error" />;
  const selected = new Set(model.selected);
  return (
    <div className="tool-stack" data-testid="tools-coinSelection">
      <div className="tool-actions">
        <div className="segmented" role="group" aria-label="View" data-testid="coins-mode">
          {(["list", "tree"] as const).map((m) => (
            <button key={m} type="button" className="segment" aria-pressed={model.mode === m} data-testid={`coins-mode-${m}`} onClick={() => model.setMode(m)}>
              {m === "list" ? "List mode" : "Tree mode"}
            </button>
          ))}
        </div>
        <DashButton variant="tintedGray" size="small" data-testid="coins-coinjoin" onClick={() => model.setShowCoinJoin(!model.showCoinJoin)}>
          {model.coinJoinToggleTitle}
        </DashButton>
        <DashButton variant="tintedGray" size="small" data-testid="coins-select-all" onClick={() => model.toggleAll()}>
          (un)select all
        </DashButton>
        <DashButton variant="tintedGray" size="small" data-testid="coins-toggle-locks" onClick={() => void model.toggleLocks()}>
          Toggle lock state
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
        <p className="placeholder-note" data-testid="coins-none">
          {utxos.data.length === 0 ? "This wallet has no coins yet." : "Every coin of this wallet is a CoinJoin coin; Show all coins lists them."}
        </p>
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
        <section className="tool-card card-l1" aria-label="Selection" data-testid="coins-selection">
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
        <DashButton variant="filledBlue" size="small" data-testid="coins-pay" disabled={model.selected.length === 0} onClick={() => openPayForm({ payCoins: model.selected })}>
          Pay with these coins
        </DashButton>
      </div>
    </div>
  );
}
