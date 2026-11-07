// Tools ▸ Repair (QT-117, QT-148, IOS-034, IOS-113, E0-04 C9): rescans with progress and Cancel
// (D10), the wallet's birth height, removing unconfirmed transactions and resetting chain data
// (each asks first, D11), resolving a registration's, a top-up's or a move into Shielded's stuck
// funding (D12, DP1-10 D16), and finishing stranded asset locks (DP1-09 D10). Unrecorded asset
// locks wait on DP1-02's facade call.

import { useEffect, useId, useState } from "react";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { useQueryClient } from "@tanstack/react-query";
import { keys, useBackend, useUnfinishedTransfers } from "../../api/queries";
import { unreadText } from "../../dashpay/registration";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { useDisplay } from "../../format/display";
import { useModel } from "../../onboarding/model";
import type { GrantModel } from "../../tools/grant";
import { isShield, isTopUp, repairErrorText, RepairModel, RepairText, type Confirmation } from "../../tools/repair";
import { NotYet, Outcome, ToolCard, useReloadOnSync } from "./common";

/** The rescan's progress is read this often while one runs (D10). */
const POLL_MS = 1000;

const QUESTION: Record<Confirmation, { title: string; text: string; action: string }> = {
  resetChainData: { title: RepairText.resetChainData, text: RepairText.resetChainDataQuestion, action: "Reset and resync" },
  dropUnconfirmed: { title: RepairText.dropUnconfirmed, text: RepairText.dropUnconfirmedQuestion, action: "Remove" },
  resolveStuck: { title: RepairText.resolveStuck, text: RepairText.resolveStuckQuestion, action: "Send to yourself" },
};

export function RepairTab({ grants, wallet, network }: { grants: GrantModel; wallet: WalletSummary; network: NetworkInfo }) {
  const b = useBackend();
  const shown = useDisplay(network);
  const amount = (duffs: number) => `${shown.compact(duffs)} ${shown.name}`;
  const client = useQueryClient();
  const [model] = useState(() => new RepairModel(b, grants, wallet.walletId));
  const unfinished = useUnfinishedTransfers(wallet.walletId);
  useModel(model);
  const [from, setFrom] = useState("");
  const [birth, setBirth] = useState("");
  const id = useId();
  useReloadOnSync(() => void model.refresh());
  useEffect(() => {
    if (!model.rescanning) return;
    const timer = setInterval(() => void model.refresh(), POLL_MS);
    return () => clearInterval(timer);
  }, [model, model.rescanning]);

  const { state } = model;
  const outcome = state.kind === "done" ? { ok: true, text: state.text } : state.kind === "failed" ? { ok: false, text: state.text } : null;
  const target = state.kind === "confirming" ? state.target : undefined;
  const question =
    state.kind !== "confirming"
      ? null
      : target && isShield(target) && target.repair
        ? {
            title: RepairText.resolveStuck,
            text: RepairText.resolveStuckShieldQuestion(target.repair.inputs, amount(target.repair.totalDuffs), amount(target.repair.feeDuffs)),
            action: `Send back and pay ${amount(target.repair.feeDuffs)}`,
          }
        : target && isTopUp(target)
          ? { ...QUESTION.resolveStuck, text: RepairText.resolveStuckTopUpQuestion }
          : QUESTION[state.what];
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
                <span>{r.label || RepairText.unnamedRegistration}</span>
                <DashButton variant="tintedBlue" size="small" data-testid={`repair-resolve-${r.draft}`} disabled={model.busy} onClick={() => model.ask("resolveStuck", r)}>
                  Resolve stuck funding: send all coins to yourself
                </DashButton>
              </li>
            ))}
          </ul>
        </ToolCard>
      )}
      {model.stuckTopUps.length > 0 && (
        <ToolCard title="Stuck credit top-ups" testId="repair-stuck-top-ups">
          <p className="placeholder-note">
            Whether these top-ups' payment reached the network is unknown. Sending every coin of the wallet to itself settles it either way; you can top up
            again once that is confirmed.
          </p>
          <ul className="tool-list">
            {model.stuckTopUps.map(({ funding, name }) => (
              <li key={funding.step} data-testid={`repair-stuck-${funding.step}`}>
                <span>{RepairText.topUp(name)}</span>
                <DashButton
                  variant="tintedBlue"
                  size="small"
                  data-testid={`repair-resolve-${funding.step}`}
                  disabled={model.busy}
                  onClick={() => model.ask("resolveStuck", funding)}
                >
                  Resolve stuck funding: send all coins to yourself
                </DashButton>
              </li>
            ))}
          </ul>
        </ToolCard>
      )}
      {model.stuckShields.length > 0 && (
        <ToolCard title="Stuck moves into Shielded" testId="repair-stuck-shields">
          <p className="placeholder-note">
            Whether these moves' payment reached the network is unknown, so no new move into Shielded can start. Repair settles each by sending its coins back
            to your own wallet; you confirm its fee first.
          </p>
          <ul className="tool-list">
            {model.stuckShields.map((f) => (
              <li key={f.step} data-testid={`repair-stuck-${f.step}`}>
                <span>
                  {RepairText.shield(f.repair?.mixed ?? false)}.{" "}
                  {f.repair
                    ? RepairText.shieldRepair(f.repair.inputs, amount(f.repair.totalDuffs), amount(f.repair.feeDuffs))
                    : RepairText.shieldBlocked(repairErrorText(f.blocked))}
                </span>
                <DashButton
                  variant="tintedBlue"
                  size="small"
                  data-testid={`repair-resolve-${f.step}`}
                  disabled={model.busy || !f.repair}
                  onClick={() => model.ask("resolveStuck", f)}
                >
                  Send the coins back…
                </DashButton>
              </li>
            ))}
          </ul>
        </ToolCard>
      )}
      {model.stuckUnreadable && (
        <p className="placeholder-note" data-testid="repair-stuck-unreadable">
          {RepairText.stuckUnreadable}
        </p>
      )}
      {model.stuckShieldsUnreadable && (
        <p className="placeholder-note" data-testid="repair-stuck-shields-unreadable">
          {RepairText.stuckShieldsUnreadable}
        </p>
      )}
      {model.stuckTopUpsUnreadable && (
        <p className="placeholder-note" data-testid="repair-stuck-top-ups-unreadable">
          {RepairText.stuckTopUpsUnreadable}
        </p>
      )}
      <ToolCard title="Finish transfers" testId="repair-finish">
        <p className="placeholder-note">
          Finishes username registrations, credit top-ups, and transfers to Shielded or to your Platform addresses whose payment was sent but never completed. Nothing more is paid.
        </p>
        {unfinished.data && unfinished.data.unread.length > 0 && (
          <p className="placeholder-note" role="status" data-testid="repair-unfinished-unread">
            {unreadText(unfinished.data.unread)}
          </p>
        )}
        <div className="tool-actions">
          <DashButton variant="tintedBlue" size="small" data-testid="repair-finish-transfers" disabled={model.busy} onClick={() => void model.finishTransfers()}>
            Finish transfers
          </DashButton>
        </div>
      </ToolCard>
      <NotYet title="Asset locks" owner="DP1-02" testId="repair-asset-locks-not-yet">
        Sending or cancelling unrecorded asset locks.
      </NotYet>
      {question && (
        <Sheet title={question.title} width="small" testId="repair-confirm" onClose={() => model.dismiss()}>
          <p className="sheet-text">{question.text}</p>
          <div className="sheet-actions">
            <DashButton variant="tintedGray" size="large" data-testid="repair-confirm-cancel" onClick={() => model.dismiss()}>
              Cancel
            </DashButton>
            {/* A resolved funding leaves the stuck lists at once (DP1-10 D3): My Profile reads them again. */}
            <DashButton
              variant="filledBlue"
              size="large"
              data-testid="repair-confirm-ok"
              onClick={() => void model.confirm().then(() => client.invalidateQueries({ queryKey: keys.dashpayAll }))}
            >
              {question.action}
            </DashButton>
          </div>
        </Sheet>
      )}
    </div>
  );
}
