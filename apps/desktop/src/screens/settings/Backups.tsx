// Settings ▸ Backups (QT-106…110, QT-116): back up a wallet, the automatic backups, import a
// wallet file (D21) or keys, and export for Dash Core. No file chooser (D19): files are written
// into the backup folder and read from a typed path.

import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useState } from "react";
import type { CoreExportFormatName } from "../../api/generated/CoreExportFormatName";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { keys, useAutomaticBackups, useBackend, useBackupDirectory, useBackupPolicy, useLockStatus, useWallets } from "../../api/queries";
import { useLaunch } from "../../app/App";
import { DashButton } from "../../components/basics";
import { CopyValue, DetailRow } from "../../components/detail";
import { NewPassphraseForm, PassphrasePrompt } from "../../components/passphrase";
import { Sheet } from "../../components/sheet";
import { detailDate } from "../../format/tx";
import { failureText, useModel } from "../../onboarding/model";
import { BackupModel, ExportModel, FilesText, ImportFileModel, ImportKeysModel, KEY_KINDS, type ImportDone, type KeyKind } from "../../settings/files";
import { useLockTimer } from "../LockScreen";
import { Done, Hint, NotYetRow, Problem, SettingsCard } from "./parts";

type BackupsSheet = { kind: "backup" | "export"; wallet: WalletSummary } | { kind: "importFile" | "importKeys" } | null;

const KEEP_CHOICES = Array.from({ length: 11 }, (_, i) => i);

export function BackupsTab() {
  const wallets = useWallets();
  const [sheet, setSheet] = useState<BackupsSheet>(null);
  const [walletId, setWalletId] = useState<string | null>(null);
  const id = useId();
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
      <AutomaticBackupsCard names={new Map((wallets.data ?? []).map((w) => [w.walletId, w.name]))} />
      <SettingsCard title="Import" testId="settings-import-card">
        <Hint>A Dash Wallet backup, a Dash Core dumpwallet file or wallet.dat, or raw keys.</Hint>
        <DashButton variant="plainBlue" data-testid="settings-import-file" onClick={() => setSheet({ kind: "importFile" })}>
          Import a wallet file…
        </DashButton>
        <DashButton variant="plainBlue" data-testid="settings-import-keys" onClick={() => setSheet({ kind: "importKeys" })}>
          Import keys…
        </DashButton>
      </SettingsCard>
      {sheet?.kind === "backup" && <BackupSheet wallet={sheet.wallet} onClose={() => setSheet(null)} />}
      {sheet?.kind === "export" && <ExportSheet wallet={sheet.wallet} onClose={() => setSheet(null)} />}
      {sheet?.kind === "importFile" && <ImportFileSheet onClose={() => setSheet(null)} />}
      {sheet?.kind === "importKeys" && <ImportKeysSheet onClose={() => setSheet(null)} />}
    </>
  );
}

function AutomaticBackupsCard({ names }: { names: Map<string, string> }) {
  const backend = useBackend();
  const client = useQueryClient();
  const policy = useBackupPolicy();
  const directory = useBackupDirectory();
  const backups = useAutomaticBackups();
  const [problem, setProblem] = useState<string | null>(null);
  const id = useId();
  const setKeep = async (keep: number) => {
    setProblem(null);
    try {
      await backend.setBackupPolicy(keep);
    } catch (e) {
      setProblem(failureText(e));
    }
    await client.invalidateQueries({ queryKey: keys.backupsAll });
  };
  return (
    <SettingsCard title="Automatic backups" testId="settings-automatic-backups">
      <div className="settings-row">
        <label className="settings-row-title" htmlFor={`${id}-keep`}>
          Backups to keep for each wallet
        </label>
        <select
          id={`${id}-keep`}
          className="settings-select"
          data-testid="settings-backup-keep"
          value={policy.data?.keep ?? ""}
          disabled={!policy.data}
          onChange={(e) => void setKeep(Number(e.target.value))}
        >
          {KEEP_CHOICES.map((n) => (
            <option key={n} value={n}>
              {n === 0 ? "None (off)" : n}
            </option>
          ))}
        </select>
      </div>
      <Problem text={problem ?? (policy.isError ? failureText(policy.error) : null)} testId="settings-backup-policy-error" />
      <dl className="detail-list settings-details">
        <DetailRow label="Folder" testId="settings-backup-folder">
          {directory.data ? <CopyValue value={directory.data} label="backup folder" mono /> : directory.isError ? failureText(directory.error) : "…"}
        </DetailRow>
      </dl>
      <NotYetRow testId="settings-show-folder" title="Show folder" why="the app cannot open folders yet (a native file chooser is a follow-up). Copy the path instead." />
      {backups.data && backups.data.length === 0 && <Hint>No automatic backup yet.</Hint>}
      {backups.data && backups.data.length > 0 && (
        <ul className="settings-backup-list" data-testid="settings-backup-list">
          {backups.data.map((b) => (
            <li key={b.path} className="settings-backup">
              <span>
                {names.get(b.walletId) ?? `${b.walletId.slice(0, 8)}…`} · {detailDate(new Date(b.createdAt * 1000))}
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

function BackupSheet({ wallet, onClose }: { wallet: WalletSummary; onClose: () => void }) {
  const backend = useBackend();
  const client = useQueryClient();
  const lock = useLockStatus();
  const [model] = useState(() => new BackupModel(backend, () => backend.now()));
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
  const [model] = useState(() => new ImportFileModel(backend, network.dirName));
  useModel(model);
  const [path, setPath] = useState("");
  const [passphrase, setPassphrase] = useState("");
  const [name, setName] = useState("");
  const id = useId();
  const plan = model.plan;
  const working = model.step === "inspecting" || model.step === "working";
  return (
    <Sheet title="Import a wallet file" width="medium" testId="settings-import-file-sheet" focusKey={model.step} closable={!working} onClose={onClose}>
      {(model.step === "path" || model.step === "inspecting") && (
        <form
          className="secret-form"
          onSubmit={(e) => {
            e.preventDefault();
            void model.inspect(path);
          }}
        >
          <div className="form-field">
            <label htmlFor={`${id}-path`}>The file's full path</label>
            <input id={`${id}-path`} className="mono" data-testid="settings-import-path" data-autofocus autoComplete="off" spellCheck={false} value={path} onChange={(e) => setPath(e.target.value)} />
          </div>
          <Hint>Paste the path: Dash Wallet cannot browse for files yet.</Hint>
          <Problem text={model.error} testId="settings-import-error" />
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
  const [model] = useState(() => new ImportKeysModel(backend));
  useModel(model);
  const [kind, setKind] = useState<KeyKind>("hdSeed");
  const [text, setText] = useState("");
  const [name, setName] = useState("");
  const id = useId();
  return (
    <Sheet title="Import keys" width="medium" testId="settings-import-keys-sheet" focusKey={String(!!model.done)} closable={!model.working} onClose={onClose}>
      {model.done ? (
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
  const [model] = useState(() => new ExportModel(backend, () => backend.now(), wallet.walletId, wallet.name));
  useModel(model);
  useLockTimer(model.lock);
  useEffect(() => void model.load(), [model]);
  const [format, setFormat] = useState<CoreExportFormatName>("dumpWallet");
  return (
    <Sheet title={`Export ${wallet.name} for Dash Core`} width="medium" testId="settings-export-sheet" focusKey={model.step} closable={model.step !== "working"} onClose={onClose}>
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
              busy={model.lock.working || model.step === "working"}
              disabled={model.lock.blocked}
              submitLabel="Export"
              onSubmit={(p) => model.export(format, p)}
            />
          ) : (
            <>
              <Problem text={model.error} testId="settings-export-error" />
              <DashButton variant="filledBlue" size="large" fill data-autofocus data-testid="settings-export-confirm" onClick={() => void model.export(format, "")}>
                Export
              </DashButton>
            </>
          )}
        </>
      )}
      {model.step === "done" && model.done && (
        <>
          <Done text={FilesText.exported(model.done.keyCount, model.done.path)} testId="settings-export-done" />
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
