// Settings ▸ Backups (QT-106…110, QT-116): back up a wallet, the automatic backups, import a
// wallet file (D21) or keys, and export for Dash Core. No file chooser (D19): files are written
// into the backup folder and read from a typed path.

import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useState } from "react";
import type { CoreExportFormatName } from "../../api/generated/CoreExportFormatName";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { keys, useAutomaticBackupFailures, useAutomaticBackups, useBackend, useBackupDirectory, useBackupPolicy, useLockStatus, useRestored, useWallets } from "../../api/queries";
import { useLaunch } from "../../app/App";
import { chooseFilePath, fileErrorText } from "../../app/files";
import { keptAcrossLock, useAcrossLock } from "../../app/acrossLock";
import { useSecretScreen } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { CopyValue, DetailRow } from "../../components/detail";
import { NewPassphraseForm, PassphrasePrompt } from "../../components/passphrase";
import { ProtectionPending, ProtectionText } from "../../components/phrase";
import { Sheet } from "../../components/sheet";
import { detailDate } from "../../format/tx";
import { useT } from "../../i18n";
import { failureText, useModel } from "../../onboarding/model";
import { BackupModel, deletedByKeep, ExportModel, FilesText, folderErrorText, ImportFileModel, ImportKeysModel, keepAsks, KEY_KINDS, type ImportDone, type KeyKind } from "../../settings/files";
import { useLockTimer } from "../LockScreen";
import { Done, Hint, pendingText, Problem, SettingsCard } from "./parts";

type BackupsSheet = { kind: "backup" | "export"; wallet: WalletSummary } | { kind: "importFile" | "importKeys" } | null;

/** A sheet whose file was being written or read, or whose result was up, when the vault locked:
 * it opens again with the same model after the unlock, so where the file went, or why it failed,
 * is still said (app/acrossLock.ts). */
type Kept<M> = { sheet: NonNullable<BackupsSheet>; model: M };
const SLOT = "settings-backups";

function useKeptSheet<M>(sheet: NonNullable<BackupsSheet>, make: () => M, holds: (m: M) => boolean, end?: (m: M) => void): M {
  return useAcrossLock<Kept<M>>(SLOT, () => ({ sheet, model: make() }), (k) => holds(k.model), end && ((k) => end(k.model))).model;
}

const KEEP_CHOICES = Array.from({ length: 11 }, (_, i) => i);

export function BackupsTab() {
  const wallets = useWallets();
  const [sheet, setSheet] = useState<BackupsSheet>(() => keptAcrossLock<Kept<unknown>>(SLOT)?.sheet ?? null);
  const [walletId, setWalletId] = useState<string | null>(null);
  const id = useId();
  const restoreOnly = useRestored().data === true;
  const own = (wallets.data ?? []).filter((w) => !w.watchOnly);
  const chosen = own.find((w) => w.walletId === walletId) ?? own[0];
  return (
    <>
      <SettingsCard title="Back up and export" testId="settings-backup-card">
        {own.length === 0 ? (
          <Hint>No wallet on this network has keys to back up.</Hint>
        ) : (
          <>
            {own.length > 1 && (
              <div className="settings-row">
                <label className="settings-row-title" htmlFor={`${id}-wallet`}>
                  Wallet
                </label>
                <select id={`${id}-wallet`} className="settings-select" data-testid="settings-backup-wallet" value={chosen?.walletId} onChange={(e) => setWalletId(e.target.value)}>
                  {own.map((w) => (
                    <option key={w.walletId} value={w.walletId}>
                      {w.name}
                    </option>
                  ))}
                </select>
              </div>
            )}
            <DashButton variant="plainBlue" data-testid="settings-backup" onClick={() => chosen && setSheet({ kind: "backup", wallet: chosen })}>
              Back up wallet…
            </DashButton>
            <DashButton variant="plainBlue" data-testid="settings-export" onClick={() => chosen && setSheet({ kind: "export", wallet: chosen })}>
              Export for Dash Core…
            </DashButton>
          </>
        )}
      </SettingsCard>
      <AutomaticBackupsCard names={new Map((wallets.data ?? []).map((w) => [w.walletId, w.name]))} own={own} onBackUp={(wallet) => setSheet({ kind: "backup", wallet })} />
      {!restoreOnly && (
        <SettingsCard title="Import" testId="settings-import-card">
          <Hint>A Dash Wallet backup, a Dash Core dumpwallet file or wallet.dat, or raw keys.</Hint>
          <DashButton variant="plainBlue" data-testid="settings-import-file" onClick={() => setSheet({ kind: "importFile" })}>
            Import a wallet file…
          </DashButton>
          <DashButton variant="plainBlue" data-testid="settings-import-keys" onClick={() => setSheet({ kind: "importKeys" })}>
            Import keys…
          </DashButton>
        </SettingsCard>
      )}
      {sheet?.kind === "backup" && <BackupSheet wallet={sheet.wallet} onClose={() => setSheet(null)} />}
      {sheet?.kind === "export" && <ExportSheet wallet={sheet.wallet} onClose={() => setSheet(null)} />}
      {sheet?.kind === "importFile" && !restoreOnly && <ImportFileSheet onClose={() => setSheet(null)} />}
      {sheet?.kind === "importKeys" && !restoreOnly && <ImportKeysSheet onClose={() => setSheet(null)} />}
    </>
  );
}

function AutomaticBackupsCard({ names, own, onBackUp }: { names: Map<string, string>; own: WalletSummary[]; onBackUp: (wallet: WalletSummary) => void }) {
  const backend = useBackend();
  const client = useQueryClient();
  const policy = useBackupPolicy();
  const directory = useBackupDirectory();
  const backups = useAutomaticBackups();
  const [problem, setProblem] = useState<string | null>(null);
  const t = useT();
  const [folderProblem, setFolderProblem] = useState<string | null>(null);
  const showFolder = async () => {
    setFolderProblem(null);
    try {
      await backend.openBackupDirectory();
    } catch (e) {
      setFolderProblem(folderErrorText(e));
    }
  };
  /** A lower count waiting for its confirmation: it deletes files at once (D30). */
  const [asking, setAsking] = useState<{ keep: number; deleted: number | null } | null>(null);
  const [saving, setSaving] = useState(false);
  const id = useId();
  const setKeep = async (keep: number) => {
    setProblem(null);
    setSaving(true);
    try {
      await backend.setBackupPolicy(keep);
    } catch (e) {
      setProblem(failureText(e));
    }
    await client.invalidateQueries({ queryKey: keys.backupsAll });
    setSaving(false);
    setAsking(null);
  };
  const choose = (keep: number) => {
    // Every lower count asks, even when the list shows nothing to delete (D42). A list still
    // being read is not counted (D30).
    if (!keepAsks(policy.data?.keep ?? 0, keep)) return void setKeep(keep);
    setAsking({ keep, deleted: deletedByKeep(backups.isFetching ? undefined : backups.data, keep) });
  };
  return (
    <SettingsCard title="Automatic backups" testId="settings-automatic-backups">
      <AutomaticBackupFailures own={own} onBackUp={onBackUp} />
      <div className="settings-row">
        <label className="settings-row-title" htmlFor={`${id}-keep`}>
          Backups to keep for each wallet
        </label>
        <select
          id={`${id}-keep`}
          className="settings-select"
          data-testid="settings-backup-keep"
          value={policy.data?.keep ?? ""}
          disabled={!policy.data || saving}
          onChange={(e) => choose(Number(e.target.value))}
        >
          {KEEP_CHOICES.map((n) => (
            <option key={n} value={n}>
              {n === 0 ? "None (off)" : n}
            </option>
          ))}
        </select>
      </div>
      <Problem text={problem ?? (policy.isError ? failureText(policy.error) : null)} testId="settings-backup-policy-error" />
      {asking && (
        <Sheet title={FilesText.keepTitle} width="small" testId="settings-backup-keep-sheet" closable={!saving} onClose={() => setAsking(null)}>
          <p className="sheet-warning" role="note">
            {FilesText.keepDeletes(asking.keep, asking.deleted, directory.data ?? "the backup folder")}
          </p>
          <div className="sheet-actions">
            <DashButton variant="tintedGray" data-autofocus disabled={saving} onClick={() => setAsking(null)}>
              Cancel
            </DashButton>
            <DashButton variant="plainRed" data-testid="settings-backup-keep-confirm" disabled={saving} onClick={() => void setKeep(asking.keep)}>
              {FilesText.keepConfirm}
            </DashButton>
          </div>
        </Sheet>
      )}
      <dl className="detail-list settings-details">
        <DetailRow label="Folder" testId="settings-backup-folder">
          {directory.data ? <CopyValue value={directory.data} label="backup folder" mono /> : pendingText(directory)}
        </DetailRow>
      </dl>
      <div className="settings-row">
        <DashButton variant="plainBlue" data-testid="settings-show-folder" onClick={() => void showFolder()}>
          {t("settings.backups.showFolder")}
        </DashButton>
      </div>
      <Problem text={folderProblem} testId="settings-show-folder-error" />
      {backups.data && backups.data.length === 0 && <Hint>No automatic backup yet.</Hint>}
      {backups.data && backups.data.length > 0 && (
        <ul className="settings-backup-list" data-testid="settings-backup-list">
          {backups.data.map((b) => (
            <li key={b.path} className="settings-backup">
              <span>
                {names.get(b.walletId) ?? `${b.walletId.slice(0, 8)}…`} · {detailDate(new Date(b.createdAt * 1000), backend.locale)}
              </span>
              <CopyValue value={b.path} label="backup path" mono />
            </li>
          ))}
        </ul>
      )}
      {backups.isError && <Problem text={failureText(backups.error)} testId="settings-backup-list-error" />}
    </SettingsCard>
  );
}

/** Each wallet whose latest automatic backup failed, until one succeeds (issue 340). */
function AutomaticBackupFailures({ own, onBackUp }: { own: WalletSummary[]; onBackUp: (wallet: WalletSummary) => void }) {
  const t = useT();
  const backend = useBackend();
  const failures = useAutomaticBackupFailures();
  return (failures.data ?? []).map(({ walletId, failedAt }) => {
    const wallet = own.find((w) => w.walletId === walletId);
    if (!wallet) return null;
    const date = detailDate(new Date(failedAt * 1000), backend.locale);
    return (
      <div key={walletId} className="settings-backup-failed" data-testid="settings-backup-failed">
        <p className="settings-backup-failed-title">{t("settings.backups.automaticFailed.title")}</p>
        <p>{t("settings.backups.automaticFailed.body", { wallet: wallet.name, date })}</p>
        <DashButton variant="plainBlue" data-testid="settings-backup-failed-back-up" onClick={() => onBackUp(wallet)}>
          {t("settings.backups.backUp")}
        </DashButton>
      </div>
    );
  });
}

function BackupSheet({ wallet, onClose }: { wallet: WalletSummary; onClose: () => void }) {
  const backend = useBackend();
  const client = useQueryClient();
  const lock = useLockStatus();
  const model = useKeptSheet({ kind: "backup", wallet }, () => new BackupModel(backend, () => backend.now()), (m) => m.working || m.done !== null);
  useModel(model);
  const encrypted = lock.data?.encrypted !== false;
  useEffect(() => {
    if (model.done) void client.invalidateQueries({ queryKey: keys.backupsAll });
  }, [model.done, client]);
  return (
    <Sheet title={`Back up ${wallet.name}`} width="medium" testId="settings-backup-sheet" focusKey={String(!!model.done)} closable={!model.working} onClose={onClose}>
      {model.done ? (
        <>
          <Done text={FilesText.backedUp(model.done.path)} testId="settings-backup-done" />
          <dl className="detail-list card-l1">
            <DetailRow label="File" testId="settings-backup-path">
              <CopyValue value={model.done.path} label="backup path" mono />
            </DetailRow>
          </dl>
          <DashButton variant="filledBlue" size="large" fill data-autofocus onClick={onClose}>
            Done
          </DashButton>
        </>
      ) : encrypted ? (
        <>
          <p className="sheet-text">{FilesText.backupUnlocked}</p>
          <Problem text={model.error} testId="settings-backup-error" />
          <DashButton
            variant="filledBlue"
            size="large"
            fill
            data-autofocus
            data-testid="settings-backup-write"
            disabled={model.working}
            onClick={() => void model.backup(wallet.walletId, wallet.name, null, null)}
          >
            Write backup
          </DashButton>
        </>
      ) : (
        <>
          <p className="sheet-text">{FilesText.backupPassphraseHint}</p>
          <NewPassphraseForm
            testId="settings-backup-passphrase"
            error={model.error}
            submitLabel="Write backup"
            onEdit={() => model.edited()}
            onSubmit={(text, confirmation) => model.backup(wallet.walletId, wallet.name, text, confirmation)}
          />
        </>
      )}
    </Sheet>
  );
}

function DoneLines({ done, onClose }: { done: ImportDone; onClose: () => void }) {
  return (
    <>
      {done.lines.map((l, i) => (
        <Done key={l} text={l} testId={`settings-import-done-${i}`} />
      ))}
      <DashButton variant="filledBlue" size="large" fill data-autofocus data-testid="settings-import-close" onClick={onClose}>
        Done
      </DashButton>
    </>
  );
}

function ImportFileSheet({ onClose }: { onClose: () => void }) {
  const backend = useBackend();
  const { network } = useLaunch();
  const model = useKeptSheet({ kind: "importFile" }, () => new ImportFileModel(backend, network.dirName), (m) => m.step === "working" || m.step === "done");
  useModel(model);
  const [path, setPath] = useState("");
  const [passphrase, setPassphrase] = useState("");
  const [name, setName] = useState("");
  const id = useId();
  const plan = model.plan;
  const working = model.step === "inspecting" || model.step === "working";
  const t = useT();
  // The system's open dialog fills the path; a cancel leaves it as it was.
  const browse = async () => {
    setBrowseProblem(null);
    model.clearError();
    try {
      const chosen = await chooseFilePath(backend, "walletFile", false, null);
      if (chosen) setPath(chosen);
    } catch (e) {
      setBrowseProblem(fileErrorText(e));
    }
  };
  const [browseProblem, setBrowseProblem] = useState<string | null>(null);
  return (
    <Sheet title="Import a wallet file" width="medium" testId="settings-import-file-sheet" focusKey={model.step} closable={!working} onClose={onClose}>
      {(model.step === "path" || model.step === "inspecting") && (
        <form
          className="secret-form"
          onSubmit={(e) => {
            e.preventDefault();
            setBrowseProblem(null);
            void model.inspect(path);
          }}
        >
          <div className="form-field">
            <label htmlFor={`${id}-path`}>The file's full path</label>
            <span className="amount-entry">
              <input id={`${id}-path`} className="mono" data-testid="settings-import-path" data-autofocus autoComplete="off" spellCheck={false} value={path} onChange={(e) => setPath(e.target.value)} />
              <DashButton variant="tintedBlue" size="small" data-testid="settings-import-browse" disabled={working} onClick={() => void browse()}>
                {t("settings.backups.browse")}
              </DashButton>
            </span>
          </div>
          <Hint>{t("settings.backups.pathHint")}</Hint>
          <Problem text={browseProblem ?? model.error} testId="settings-import-error" />
          <DashButton type="submit" variant="filledBlue" size="large" fill data-testid="settings-import-inspect" disabled={working}>
            Continue
          </DashButton>
        </form>
      )}
      {(model.step === "details" || model.step === "working") && plan && (
        <form
          className="secret-form"
          onSubmit={(e) => {
            e.preventDefault();
            const p = passphrase;
            setPassphrase("");
            void model.run(plan.passphrase ? p : null, name);
          }}
        >
          <p className="sheet-text" data-testid="settings-import-kind">
            {plan.summary}
          </p>
          {plan.refusal ? (
            <p className="sheet-warning" role="alert" data-testid="settings-import-refusal">
              {plan.refusal}
            </p>
          ) : (
            <>
              {plan.passphrase && (
                <div className="form-field">
                  <label htmlFor={`${id}-pass`}>{plan.passphrase}</label>
                  <input
                    id={`${id}-pass`}
                    type="password"
                    autoComplete="off"
                    data-testid="settings-import-passphrase"
                    data-autofocus
                    value={passphrase}
                    onChange={(e) => setPassphrase(e.target.value)}
                  />
                </div>
              )}
              {plan.named && (
                <div className="form-field">
                  <label htmlFor={`${id}-name`}>Wallet name (optional)</label>
                  <input id={`${id}-name`} data-testid="settings-import-name" autoComplete="off" maxLength={64} value={name} onChange={(e) => setName(e.target.value)} />
                </div>
              )}
            </>
          )}
          <Problem text={model.error} testId="settings-import-error" />
          <div className="sheet-actions">
            <DashButton variant="tintedGray" data-testid="settings-import-back" disabled={working} onClick={() => model.back()}>
              Back
            </DashButton>
            {!plan.refusal && (
              <DashButton type="submit" variant="filledBlue" data-testid="settings-import-run" disabled={working}>
                {plan.kind === "dwBackup" ? "Restore" : "Import"}
              </DashButton>
            )}
          </div>
        </form>
      )}
      {model.step === "done" && model.done && <DoneLines done={model.done} onClose={onClose} />}
    </Sheet>
  );
}

function ImportKeysSheet({ onClose }: { onClose: () => void }) {
  const backend = useBackend();
  const model = useKeptSheet({ kind: "importKeys" }, () => new ImportKeysModel(backend), (m) => m.working || m.done !== null);
  useModel(model);
  const [kind, setKind] = useState<KeyKind>("hdSeed");
  const [text, setText] = useState("");
  const [name, setName] = useState("");
  const id = useId();
  // A seed or a private key: the sheet stays empty until the window is kept out of screen captures (D15, D27).
  const protection = useSecretScreen();
  return (
    <Sheet title="Import keys" width="medium" testId="settings-import-keys-sheet" focusKey={`${!!model.done} ${protection}`} closable={!model.working} onClose={onClose}>
      {protection !== "on" ? (
        <ProtectionPending protection={protection} failedText={ProtectionText.failedKeys} />
      ) : model.done ? (
        <DoneLines done={model.done} onClose={onClose} />
      ) : (
        <form
          className="secret-form"
          onSubmit={(e) => {
            e.preventDefault();
            const t = text;
            setText("");
            void model.run(kind, t, name);
          }}
        >
          <div className="segmented" role="group" aria-label="Kind of key">
            {KEY_KINDS.map((k) => (
              <button key={k.kind} type="button" className="segment" data-testid={`settings-keys-${k.kind}`} aria-pressed={kind === k.kind} onClick={() => setKind(k.kind)}>
                {k.label}
              </button>
            ))}
          </div>
          <div className="form-field">
            <label htmlFor={`${id}-keys`}>{KEY_KINDS.find((k) => k.kind === kind)!.label}</label>
            <textarea
              id={`${id}-keys`}
              className="mono settings-textarea"
              data-testid="settings-keys-text"
              data-autofocus
              autoComplete="off"
              spellCheck={false}
              rows={kind === "descriptors" ? 6 : 2}
              value={text}
              onChange={(e) => setText(e.target.value)}
            />
          </div>
          <div className="form-field">
            <label htmlFor={`${id}-name`}>Wallet name (optional)</label>
            <input id={`${id}-name`} data-testid="settings-keys-name" autoComplete="off" maxLength={64} value={name} onChange={(e) => setName(e.target.value)} />
          </div>
          <Problem text={model.error} testId="settings-keys-error" />
          <DashButton type="submit" variant="filledBlue" size="large" fill data-testid="settings-keys-import" disabled={model.working}>
            Import
          </DashButton>
        </form>
      )}
    </Sheet>
  );
}

function ExportSheet({ wallet, onClose }: { wallet: WalletSummary; onClose: () => void }) {
  const backend = useBackend();
  const model = useKeptSheet(
    { kind: "export", wallet },
    () => new ExportModel(backend, () => backend.now(), wallet.walletId, wallet.name),
    (m) => m.step === "working" || m.step === "done",
    (m) => m.cancel(),
  );
  useModel(model);
  useLockTimer(model.lock);
  useEffect(() => {
    if (model.step === "loading") void model.load();
  }, [model]);
  const [format, setFormat] = useState<CoreExportFormatName>("dumpWallet");
  return (
    <Sheet title={`Export ${wallet.name} for Dash Core`} width="medium" testId="settings-export-sheet" focusKey={model.step} closable={!model.busy} onClose={onClose}>
      {model.step === "loading" && (
        <p className="sheet-text" role="status">
          Checking…
        </p>
      )}
      {(model.step === "confirm" || model.step === "working") && (
        <>
          <div className="segmented" role="group" aria-label="File format">
            <button type="button" className="segment" data-testid="settings-export-dumpwallet" aria-pressed={format === "dumpWallet"} onClick={() => setFormat("dumpWallet")}>
              dumpwallet file
            </button>
            <button
              type="button"
              className="segment"
              data-testid="settings-export-descriptors"
              aria-pressed={format === "importDescriptorsJson"}
              onClick={() => setFormat("importDescriptorsJson")}
            >
              importdescriptors JSON
            </button>
          </div>
          <p className="sheet-warning" role="note">
            {FilesText.exportWarning}
          </p>
          {model.warnings.map((w) => (
            <p key={w} className="sheet-warning" data-testid={`settings-export-warning-${w}`}>
              {FilesText.warnings[w]}
            </p>
          ))}
          {model.encrypted ? (
            <PassphrasePrompt
              testId="settings-export-passphrase"
              error={model.message}
              busy={model.busy}
              disabled={model.lock.blocked}
              submitLabel="Export"
              onSubmit={(p) => model.export(format, p)}
            />
          ) : (
            <>
              <Problem text={model.error} testId="settings-export-error" />
              {/* No autofocus: Enter on opening must not write the keys out (D35). */}
              <DashButton variant="filledBlue" size="large" fill data-testid="settings-export-confirm" disabled={model.busy} onClick={() => void model.export(format, "")}>
                Export
              </DashButton>
            </>
          )}
        </>
      )}
      {model.step === "done" && model.done && (
        <>
          <Done text={FilesText.exported(model.done.keyCount, model.done.path)} testId="settings-export-done" />
          {model.done.warnings.map((w) => (
            <p key={w} className="sheet-warning" data-testid={`settings-export-done-warning-${w}`}>
              {FilesText.warnings[w]}
            </p>
          ))}
          <dl className="detail-list card-l1">
            <DetailRow label="File" testId="settings-export-path">
              <CopyValue value={model.done.path} label="export path" mono />
            </DetailRow>
          </dl>
          <DashButton variant="filledBlue" size="large" fill data-autofocus onClick={onClose}>
            Done
          </DashButton>
        </>
      )}
    </Sheet>
  );
}
