// Tools ▸ Repair (QT-117, QT-148, IOS-034, IOS-113, E0-04 C9): rescans with progress and Cancel
// (D10), the wallet's birth height, removing unconfirmed transactions and resetting chain data
// (each asks first, D11), resolving a registration's, a top-up's or a move into Shielded's stuck
// funding (D12, DP1-10 D16), and finishing stranded asset locks (DP1-09 D10). Unrecorded asset
// locks wait on DP1-02's facade call.

import { useEffect, useId, useRef, useState } from "react";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { useQueryClient } from "@tanstack/react-query";
import { keys, useBackend, useShieldedSyncInfo, useUnfinishedTransfers } from "../../api/queries";
import { shieldedSpendsOff } from "../../shielded/spends";
import { unreadText } from "../../dashpay/registration";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { useDisplay } from "../../format/display";
import { useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import type { GrantModel } from "../../tools/grant";
import { isShield, isTopUp, RepairModel, RepairText, shieldBlockedText, type Confirmation } from "../../tools/repair";
import { NotYet, Outcome, ToolCard, useReloadOnSync } from "./common";

/** The rescan's progress is read this often while one runs (D10). */
const POLL_MS = 1000;

export function RepairTab({ grants, wallet, network }: { grants: GrantModel; wallet: WalletSummary; network: NetworkInfo }) {
  const t = useT();
  const b = useBackend();
  const shown = useDisplay(network);
  const amount = (duffs: number) => `${shown.compact(duffs)} ${shown.name}`;
  const client = useQueryClient();
  const spendsOff = shieldedSpendsOff(useShieldedSyncInfo(wallet.walletId).data);
  const spendsOffNow = useRef(spendsOff);
  useEffect(() => {
    spendsOffNow.current = spendsOff;
  }, [spendsOff]);
  const [model] = useState(() => new RepairModel(b, grants, wallet.walletId, () => spendsOffNow.current));
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

  const questions: Record<Confirmation, { title: string; text: string; action: string }> = {
    resetChainData: { title: RepairText.resetChainData, text: RepairText.resetChainDataQuestion, action: RepairText.resetAction },
    dropUnconfirmed: { title: RepairText.dropUnconfirmed, text: RepairText.dropUnconfirmedQuestion, action: t("tools.repair.dropAction") },
    resolveStuck: { title: RepairText.resolveStuck, text: RepairText.resolveStuckQuestion, action: t("tools.repair.resolveAction") },
  };
  const { state } = model;
  const outcome = state.kind === "done" ? { ok: true, text: state.text } : state.kind === "failed" ? { ok: false, text: state.text } : null;
  const target = state.kind === "confirming" ? state.target : undefined;
  const question =
    state.kind !== "confirming"
      ? null
      : target && isShield(target) && target.repair
        ? {
            title: RepairText.resolveStuck,
            text: RepairText.resolveStuckShieldQuestion(target.repair.mixed, target.repair.inputs, amount(target.repair.totalDuffs), amount(target.repair.feeDuffs)),
            action: t("tools.repair.sendBackAndPay", { fee: amount(target.repair.feeDuffs) }),
          }
        : target && isTopUp(target)
          ? { ...questions.resolveStuck, text: RepairText.resolveStuckTopUpQuestion }
          : questions[state.what];
  const fraction = model.progressFraction;
  return (
    <div className="tool-stack" data-testid="tools-repair">
      <Outcome ok={outcome?.ok ?? true} text={outcome?.text ?? null} testId="repair-outcome" polite />
      <ToolCard title={t("tools.repair.rescanTitle")} testId="repair-rescan">
        <p className="placeholder-note">{t("tools.repair.rescanHint")}</p>
        {model.rescanning ? (
          <div className="tool-progress" data-testid="repair-progress">
            <progress max={1} value={fraction ?? undefined} aria-label={t("tools.repair.rescanProgress")} />
            <span role="status">{model.progressText ?? t("tools.repair.rescanning")}</span>
            <DashButton variant="tintedGray" size="small" data-testid="repair-cancel-rescan" onClick={() => void model.cancelRescan()}>
              {t("common.cancel")}
            </DashButton>
          </div>
        ) : (
          <div className="tool-actions">
            <DashButton variant="tintedBlue" size="small" data-testid="repair-rescan-birth" disabled={model.busy} onClick={() => void model.rescan({ kind: "walletBirth" })}>
              {t("tools.repair.fromBirth")}
            </DashButton>
            <DashButton variant="tintedBlue" size="small" data-testid="repair-rescan-genesis" disabled={model.busy} onClick={() => void model.rescan({ kind: "genesis" })}>
              {t("tools.repair.fromGenesis")}
            </DashButton>
            <form
              className="tool-inline-form"
              onSubmit={(e) => {
                e.preventDefault();
                void model.rescanFrom(from);
              }}
            >
              <label htmlFor={`${id}-from`}>{t("tools.repair.fromHeight")}</label>
              <input id={`${id}-from`} data-testid="repair-rescan-height" inputMode="numeric" value={from} onChange={(e) => setFrom(e.target.value)} />
              <DashButton type="submit" variant="tintedBlue" size="small" data-testid="repair-rescan-from" disabled={model.busy || from.trim() === ""}>
                {t("tools.repair.rescanButton")}
              </DashButton>
            </form>
          </div>
        )}
      </ToolCard>
      <ToolCard title={t("tools.repair.birthTitle")} testId="repair-birth">
        <p className="placeholder-note">
          {wallet.birthHeight !== null ? t("tools.repair.birthHintCurrent", { height: String(wallet.birthHeight) }) : t("tools.repair.birthHint")}
        </p>
        <form
          className="tool-inline-form"
          onSubmit={(e) => {
            e.preventDefault();
            // The summary's birth height shows the saved one.
            void model.setBirthHeight(birth).then(() => client.invalidateQueries({ queryKey: keys.wallets }));
          }}
        >
          <label htmlFor={`${id}-birth`}>{t("tools.repair.birthHeight")}</label>
          <input id={`${id}-birth`} data-testid="repair-birth-height" inputMode="numeric" value={birth} onChange={(e) => setBirth(e.target.value)} />
          <DashButton type="submit" variant="tintedBlue" size="small" data-testid="repair-birth-save" disabled={model.busy || birth.trim() === ""}>
            {t("common.save")}
          </DashButton>
        </form>
      </ToolCard>
      <ToolCard title={t("tools.repair.walletTitle")} testId="repair-actions">
        <div className="tool-actions">
          <DashButton variant="tintedBlue" size="small" data-testid="repair-drop-unconfirmed" disabled={model.busy} onClick={() => model.ask("dropUnconfirmed")}>
            {t("tools.repair.dropUnconfirmedButton")}
          </DashButton>
          <DashButton variant="plainRed" size="small" data-testid="repair-reset-chain" disabled={model.busy} onClick={() => model.ask("resetChainData")}>
            {RepairText.resetChainDataButton}
          </DashButton>
        </div>
      </ToolCard>
      {model.stuck.length > 0 && (
        <ToolCard title={t("tools.repair.stuckTitle")} testId="repair-stuck">
          <p className="placeholder-note">{t("tools.repair.stuckHint")}</p>
          <ul className="tool-list">
            {model.stuck.map((r) => (
              <li key={r.draft} data-testid={`repair-stuck-${r.draft}`}>
                <span>{r.label ? <bdi>{r.label}</bdi> : RepairText.unnamedRegistration}</span>
                <DashButton variant="tintedBlue" size="small" data-testid={`repair-resolve-${r.draft}`} disabled={model.busy} onClick={() => model.ask("resolveStuck", r)}>
                  {t("tools.repair.resolveButton")}
                </DashButton>
              </li>
            ))}
          </ul>
        </ToolCard>
      )}
      {model.stuckTopUps.length > 0 && (
        <ToolCard title={t("tools.repair.stuckTopUpsTitle")} testId="repair-stuck-top-ups">
          <p className="placeholder-note">{t("tools.repair.stuckTopUpsHint")}</p>
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
                  {t("tools.repair.resolveButton")}
                </DashButton>
              </li>
            ))}
          </ul>
        </ToolCard>
      )}
      {model.stuckShields.length > 0 && (
        <ToolCard title={t("tools.repair.stuckShieldsTitle")} testId="repair-stuck-shields">
          <p className="placeholder-note">{RepairText.stuckShieldsHint(spendsOff)}</p>
          <ul className="tool-list">
            {model.stuckShields.map((f) => (
              <li key={f.step} data-testid={`repair-stuck-${f.step}`}>
                <span>
                  {t("tools.repair.shieldLine", {
                    what: RepairText.shield(f.repair?.mixed ?? false),
                    detail: f.repair
                      ? RepairText.shieldRepair(f.repair.mixed, f.repair.inputs, amount(f.repair.totalDuffs), amount(f.repair.feeDuffs), spendsOff)
                      : f.blocked
                        ? shieldBlockedText(f.blocked, spendsOff)
                        : "",
                  })}
                </span>
                <DashButton
                  variant="tintedBlue"
                  size="small"
                  data-testid={`repair-resolve-${f.step}`}
                  disabled={model.busy || !f.repair}
                  onClick={() => model.ask("resolveStuck", f)}
                >
                  {t("tools.repair.sendCoinsBack")}
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
      <ToolCard title={t("tools.repair.finishTitle")} testId="repair-finish">
        <p className="placeholder-note">{t("tools.repair.finishHint")}</p>
        {unfinished.data && unfinished.data.unread.length > 0 && (
          <p className="placeholder-note" role="status" data-testid="repair-unfinished-unread">
            {unreadText(unfinished.data.unread)}
          </p>
        )}
        <div className="tool-actions">
          <DashButton variant="tintedBlue" size="small" data-testid="repair-finish-transfers" disabled={model.busy} onClick={() => void model.finishTransfers()}>
            {t("tools.repair.finishButton")}
          </DashButton>
        </div>
      </ToolCard>
      <NotYet title={t("tools.repair.assetLocksTitle")} testId="repair-asset-locks-not-yet">
        {t("tools.repair.assetLocksSoon")}
      </NotYet>
      {question && (
        <Sheet title={question.title} width="small" testId="repair-confirm" onClose={() => model.dismiss()}>
          <p className="sheet-text">{question.text}</p>
          <div className="sheet-actions">
            <DashButton variant="tintedGray" size="large" data-testid="repair-confirm-cancel" onClick={() => model.dismiss()}>
              {t("common.cancel")}
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
