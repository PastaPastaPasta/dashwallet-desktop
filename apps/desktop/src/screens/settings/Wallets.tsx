// Settings ▸ Wallets (UX-SPEC §4.17, QT-101, QT-102, QT-114, IOS-110, IOS-111): U-02 D23's
// Wallet card grown into the list of every registered wallet (D17), with Open/Close, the startup
// list, Rename, Remove (D18), the extended public key and a watch-only wallet.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useState } from "react";
import { keys, useBackend, useQrMatrix, useRestored, useWallets } from "../../api/queries";
import { ui } from "../../app/ui";
import { Chip, DashButton } from "../../components/basics";
import { CopyValue, DetailRow } from "../../components/detail";
import { PassphrasePrompt } from "../../components/passphrase";
import { QrCode } from "../../components/qr";
import { Sheet } from "../../components/sheet";
import { useT } from "../../i18n";
import { failureText, useModel } from "../../onboarding/model";
import { RemoveWalletModel, WalletsModel, WalletsText, WatchOnlyModel, type WalletRow } from "../../settings/wallets";
import { useLockTimer } from "../LockScreen";
import { Done, Hint, Problem, SettingsCard } from "./parts";

type WalletSheet = { kind: "rename" | "remove" | "xpub"; row: WalletRow } | { kind: "watchOnly" } | null;

export function WalletsTab() {
  const t = useT();
  const backend = useBackend();
  const client = useQueryClient();
  const [model] = useState(() => new WalletsModel(backend));
  useModel(model);
  // Every wallet event refreshes the list too: the shell's list and this one move together.
  const wallets = useWallets();
  useEffect(() => void model.load(), [model, wallets.dataUpdatedAt]);
  const [sheet, setSheet] = useState<WalletSheet>(null);
  const restoreOnly = useRestored().data === true;
  const [notice, setNotice] = useState<string | null>(null);
  const refresh = () => void client.invalidateQueries({ queryKey: keys.wallets });
  const closeSheet = (message?: string) => {
    setSheet(null);
    if (message) setNotice(message);
    refresh();
  };
  return (
    <>
      <SettingsCard title={t("settings.wallets.title")} testId="settings-wallet">
        <DashButton variant="plainBlue" data-testid="settings-add-wallet" onClick={() => ui.set({ onboarding: true })}>
          {t("settings.wallets.addWallet")}
        </DashButton>
        {!restoreOnly && (
          <DashButton variant="plainBlue" data-testid="settings-add-watch-only" onClick={() => setSheet({ kind: "watchOnly" })}>
            {t("settings.wallets.addWatchOnly")}
          </DashButton>
        )}
        <DashButton variant="plainBlue" data-testid="settings-recovery-phrase" onClick={() => ui.set({ dialog: "recoveryPhrase", recoveryWallet: null })}>
          {t("settings.wallets.showPhrase")}
        </DashButton>
      </SettingsCard>
      <SettingsCard title={t("settings.wallets.list")} testId="settings-wallets">
        <Done text={notice} testId="settings-wallets-notice" />
        <Problem text={model.error} testId="settings-wallets-error" />
        {model.fallback && <Hint>{WalletsText.noLoadStates}</Hint>}
        {model.loading ? (
          <p className="field-hint" role="status">
            {t("settings.wallets.loading")}
          </p>
        ) : (
          <ul className="settings-wallet-list" data-testid="settings-wallet-list">
            {model.rows.map((row) => (
              <WalletItem
                key={row.walletId}
                row={row}
                model={model}
                onSheet={(kind) => setSheet({ kind, row })}
                onChanged={(message) => {
                  setNotice(message ?? null);
                  refresh();
                }}
              />
            ))}
          </ul>
        )}
      </SettingsCard>
      {sheet?.kind === "rename" && <RenameSheet row={sheet.row} model={model} onClose={closeSheet} />}
      {sheet?.kind === "remove" && <RemoveSheet row={sheet.row} onClose={closeSheet} />}
      {sheet?.kind === "xpub" && <XpubSheet row={sheet.row} onClose={() => setSheet(null)} />}
      {sheet?.kind === "watchOnly" && !restoreOnly && <WatchOnlySheet onClose={closeSheet} />}
    </>
  );
}

function WalletItem({
  row,
  model,
  onSheet,
  onChanged,
}: {
  row: WalletRow;
  model: WalletsModel;
  onSheet: (kind: "rename" | "remove" | "xpub") => void;
  onChanged: (message?: string) => void;
}) {
  const t = useT();
  const id = useId();
  const testId = `settings-wallet-${row.walletId.slice(0, 8)}`;
  const busy = model.busy !== null;
  const closable = model.canClose(row);
  const removable = model.canRemove(row);
  const untickable = model.canUntickStartup(row);
  const cannotClose = row.loaded && !closable && !model.fallback;
  const hint =
    cannotClose && !removable
      ? WalletsText.openAnotherToCloseOrRemove
      : cannotClose
        ? WalletsText.openAnotherToClose
        : row.loaded && !removable
          ? WalletsText.openAnotherToRemove
          : null;
  return (
    <li className="settings-wallet" data-testid={testId} aria-labelledby={`${id}-name`}>
      <div className="settings-wallet-head">
        <span id={`${id}-name`} className="settings-wallet-name">
          {row.name}
        </span>
        <span className="field-hint" data-testid={`${testId}-state`}>
          {row.loaded ? t("settings.wallets.open") : t("settings.wallets.closed")}
        </span>
        {row.watchOnly && <Chip tone="info">{t("settings.wallets.watchOnly")}</Chip>}
      </div>
      <span className="mono field-hint">{row.walletId}</span>
      {!model.fallback && (
        <>
          <label className="checkbox-row">
            <input
              type="checkbox"
              data-testid={`${testId}-startup`}
              checked={row.loadOnStartup}
              disabled={busy || !untickable}
              aria-describedby={untickable ? undefined : `${id}-startup-why`}
              onChange={(e) => void model.setStartup(row.walletId, e.target.checked).then(() => onChanged())}
            />
            {t("settings.wallets.openAtStart")}
          </label>
          {!untickable && (
            <span id={`${id}-startup-why`} className="field-hint" data-testid={`${testId}-startup-why`}>
              {WalletsText.oneOpensAtStart}
            </span>
          )}
        </>
      )}
      {/* The group names the wallet the buttons act on ("Remove…" of which wallet). */}
      <div className="settings-wallet-actions" role="group" aria-labelledby={`${id}-name`}>
        {!model.fallback &&
          (row.loaded ? (
            <DashButton
              variant="tintedGray"
              size="small"
              data-testid={`${testId}-close`}
              disabled={busy || !closable}
              title={closable ? undefined : WalletsText.openAnotherFirst}
              onClick={() => void model.close(row.walletId).then(() => onChanged())}
            >
              {t("settings.wallets.closeWallet")}
            </DashButton>
          ) : (
            <DashButton variant="tintedBlue" size="small" data-testid={`${testId}-open`} disabled={busy} onClick={() => void model.open(row.walletId).then(() => onChanged())}>
              {t("settings.wallets.openWallet")}
            </DashButton>
          ))}
        <DashButton variant="tintedGray" size="small" data-testid={`${testId}-rename`} disabled={busy || !row.loaded} onClick={() => onSheet("rename")}>
          {t("settings.wallets.rename")}
        </DashButton>
        {row.loaded && row.hasMnemonic && (
          <DashButton
            variant="tintedGray"
            size="small"
            data-testid={`${testId}-phrase`}
            disabled={busy}
            onClick={() => ui.set({ dialog: "recoveryPhrase", recoveryWallet: row.walletId })}
          >
            {t("settings.wallets.phrase")}
          </DashButton>
        )}
        <DashButton variant="tintedGray" size="small" data-testid={`${testId}-xpub`} disabled={busy || !row.loaded} onClick={() => onSheet("xpub")}>
          {t("settings.wallets.xpub")}
        </DashButton>
        <DashButton variant="plainRed" size="small" data-testid={`${testId}-remove`} disabled={busy || !removable} onClick={() => onSheet("remove")}>
          {t("settings.wallets.remove")}
        </DashButton>
      </div>
      {hint && <span className="field-hint">{hint}</span>}
    </li>
  );
}

function RenameSheet({ row, model, onClose }: { row: WalletRow; model: WalletsModel; onClose: (message?: string) => void }) {
  const t = useT();
  const [name, setName] = useState(row.name);
  const id = useId();
  return (
    <Sheet title={t("settings.wallets.renameTitle")} width="small" testId="settings-rename-sheet" onClose={() => onClose()}>
      <form
        className="secret-form"
        onSubmit={(e) => {
          e.preventDefault();
          void model.rename(row.walletId, name).then((ok) => ok && onClose());
        }}
      >
        <div className="form-field">
          <label htmlFor={`${id}-name`}>{t("settings.wallets.name")}</label>
          <input id={`${id}-name`} data-testid="settings-rename-field" data-autofocus autoComplete="off" maxLength={64} value={name} onChange={(e) => setName(e.target.value)} />
        </div>
        <Problem text={model.error} testId="settings-rename-error" />
        <DashButton type="submit" variant="filledBlue" size="large" fill data-testid="settings-rename-save" disabled={model.busy !== null}>
          {t("common.save")}
        </DashButton>
      </form>
    </Sheet>
  );
}

function RemoveSheet({ row, onClose }: { row: WalletRow; onClose: (message?: string) => void }) {
  const t = useT();
  const backend = useBackend();
  const [model] = useState(() => new RemoveWalletModel(backend, row.walletId, row.loaded));
  useModel(model);
  useLockTimer(model.lock);
  useEffect(() => {
    void model.load();
    return () => model.cancel();
  }, [model]);
  useEffect(() => {
    if (model.step === "done") {
      if (ui.get().walletId === row.walletId) ui.set({ walletId: null });
      onClose(WalletsText.removed(row.name));
    }
  }, [model.step, onClose, row]);
  if (model.step === "awaiting")
    return (
      <Sheet title={WalletsText.removeAwaitingTitle} width="medium" testId="settings-remove-sheet" focusKey="awaiting" closable={!model.busy} onClose={() => onClose()}>
        <p className="sheet-warning" role="alert" data-testid="settings-remove-awaiting">
          {WalletsText.removeAwaiting(row.name)}
        </p>
        <div className="sheet-actions">
          <DashButton variant="tintedGray" size="large" data-testid="settings-remove-awaiting-cancel" disabled={model.busy} onClick={() => onClose()}>
            {t("common.cancel")}
          </DashButton>
          <DashButton variant="plainRed" size="large" data-testid="settings-remove-anyway" disabled={model.busy} onClick={() => void model.removeAnyway()}>
            {WalletsText.removeAnyway}
          </DashButton>
        </div>
      </Sheet>
    );
  if (model.step === "held")
    return (
      <Sheet title={WalletsText.removeHeldTitle} width="medium" testId="settings-remove-sheet" focusKey="held" onClose={() => onClose()}>
        <p className="sheet-warning" role="alert" data-testid="settings-remove-held">
          {model.held}
        </p>
        <DashButton variant="filledBlue" size="large" fill data-testid="settings-remove-held-ok" onClick={() => onClose()}>
          {WalletsText.removeHeldOk}
        </DashButton>
      </Sheet>
    );
  return (
    <Sheet title={t("settings.wallets.removeTitle")} width="medium" testId="settings-remove-sheet" focusKey={String(model.encrypted)} closable={!model.busy} onClose={() => onClose()}>
      <p className="sheet-warning" role="note">
        {WalletsText.removeQuestion(row)}
      </p>
      {model.encrypted ? (
        <PassphrasePrompt
          testId="settings-remove-passphrase"
          error={model.message}
          busy={model.busy}
          disabled={model.lock.blocked}
          submitLabel={t("settings.wallets.removeConfirm")}
          onSubmit={(p) => model.remove(p)}
        />
      ) : (
        <>
          <Problem text={model.error} testId="settings-remove-error" />
          {/* No autofocus: Enter on opening must not remove the wallet (D35). */}
          <DashButton variant="filledBlue" size="large" fill data-testid="settings-remove-confirm" disabled={model.busy} onClick={() => void model.remove("")}>
            {t("settings.wallets.removeConfirm")}
          </DashButton>
        </>
      )}
    </Sheet>
  );
}

function XpubSheet({ row, onClose }: { row: WalletRow; onClose: () => void }) {
  const t = useT();
  const backend = useBackend();
  const xpub = useQuery({ queryKey: ["accountXpub", row.walletId, 0], queryFn: () => backend.accountXpub(row.walletId, 0) });
  const qr = useQrMatrix(xpub.data?.xpub);
  return (
    <Sheet title={WalletsText.xpubTitle} width="medium" testId="settings-xpub-sheet" onClose={onClose}>
      {xpub.isError && <Problem text={failureText(xpub.error)} testId="settings-xpub-error" />}
      {xpub.data && (
        <>
          <p className="sheet-text">{WalletsText.xpubHint}</p>
          {qr.data && (
            <div className="request-qr">
              <QrCode matrix={qr.data} size={220} label={t("settings.wallets.xpubQr", { name: row.name })} testId="settings-xpub-qr" />
            </div>
          )}
          <dl className="detail-list card-l1">
            <DetailRow label={t("settings.wallets.account")} testId="settings-xpub-path">
              {xpub.data.derivationPath}
            </DetailRow>
            <DetailRow label={t("settings.wallets.key")} testId="settings-xpub-key">
              <CopyValue value={xpub.data.xpub} action={t("settings.wallets.copyXpub")} mono />
            </DetailRow>
          </dl>
        </>
      )}
    </Sheet>
  );
}

function WatchOnlySheet({ onClose }: { onClose: (message?: string) => void }) {
  const t = useT();
  const backend = useBackend();
  const [model] = useState(() => new WatchOnlyModel(backend));
  useModel(model);
  const [xpub, setXpub] = useState("");
  const [name, setName] = useState("");
  const id = useId();
  useEffect(() => {
    if (model.added) onClose(WalletsText.watchOnlyAdded(model.added.name));
  }, [model.added, onClose]);
  return (
    <Sheet title={t("settings.wallets.watchOnlyTitle")} width="medium" testId="settings-watch-only-sheet" closable={!model.working} onClose={() => onClose()}>
      <form
        className="secret-form"
        onSubmit={(e) => {
          e.preventDefault();
          void model.add(xpub, name);
        }}
      >
        <p className="sheet-text">{WalletsText.watchOnlyHint}</p>
        <div className="form-field">
          <label htmlFor={`${id}-xpub`}>{t("settings.wallets.watchOnlyField")}</label>
          <input id={`${id}-xpub`} className="mono" data-testid="settings-watch-only-xpub" data-autofocus autoComplete="off" spellCheck={false} value={xpub} onChange={(e) => setXpub(e.target.value)} />
        </div>
        <div className="form-field">
          <label htmlFor={`${id}-name`}>{t("settings.wallets.watchOnlyName")}</label>
          <input id={`${id}-name`} data-testid="settings-watch-only-name" autoComplete="off" maxLength={64} value={name} onChange={(e) => setName(e.target.value)} />
        </div>
        <Problem text={model.error} testId="settings-watch-only-error" />
        <DashButton type="submit" variant="filledBlue" size="large" fill data-testid="settings-watch-only-add" disabled={model.working}>
          {t("settings.wallets.watchOnlyAdd")}
        </DashButton>
      </form>
    </Sheet>
  );
}
