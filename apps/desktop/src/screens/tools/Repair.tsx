// Tools ▸ Repair (QT-117, QT-148, IOS-034, IOS-113, E0-04 C9): rescans with progress and Cancel
// (D10), the wallet's birth height, removing unconfirmed transactions and resetting chain data
// (each asks first, D11), and resolving a registration's stuck funding (D12). Finishing stranded
// asset locks waits on DP1-09 and DP1-02's facade call.

import { useEffect, useId, useState } from "react";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { useQueryClient } from "@tanstack/react-query";
import { keys, useBackend, useSyncStatus } from "../../api/queries";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { useModel } from "../../onboarding/model";
import type { GrantModel } from "../../tools/grant";
import { RepairModel, RepairText, type Confirmation } from "../../tools/repair";
import { NotYet, Outcome, ToolCard } from "./common";

/** The rescan's progress is read this often while one runs (D10). */
const POLL_MS = 1000;

const QUESTION: Record<Confirmation, { title: string; text: string; action: string }> = {
  resetChainData: { title: RepairText.resetChainData, text: RepairText.resetChainDataQuestion, action: "Reset and resync" },
  dropUnconfirmed: { title: RepairText.dropUnconfirmed, text: RepairText.dropUnconfirmedQuestion, action: "Remove" },
};

export function RepairTab({ grants, wallet }: { grants: GrantModel; wallet: WalletSummary }) {
  const b = useBackend();
  const client = useQueryClient();
  const [model] = useState(() => new RepairModel(b, grants, wallet.walletId));
  useModel(model);
  const [from, setFrom] = useState("");
  const [birth, setBirth] = useState("");
  const id = useId();
  const synced = useSyncStatus().dataUpdatedAt;
  useEffect(() => void model.refresh(), [model, synced]);
  useEffect(() => {
    if (!model.rescanning) return;
    const timer = setInterval(() => void model.refresh(), POLL_MS);
    return () => clearInterval(timer);
  }, [model, model.rescanning]);

  const { state } = model;
  const outcome = state.kind === "done" ? { ok: true, text: state.text } : state.kind === "failed" ? { ok: false, text: state.text } : null;
  const question = state.kind === "confirming" ? QUESTION[state.what] : null;
  const fraction = model.progressFraction;
  return (
    <div className="tool-stack" data-testid="tools-repair">
      {outcome && <Outcome ok={outcome.ok} text={outcome.text} testId="repair-outcome" />}
      <ToolCard title="Rescan" testId="repair-rescan">
        <p className="placeholder-note">Reads the block filters again to find this wallet's transactions.</p>
        {model.rescanning ? (
          <div className="tool-progress" data-testid="repair-progress">
            <progress max={1} value={fraction ?? undefined} aria-label="Rescan progress" />
            <span role="status">{model.progressText ?? "Rescanning…"}</span>
            <DashButton variant="tintedGray" size="small" data-testid="repair-cancel-rescan" onClick={() => void model.cancelRescan()}>
              Cancel
            </DashButton>
          </div>
        ) : (
          <div className="tool-actions">
            <DashButton variant="tintedBlue" size="small" data-testid="repair-rescan-birth" disabled={model.busy} onClick={() => void model.rescan({ kind: "walletBirth" })}>
              From the wallet's birth
            </DashButton>
            <DashButton variant="tintedBlue" size="small" data-testid="repair-rescan-genesis" disabled={model.busy} onClick={() => void model.rescan({ kind: "genesis" })}>
              From the first block
            </DashButton>
            <form
              className="tool-inline-form"
              onSubmit={(e) => {
                e.preventDefault();
                void model.rescanFrom(from);
              }}
            >
              <label htmlFor={`${id}-from`}>From height</label>
              <input id={`${id}-from`} data-testid="repair-rescan-height" inputMode="numeric" value={from} onChange={(e) => setFrom(e.target.value)} />
              <DashButton type="submit" variant="tintedBlue" size="small" data-testid="repair-rescan-from" disabled={model.busy || from.trim() === ""}>
                Rescan
              </DashButton>
            </form>
          </div>
        )}
      </ToolCard>
      <ToolCard title="Wallet birth height" testId="repair-birth">
        <p className="placeholder-note">
          The first block this wallet can have transactions in{wallet.birthHeight !== null ? <>: now {wallet.birthHeight}</> : null}. A lower height rescans
          from it.
        </p>
        <form
          className="tool-inline-form"
          onSubmit={(e) => {
            e.preventDefault();
            // The summary's birth height shows the saved one.
            void model.setBirthHeight(birth).then(() => client.invalidateQueries({ queryKey: keys.wallets }));
          }}
        >
          <label htmlFor={`${id}-birth`}>Birth height</label>
          <input id={`${id}-birth`} data-testid="repair-birth-height" inputMode="numeric" value={birth} onChange={(e) => setBirth(e.target.value)} />
          <DashButton type="submit" variant="tintedBlue" size="small" data-testid="repair-birth-save" disabled={model.busy || birth.trim() === ""}>
            Save
          </DashButton>
        </form>
      </ToolCard>
      <ToolCard title="Repair the wallet" testId="repair-actions">
        <div className="tool-actions">
          <DashButton variant="tintedBlue" size="small" data-testid="repair-drop-unconfirmed" disabled={model.busy} onClick={() => model.ask("dropUnconfirmed")}>
            {RepairText.dropUnconfirmed}…
          </DashButton>
          <DashButton variant="plainRed" size="small" data-testid="repair-reset-chain" disabled={model.busy} onClick={() => model.ask("resetChainData")}>
            {RepairText.resetChainData}…
          </DashButton>
        </div>
      </ToolCard>
      {model.stuck.length > 0 && (
        <ToolCard title="Stuck username funding" testId="repair-stuck">
          <p className="placeholder-note">
            Whether these registrations' funding reached the network is unknown. Sending every coin of the wallet to itself settles it either way; the
            registration goes on once that is confirmed.
          </p>
          <ul className="tool-list">
            {model.stuck.map((r) => (
              <li key={r.draft} data-testid={`repair-stuck-${r.draft}`}>
                <span>{r.label}</span>
                <DashButton variant="tintedBlue" size="small" data-testid={`repair-resolve-${r.draft}`} disabled={model.busy} onClick={() => void model.resolveStuck(r)}>
                  Resolve stuck funding: send all coins to yourself
                </DashButton>
              </li>
            ))}
          </ul>
        </ToolCard>
      )}
      <NotYet title="Asset locks" owner="DP1-09, DP1-02" testId="repair-asset-locks-not-yet">
        Finishing stranded transfers and sending or cancelling unrecorded asset locks.
      </NotYet>
      {question && (
        <Sheet title={question.title} width="small" testId="repair-confirm" onClose={() => model.dismiss()}>
          <p className="sheet-text">{question.text}</p>
          <div className="sheet-actions">
            <DashButton variant="tintedGray" size="large" data-testid="repair-confirm-cancel" onClick={() => model.dismiss()}>
              Cancel
            </DashButton>
            <DashButton variant="filledBlue" size="large" data-testid="repair-confirm-ok" onClick={() => void model.confirm()}>
              {question.action}
            </DashButton>
          </div>
        </Sheet>
      )}
    </div>
  );
}
