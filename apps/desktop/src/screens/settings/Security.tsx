// Settings ▸ Security (UX-SPEC §4.13, QT-111, QT-113, IOS-011…016, IOS-108, IOS-109): the
// vault's encryption and passphrase, Show Recovery Phrase and Forgot passphrase (U-02's), quick
// unlock (not yet, D11), auto lock (D12), payment authentication (D13), autohide (D14) and the
// wipe (D16).

import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useState, type ChangeEvent, type FormEvent } from "react";
import { useBackend, useBackupDirectory, useLockStatus } from "../../api/queries";
import { navigate, ui, useUi } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { NewPassphraseForm, PassphrasePrompt } from "../../components/passphrase";
import { Sheet } from "../../components/sheet";
import { showToast } from "../../components/toast";
import { useModel } from "../../onboarding/model";
import { PassphraseText } from "../../onboarding/passphrase";
import { AUTO_LOCK_CHOICES, ChangePassphraseModel, EncryptModel, PaymentAuthModel, SecurityText, WipeModel } from "../../settings/security";
import { useLockTimer } from "../LockScreen";
import { useSavePrefs } from "./General";
import { Done, Hint, NotYetRow, Problem, SettingsCard } from "./parts";

type SecuritySheet = "encrypt" | "change" | "wipe" | null;

export function SecurityTab() {
  const lock = useLockStatus();
  const prefs = useUi((s) => s.prefs);
  const { save, problem } = useSavePrefs();
  const [sheet, setSheet] = useState<SecuritySheet>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const id = useId();
  const status = lock.data;
  const encrypted = status?.encrypted === true;
  const canForget = encrypted && (status?.walletsWithSecrets.length ?? 0) > 0;
  return (
    <>
      <SettingsCard title="Wallet encryption" testId="settings-encryption">
        <p className="settings-row" data-testid="settings-encryption-state">
          <span className="settings-row-title">Status</span>
          <span>{encrypted ? SecurityText.encrypted : SecurityText.notEncrypted}</span>
        </p>
        <Done text={notice} testId="settings-security-notice" />
        {status && !encrypted && status.state !== "noVault" && (
          <>
            <Hint>{SecurityText.encryptHint}</Hint>
            <DashButton variant="plainBlue" data-testid="settings-encrypt" onClick={() => setSheet("encrypt")}>
              Encrypt wallet…
            </DashButton>
          </>
        )}
        {encrypted && (
          <DashButton variant="plainBlue" data-testid="settings-change-passphrase" onClick={() => setSheet("change")}>
            Change passphrase…
          </DashButton>
        )}
        <DashButton variant="plainBlue" data-testid="settings-security-recovery-phrase" onClick={() => ui.set({ dialog: "recoveryPhrase", recoveryWallet: null })}>
          Show recovery phrase…
        </DashButton>
        {canForget && (
          <DashButton variant="plainBlue" data-testid="settings-forgot" onClick={() => ui.set({ forgotPassphrase: true })}>
            Forgot passphrase?
          </DashButton>
        )}
      </SettingsCard>
      <SettingsCard title="Unlocking" testId="settings-unlocking">
        <NotYetRow
          testId="settings-quick-unlock"
          title="Touch ID or Windows Hello, and its spending limit"
          label={SecurityText.quickUnlockUnavailable}
          why={SecurityText.quickUnlockWhy}
        />
        <div className="settings-row">
          <label className="settings-row-title" htmlFor={`${id}-auto-lock`}>
            Lock automatically after
          </label>
          <select
            id={`${id}-auto-lock`}
            className="settings-select"
            data-testid="settings-auto-lock"
            value={String(prefs.autoLockSecs)}
            disabled={!encrypted}
            onChange={(e) => void save({ autoLockSecs: e.target.value === "null" ? null : Number(e.target.value) })}
          >
            {/* settings.json can hold a time that is not a choice: shown as it is, not as "Never". */}
            {!AUTO_LOCK_CHOICES.some((c) => c.secs === prefs.autoLockSecs) && (
              <option value={String(prefs.autoLockSecs)}>{SecurityText.autoLockSeconds(prefs.autoLockSecs!)}</option>
            )}
            {AUTO_LOCK_CHOICES.map((c) => (
              <option key={c.label} value={String(c.secs)}>
                {c.label}
              </option>
            ))}
          </select>
        </div>
        <Hint>{encrypted ? "Counts from the last key press, click or scroll in this window." : SecurityText.encryptFirst}</Hint>
        <Problem text={problem} testId="settings-security-not-saved" />
      </SettingsCard>
      <PaymentAuthCard encrypted={encrypted} known={!!status} />
      <SettingsCard title="Privacy" testId="settings-privacy">
        <label className="checkbox-row">
          <input
            type="checkbox"
            data-testid="settings-autohide"
            checked={prefs.autohideBalance}
            onChange={(e) => void save({ autohideBalance: e.target.checked })}
          />
          {SecurityText.autohideBalance}
        </label>
      </SettingsCard>
      <SettingsCard title="Delete wallet data" testId="settings-wipe-card">
        <Hint>Removes every wallet of this network from this computer.</Hint>
        <DashButton variant="plainRed" data-testid="settings-wipe" onClick={() => setSheet("wipe")}>
          Wipe wallet…
        </DashButton>
      </SettingsCard>
      {sheet === "encrypt" && (
        <EncryptSheet
          onClose={(done) => {
            setSheet(null);
            if (done) setNotice(`${SecurityText.walletEncrypted} ${SecurityText.lockedAfterEncrypt}`);
          }}
        />
      )}
      {sheet === "change" && (
        <ChangePassphraseSheet
          onClose={(done) => {
            setSheet(null);
            if (done) setNotice(SecurityText.passphraseChanged);
          }}
        />
      )}
      {sheet === "wipe" && <WipeSheet onClose={() => setSheet(null)} />}
    </>
  );
}

function PaymentAuthCard({ encrypted, known }: { encrypted: boolean; known: boolean }) {
  const backend = useBackend();
  const [model] = useState(() => new PaymentAuthModel(backend));
  useModel(model);
  useLockTimer(model.lock);
  useEffect(() => void model.load(), [model]);
  return (
    <SettingsCard title="Payments" testId="settings-payments">
      <label className="checkbox-row">
        <input
          type="checkbox"
          data-testid="settings-payment-auth"
          checked={encrypted && model.on === true}
          disabled={!known || !encrypted || model.on === null}
          onChange={(e) => model.ask(e.target.checked)}
        />
        {SecurityText.requireAuthentication}
      </label>
      <Hint>{encrypted ? SecurityText.requireAuthenticationHint : SecurityText.encryptFirst}</Hint>
      {model.pending === null && <Problem text={model.error} testId="settings-payment-auth-error" />}
      {model.pending !== null && (
        <Sheet title={SecurityText.requireAuthentication} width="small" testId="settings-payment-auth-sheet" onClose={() => model.cancel()}>
          <p className="sheet-text">
            {model.pending ? "Turn on: every payment asks for the passphrase." : "Turn off: payments go through without the passphrase while the wallet is unlocked."}
          </p>
          <PassphrasePrompt
            testId="settings-payment-auth-passphrase"
            error={model.lock.message ?? model.error}
            busy={model.lock.working}
            disabled={model.lock.blocked}
            submitLabel={model.pending ? "Turn on" : "Turn off"}
            onSubmit={(p) => model.confirm(p)}
          />
        </Sheet>
      )}
    </SettingsCard>
  );
}

function EncryptSheet({ onClose }: { onClose: (done: boolean) => void }) {
  const backend = useBackend();
  const [model] = useState(() => new EncryptModel(backend));
  useModel(model);
  // The engine locks the vault once it is encrypted: the lock screen replaces this sheet.
  useEffect(() => {
    if (model.step === "done") onClose(true);
  }, [model.step, onClose]);
  return (
    <Sheet title="Encrypt wallet" width="medium" testId="settings-encrypt-sheet" focusKey={model.step} closable={model.step !== "working"} onClose={() => onClose(false)}>
      <p className="sheet-warning" role="note">
        {PassphraseText.warning}
      </p>
      {model.step === "working" ? (
        <p className="sheet-text" role="status">
          Encrypting…
        </p>
      ) : (
        <NewPassphraseForm
          testId="settings-encrypt"
          error={model.error}
          submitLabel="Encrypt wallet"
          onEdit={() => model.edited()}
          onSubmit={(text, confirmation) => model.encrypt(text, confirmation)}
        />
      )}
    </Sheet>
  );
}

function ChangePassphraseSheet({ onClose }: { onClose: (done: boolean) => void }) {
  const backend = useBackend();
  const [model] = useState(() => new ChangePassphraseModel(backend));
  useModel(model);
  useLockTimer(model.lock);
  useEffect(() => void model.load(), [model]);
  useEffect(() => {
    if (model.done) onClose(true);
  }, [model.done, onClose]);
  const id = useId();
  const [current, setCurrent] = useState("");
  const [text, setText] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const submit = (e: FormEvent) => {
    e.preventDefault();
    const [c, t, r] = [current, text, confirmation];
    setCurrent("");
    setText("");
    setConfirmation("");
    void model.change(c, t, r);
  };
  const edit = (set: (v: string) => void) => (e: ChangeEvent<HTMLInputElement>) => {
    set(e.target.value);
    model.edited();
  };
  const error = model.message;
  return (
    <Sheet title="Change passphrase" width="medium" testId="settings-change-sheet" closable={!model.lock.working} onClose={() => onClose(false)}>
      <form className="secret-form" onSubmit={submit} data-testid="settings-change-form">
        <div className="form-field">
          <label htmlFor={`${id}-current`}>Current passphrase</label>
          <input
            id={`${id}-current`}
            type="password"
            autoComplete="current-password"
            data-testid="settings-change-current"
            data-autofocus
            value={current}
            disabled={model.lock.blocked}
            onChange={edit(setCurrent)}
          />
        </div>
        <div className="form-field">
          <label htmlFor={`${id}-new`}>New passphrase</label>
          <input id={`${id}-new`} type="password" autoComplete="new-password" data-testid="settings-change-new" value={text} onChange={edit(setText)} />
        </div>
        <div className="form-field">
          <label htmlFor={`${id}-repeat`}>Repeat the new passphrase</label>
          <input
            id={`${id}-repeat`}
            type="password"
            autoComplete="new-password"
            data-testid="settings-change-repeat"
            value={confirmation}
            onChange={edit(setConfirmation)}
          />
        </div>
        <Problem text={error} testId="settings-change-error" />
        <DashButton type="submit" variant="filledBlue" size="large" fill data-testid="settings-change-submit" disabled={model.lock.working || model.lock.blocked}>
          Change passphrase
        </DashButton>
      </form>
    </Sheet>
  );
}

function WipeSheet({ onClose }: { onClose: () => void }) {
  const backend = useBackend();
  const client = useQueryClient();
  const [model] = useState(() => new WipeModel(backend));
  useModel(model);
  useLockTimer(model.lock);
  useEffect(() => void model.load(), [model]);
  // Nothing is left for the sheet to stand on: onboarding opens over a fresh shell, and the toast
  // says what happened.
  useEffect(() => {
    if (model.step !== "done") return;
    showToast(SecurityText.wiped);
    navigate("home");
    void client.invalidateQueries();
    onClose();
  }, [model.step, client, onClose]);
  const [sentence, setSentence] = useState("");
  const id = useId();
  const working = model.step === "working" || model.lock.working;
  const backupDir = useBackupDirectory();
  return (
    <Sheet title={SecurityText.wipeTitle} width="medium" testId="settings-wipe-sheet" focusKey={model.step} closable={!working} onClose={onClose}>
      {model.step === "sentence" && (
        <form
          className="secret-form"
          onSubmit={(e) => {
            e.preventDefault();
            model.accept(sentence);
          }}
        >
          <p className="sheet-warning" role="note">
            {SecurityText.wipeMessage}
          </p>
          <p className="field-hint" data-testid="settings-wipe-keeps">
            {SecurityText.wipeKeeps(backupDir.data ?? "the backup folder")}
          </p>
          <div className="form-field">
            <label htmlFor={`${id}-sentence`}>Type: {SecurityText.wipeAcceptPhrase}</label>
            <input
              id={`${id}-sentence`}
              data-testid="settings-wipe-sentence"
              data-autofocus
              autoComplete="off"
              value={sentence}
              onChange={(e) => setSentence(e.target.value)}
            />
          </div>
          <Problem text={model.error} testId="settings-wipe-error" />
          <DashButton type="submit" variant="filledBlue" size="large" fill data-testid="settings-wipe-continue">
            Continue
          </DashButton>
        </form>
      )}
      {model.step === "passphrase" &&
        (model.encrypted ? (
          <PassphrasePrompt
            testId="settings-wipe-passphrase"
            error={model.lock.message ?? model.error}
            busy={model.lock.working}
            disabled={model.lock.blocked}
            submitLabel="Delete everything"
            onSubmit={(p) => model.wipe(p)}
          />
        ) : (
          <DashButton variant="filledBlue" size="large" fill data-autofocus data-testid="settings-wipe-confirm" onClick={() => void model.wipe("")}>
            Delete everything
          </DashButton>
        ))}
      {model.step === "working" && (
        <p className="sheet-text" role="status">
          Deleting…
        </p>
      )}
      {model.step === "stopped" && <Problem text={model.error} testId="settings-wipe-error" />}
    </Sheet>
  );
}
