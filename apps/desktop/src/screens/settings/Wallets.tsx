// Settings ▸ Wallets (UX-SPEC §4.17, QT-101, QT-102, QT-114, IOS-110, IOS-111): U-02 D23's
// Wallet card grown into the list of every registered wallet (D17), with Open/Close, the startup
// list, Rename, Remove (D18), the extended public key and a watch-only wallet.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useState } from "react";
import { keys, useBackend, useQrMatrix, useWallets } from "../../api/queries";
import { ui } from "../../app/ui";
import { Chip, DashButton } from "../../components/basics";
import { CopyValue, DetailRow } from "../../components/detail";
import { PassphrasePrompt } from "../../components/passphrase";
import { QrCode } from "../../components/qr";
import { Sheet } from "../../components/sheet";
import { failureText, useModel } from "../../onboarding/model";
import { RemoveWalletModel, WalletsModel, WalletsText, WatchOnlyModel, type WalletRow } from "../../settings/wallets";
import { useLockTimer } from "../LockScreen";
import { Done, Hint, Problem, SettingsCard } from "./parts";

type WalletSheet = { kind: "rename" | "remove" | "xpub"; row: WalletRow } | { kind: "watchOnly" } | null;

export function WalletsTab() {
  const backend = useBackend();
  const client = useQueryClient();
  const [model] = useState(() => new WalletsModel(backend));
  useModel(model);
  // Every wallet event refreshes the list too: the shell's list and this one move together.
  const wallets = useWallets();
  useEffect(() => void model.load(), [model, wallets.dataUpdatedAt]);
  const [sheet, setSheet] = useState<WalletSheet>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const refresh = () => void client.invalidateQueries({ queryKey: keys.wallets });
  const closeSheet = (message?: string) => {
    setSheet(null);
    if (message) setNotice(message);
    refresh();
  };
  return (
    <>
      <SettingsCard title="Wallet" testId="settings-wallet">
        <DashButton variant="plainBlue" data-testid="settings-add-wallet" onClick={() => ui.set({ onboarding: true })}>
          Add wallet…
        </DashButton>
        <DashButton variant="plainBlue" data-testid="settings-add-watch-only" onClick={() => setSheet({ kind: "watchOnly" })}>
          Add watch-only wallet…
        </DashButton>
        <DashButton variant="plainBlue" data-testid="settings-recovery-phrase" onClick={() => ui.set({ dialog: "recoveryPhrase", recoveryWallet: null })}>
          Show recovery phrase…
        </DashButton>
      </SettingsCard>
      <SettingsCard title="Wallets on this network" testId="settings-wallets">
        <Done text={notice} testId="settings-wallets-notice" />
        <Problem text={model.error} testId="settings-wallets-error" />
        {model.fallback && <Hint>{WalletsText.noLoadStates}</Hint>}
        {model.loading ? (
          <p className="field-hint" role="status">
            Loading…
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
      {sheet?.kind === "watchOnly" && <WatchOnlySheet onClose={closeSheet} />}
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
  const id = useId();
  const testId = `settings-wallet-${row.walletId.slice(0, 8)}`;
  const busy = model.busy !== null;
  const closable = model.canClose(row);
  const removable = model.canRemove(row);
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
          {row.loaded ? "Open" : "Closed"}
        </span>
        {row.watchOnly && <Chip tone="info">Watch-only</Chip>}
      </div>
      <span className="mono field-hint">{row.walletId}</span>
      {!model.fallback && (
        <label className="checkbox-row">
          <input
            type="checkbox"
            data-testid={`${testId}-startup`}
            checked={row.loadOnStartup}
            disabled={busy}
            onChange={(e) => void model.setStartup(row.walletId, e.target.checked).then(() => onChanged())}
          />
          Open when Dash Wallet starts
        </label>
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
              Close
            </DashButton>
          ) : (
            <DashButton variant="tintedBlue" size="small" data-testid={`${testId}-open`} disabled={busy} onClick={() => void model.open(row.walletId).then(() => onChanged())}>
              Open
            </DashButton>
          ))}
        <DashButton variant="tintedGray" size="small" data-testid={`${testId}-rename`} disabled={busy || !row.loaded} onClick={() => onSheet("rename")}>
          Rename…
        </DashButton>
        {row.loaded && row.hasMnemonic && (
          <DashButton
            variant="tintedGray"
            size="small"
            data-testid={`${testId}-phrase`}
            disabled={busy}
            onClick={() => ui.set({ dialog: "recoveryPhrase", recoveryWallet: row.walletId })}
          >
            Recovery phrase…
          </DashButton>
        )}
        <DashButton variant="tintedGray" size="small" data-testid={`${testId}-xpub`} disabled={busy || !row.loaded} onClick={() => onSheet("xpub")}>
          Extended public key…
        </DashButton>
        <DashButton variant="plainRed" size="small" data-testid={`${testId}-remove`} disabled={busy || !removable} onClick={() => onSheet("remove")}>
          Remove…
        </DashButton>
      </div>
      {hint && <span className="field-hint">{hint}</span>}
    </li>
  );
}

function RenameSheet({ row, model, onClose }: { row: WalletRow; model: WalletsModel; onClose: (message?: string) => void }) {
  const [name, setName] = useState(row.name);
  const id = useId();
  return (
    <Sheet title="Rename wallet" width="small" testId="settings-rename-sheet" onClose={() => onClose()}>
      <form
        className="secret-form"
        onSubmit={(e) => {
          e.preventDefault();
          void model.rename(row.walletId, name).then((ok) => ok && onClose());
        }}
      >
        <div className="form-field">
          <label htmlFor={`${id}-name`}>Name</label>
          <input id={`${id}-name`} data-testid="settings-rename-field" data-autofocus autoComplete="off" maxLength={64} value={name} onChange={(e) => setName(e.target.value)} />
        </div>
        <Problem text={model.error} testId="settings-rename-error" />
        <DashButton type="submit" variant="filledBlue" size="large" fill data-testid="settings-rename-save" disabled={model.busy !== null}>
          Save
        </DashButton>
      </form>
    </Sheet>
  );
}

function RemoveSheet({ row, onClose }: { row: WalletRow; onClose: (message?: string) => void }) {
  const backend = useBackend();
  const [model] = useState(() => new RemoveWalletModel(backend, row.walletId));
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
  return (
    <Sheet title="Remove wallet" width="medium" testId="settings-remove-sheet" focusKey={String(model.encrypted)} closable={!model.busy} onClose={() => onClose()}>
      <p className="sheet-warning" role="note">
        {WalletsText.removeQuestion(row.name, row.watchOnly)}
      </p>
      {model.encrypted ? (
        <PassphrasePrompt
          testId="settings-remove-passphrase"
          error={model.message}
          busy={model.busy}
          disabled={model.lock.blocked}
          submitLabel="Remove wallet"
          onSubmit={(p) => model.remove(p)}
        />
      ) : (
        <>
          <Problem text={model.error} testId="settings-remove-error" />
          {/* No autofocus: Enter on opening must not remove the wallet (D35). */}
          <DashButton variant="filledBlue" size="large" fill data-testid="settings-remove-confirm" disabled={model.busy} onClick={() => void model.remove("")}>
            Remove wallet
          </DashButton>
        </>
      )}
    </Sheet>
  );
}

function XpubSheet({ row, onClose }: { row: WalletRow; onClose: () => void }) {
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
              <QrCode matrix={qr.data} size={220} label={`QR code of the extended public key of ${row.name}`} testId="settings-xpub-qr" />
            </div>
          )}
          <dl className="detail-list card-l1">
            <DetailRow label="Account" testId="settings-xpub-path">
              {xpub.data.derivationPath}
            </DetailRow>
            <DetailRow label="Key" testId="settings-xpub-key">
              <CopyValue value={xpub.data.xpub} label="extended public key" mono />
            </DetailRow>
          </dl>
        </>
      )}
    </Sheet>
  );
}

function WatchOnlySheet({ onClose }: { onClose: (message?: string) => void }) {
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
    <Sheet title="Add watch-only wallet" width="medium" testId="settings-watch-only-sheet" closable={!model.working} onClose={() => onClose()}>
      <form
        className="secret-form"
        onSubmit={(e) => {
          e.preventDefault();
          void model.add(xpub, name);
        }}
      >
        <p className="sheet-text">{WalletsText.watchOnlyHint}</p>
        <div className="form-field">
          <label htmlFor={`${id}-xpub`}>Extended public key</label>
          <input id={`${id}-xpub`} className="mono" data-testid="settings-watch-only-xpub" data-autofocus autoComplete="off" spellCheck={false} value={xpub} onChange={(e) => setXpub(e.target.value)} />
        </div>
        <div className="form-field">
          <label htmlFor={`${id}-name`}>Name (optional)</label>
          <input id={`${id}-name`} data-testid="settings-watch-only-name" autoComplete="off" maxLength={64} value={name} onChange={(e) => setName(e.target.value)} />
        </div>
        <Problem text={model.error} testId="settings-watch-only-error" />
        <DashButton type="submit" variant="filledBlue" size="large" fill data-testid="settings-watch-only-add" disabled={model.working}>
          Add wallet
        </DashButton>
      </form>
    </Sheet>
  );
}
