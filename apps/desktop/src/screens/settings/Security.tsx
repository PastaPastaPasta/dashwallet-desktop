// Settings ▸ Security (UX-SPEC §4.13, QT-111, QT-113, IOS-011…016, IOS-108, IOS-109): the
// vault's encryption and passphrase, Show Recovery Phrase and Forgot passphrase (U-02's), quick
// unlock (not yet, D11), auto lock (D12), payment authentication (D13), autohide (D14) and the
// wipe (D16).

import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useState, type ChangeEvent, type FormEvent } from "react";
import type { LockStatus } from "../../api/generated/LockStatus";
import { keys, useBackend, useBackupDirectory, useLockStatus } from "../../api/queries";
import { navigate, ui } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { NewPassphraseForm, PassphrasePrompt } from "../../components/passphrase";
import { Sheet } from "../../components/sheet";
import { showToast } from "../../components/toast";
import { useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import { PassphraseText } from "../../onboarding/passphrase";
import { AUTO_LOCK_CHOICES, ChangePassphraseModel, DEFAULT_SPEND_LIMIT, EncryptModel, PaymentAuthModel, SPEND_LIMITS, SecurityText, SpendLimitModel, WipeModel, spendLimitLabel } from "../../settings/security";
import { useLockTimer } from "../LockScreen";
import { useSavePrefs } from "./General";
import { Done, Hint, NotYetRow, Problem, SettingsCard } from "./parts";

type SecuritySheet = "encrypt" | "change" | "wipe" | null;

/** The wipe sheet's flow when the vault locked with it open: Settings unmounts with the other pages
 * when the lock screen replaces them, and the sheet opens again with it once unlocked, the shell
 * held meanwhile (D33, D38). */
let wipeAcrossLock: WipeModel | null = null;

export function SecurityTab() {
  const t = useT();
  const lock = useLockStatus();
  const { prefs, save, problem } = useSavePrefs();
  const [sheet, setSheet] = useState<SecuritySheet>(() => (wipeAcrossLock ? "wipe" : null));
  const [notice, setNotice] = useState<string | null>(null);
  const id = useId();
  const status = lock.data;
  const encrypted = status?.encrypted === true;
  const canForget = encrypted && (status?.walletsWithSecrets.length ?? 0) > 0;
  return (
    <>
      <SettingsCard title={t("settings.security.encryption")} testId="settings-encryption">
        <p className="settings-row" data-testid="settings-encryption-state">
          <span className="settings-row-title">{t("settings.security.status")}</span>
          <span>{encrypted ? SecurityText.encrypted : SecurityText.notEncrypted}</span>
        </p>
        <Done text={notice} testId="settings-security-notice" />
        {status && !encrypted && status.state !== "noVault" && (
          <>
            <Hint>{SecurityText.encryptHint}</Hint>
            <DashButton variant="plainBlue" data-testid="settings-encrypt" onClick={() => setSheet("encrypt")}>
              {t("settings.security.encrypt")}
            </DashButton>
          </>
        )}
        {encrypted && (
          <DashButton variant="plainBlue" data-testid="settings-change-passphrase" onClick={() => setSheet("change")}>
            {t("settings.security.change")}
          </DashButton>
        )}
        <DashButton variant="plainBlue" data-testid="settings-security-recovery-phrase" onClick={() => ui.set({ dialog: "recoveryPhrase", recoveryWallet: null })}>
          {t("settings.security.showPhrase")}
        </DashButton>
        {canForget && (
          <DashButton variant="plainBlue" data-testid="settings-forgot" onClick={() => ui.set({ forgotPassphrase: true })}>
            {t("settings.security.forgot")}
          </DashButton>
        )}
      </SettingsCard>
      <SettingsCard title={t("settings.security.unlocking")} testId="settings-unlocking">
        <NotYetRow
          testId="settings-quick-unlock"
          title={t("settings.security.quickUnlock")}
          label={SecurityText.quickUnlockUnavailable}
          why={SecurityText.quickUnlockWhy}
        />
        <div className="settings-row">
          <label className="settings-row-title" htmlFor={`${id}-auto-lock`}>
            {t("settings.security.autoLock")}
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
        <Hint>{encrypted ? t("settings.security.autoLockHint") : SecurityText.encryptFirst}</Hint>
        <Problem text={problem} testId="settings-security-not-saved" />
      </SettingsCard>
      <PaymentAuthCard encrypted={encrypted} known={!!status} />
      <SettingsCard title={t("settings.security.privacy")} testId="settings-privacy">
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
      <SettingsCard title={t("settings.security.wipeCard")} testId="settings-wipe-card">
        <Hint>{t("settings.security.wipeHint")}</Hint>
        <DashButton variant="plainRed" data-testid="settings-wipe" onClick={() => setSheet("wipe")}>
          {t("settings.security.wipe")}
        </DashButton>
      </SettingsCard>
      {sheet === "encrypt" && (
        <EncryptSheet
          onClose={(done) => {
            setSheet(null);
            if (done) setNotice(SecurityText.encryptedLocked);
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
  const t = useT();
  const backend = useBackend();
  const id = useId();
  const [model] = useState(() => new PaymentAuthModel(backend));
  const [limit] = useState(() => new SpendLimitModel(backend));
  useModel(model);
  useModel(limit);
  useLockTimer(model.lock);
  useLockTimer(limit.lock);
  useEffect(() => {
    void model.load();
    void limit.load();
    return () => {
      model.dispose();
      limit.dispose();
    };
  }, [model, limit]);
  const askEveryTime = encrypted && model.on === true;
  return (
    <SettingsCard title={t("settings.security.payments")} testId="settings-payments">
      <label className="checkbox-row">
        <input
          type="checkbox"
          data-testid="settings-payment-auth"
          checked={askEveryTime}
          disabled={!known || !encrypted || model.on === null}
          onChange={(e) => model.ask(e.target.checked)}
        />
        {SecurityText.requireAuthentication}
      </label>
      <Hint>{encrypted ? (askEveryTime ? SecurityText.requireAuthenticationHint : SecurityText.signingWindowHint) : SecurityText.encryptFirst}</Hint>
      <div className="settings-row">
        <label className="settings-row-title" htmlFor={`${id}-spend-limit`}>
          {SecurityText.spendLimit}
        </label>
        <select
          id={`${id}-spend-limit`}
          className="settings-select"
          data-testid="settings-spend-limit"
          value={limit.limit === null || limit.unreadable ? "" : String(limit.limit)}
          disabled={!known || !encrypted || limit.limit === null || limit.busy}
          onChange={(e) => limit.ask(Number(e.target.value))}
        >
          {/* An unreadable limit shows none as current: the one in force is 0 until it is set again. */}
          {limit.unreadable && <option value="" disabled />}
          {limit.limit !== null && !limit.unreadable && !SPEND_LIMITS.some((c) => c.duffs === limit.limit) && <option value={String(limit.limit)}>{limit.limit}</option>}
          {SPEND_LIMITS.map((c) => (
            <option key={c.duffs} value={String(c.duffs)}>
              {c.label}
            </option>
          ))}
        </select>
      </div>
      {limit.unreadable ? (
        <div className="settings-row" role="status" data-testid="settings-spend-limit-unreadable">
          <span className="form-problem">{t("settings.security.spendLimitUnreadable")}</span>
          <DashButton variant="tintedBlue" size="small" data-testid="settings-spend-limit-set-again" disabled={!known || !encrypted || limit.busy} onClick={() => limit.ask(DEFAULT_SPEND_LIMIT)}>
            {t("settings.security.spendLimitSetAgain")}
          </DashButton>
        </div>
      ) : (
        <Hint>{SecurityText.spendLimitHint}</Hint>
      )}
      {model.pending === null && limit.pending === null && <Problem text={model.error ?? limit.error} testId="settings-payment-auth-error" />}
      {model.pending !== null && (
        <Sheet
          title={SecurityText.requireAuthentication}
          width="small"
          testId="settings-payment-auth-sheet"
          closable={!model.busy}
          onClose={() => model.cancel()}
        >
          <p className="sheet-text">
            {model.pending ? t("settings.security.turnOnText") : t("settings.security.turnOffText")}
          </p>
          <PassphrasePrompt
            testId="settings-payment-auth-passphrase"
            error={model.lock.message ?? model.error}
            busy={model.busy}
            disabled={model.lock.blocked}
            submitLabel={model.pending ? t("settings.security.turnOn") : t("settings.security.turnOff")}
            onSubmit={(p) => model.confirm(p)}
          />
        </Sheet>
      )}
      {limit.pending !== null && (
        <Sheet title={SecurityText.spendLimit} width="small" testId="settings-spend-limit-sheet" closable={!limit.busy} onClose={() => limit.cancel()}>
          <p className="sheet-text" data-testid="settings-spend-limit-pending">
            {t("settings.security.spendLimitChange", { from: spendLimitLabel(limit.limit ?? 0), to: spendLimitLabel(limit.pending) })}
          </p>
          <PassphrasePrompt
            testId="settings-spend-limit-passphrase"
            error={limit.lock.message ?? limit.error}
            busy={limit.busy}
            disabled={limit.lock.blocked}
            submitLabel={t("common.save")}
            onSubmit={(p) => limit.confirm(p)}
          />
        </Sheet>
      )}
    </SettingsCard>
  );
}

function EncryptSheet({ onClose }: { onClose: (done: boolean) => void }) {
  const t = useT();
  const backend = useBackend();
  const [model] = useState(() => new EncryptModel(backend));
  useModel(model);
  // The engine locks the vault once it is encrypted: the lock screen replaces this sheet.
  useEffect(() => {
    if (model.step === "done") onClose(true);
  }, [model.step, onClose]);
  return (
    <Sheet title={t("settings.security.encryptTitle")} width="medium" testId="settings-encrypt-sheet" focusKey={model.step} closable={model.step !== "working"} onClose={() => onClose(false)}>
      <p className="sheet-warning" role="note">
        {PassphraseText.warning}
      </p>
      {model.step === "working" ? (
        <p className="sheet-text" role="status">
          {t("settings.security.encrypting")}
        </p>
      ) : (
        <NewPassphraseForm
          testId="settings-encrypt"
          error={model.error}
          submitLabel={t("settings.security.encryptSubmit")}
          onEdit={() => model.edited()}
          onSubmit={(text, confirmation) => model.encrypt(text, confirmation)}
        />
      )}
    </Sheet>
  );
}

function ChangePassphraseSheet({ onClose }: { onClose: (done: boolean) => void }) {
  const t = useT();
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
    <Sheet title={t("settings.security.changeTitle")} width="medium" testId="settings-change-sheet" closable={!model.lock.working} onClose={() => onClose(false)}>
      <form className="secret-form" onSubmit={submit} data-testid="settings-change-form">
        <div className="form-field">
          <label htmlFor={`${id}-current`}>{t("settings.security.current")}</label>
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
          <label htmlFor={`${id}-new`}>{t("settings.security.new")}</label>
          <input id={`${id}-new`} type="password" autoComplete="new-password" data-testid="settings-change-new" value={text} onChange={edit(setText)} />
        </div>
        <div className="form-field">
          <label htmlFor={`${id}-repeat`}>{t("settings.security.repeat")}</label>
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
          {t("settings.security.changeSubmit")}
        </DashButton>
      </form>
    </Sheet>
  );
}

function WipeSheet({ onClose }: { onClose: () => void }) {
  const t = useT();
  const backend = useBackend();
  const client = useQueryClient();
  const [model] = useState(() => wipeAcrossLock ?? new WipeModel(backend));
  useModel(model);
  useLockTimer(model.lock);
  useEffect(() => void model.load(), [model]);
  // While the sheet is open onboarding stays closed: the last removal empties the wallet list
  // before the vault is deleted, and a failure there must show here (D33). A lock does not close it.
  useEffect(() => {
    ui.set({ wiping: true });
    if (wipeAcrossLock === model) wipeAcrossLock = null;
    return () => {
      if (client.getQueryData<LockStatus>(keys.lockStatus)?.state === "locked") {
        wipeAcrossLock = model;
        return;
      }
      model.cancel();
      ui.set({ wiping: false });
    };
  }, [model, client]);
  // Nothing is left for the sheet to stand on: onboarding opens over a fresh shell, and the toast
  // says what happened.
  useEffect(() => {
    if (model.step !== "done") return;
    ui.set({ wiping: false });
    showToast(SecurityText.wiped);
    navigate("home");
    void client.invalidateQueries();
    onClose();
  }, [model.step, client, onClose]);
  const [sentence, setSentence] = useState("");
  const id = useId();
  const backupDir = useBackupDirectory();
  return (
    <Sheet title={SecurityText.wipeTitle} width="medium" testId="settings-wipe-sheet" focusKey={model.step} closable={!model.busy} onClose={onClose}>
      {model.step === "sentence" && (
        <form
          className="secret-form"
          onSubmit={(e) => {
            e.preventDefault();
            model.accept(sentence);
          }}
        >
          <p className="sheet-warning settings-wipe-warning" role="note">
            {SecurityText.wipeMessage}
          </p>
          <p className="field-hint" data-testid="settings-wipe-keeps">
            {SecurityText.wipeKeeps(backupDir.data ?? t("settings.security.backupFolder"))}
          </p>
          <div className="form-field">
            <label className="settings-wipe-label" htmlFor={`${id}-sentence`}>{t.rich("settings.security.wipeType", { sentence: <bdi data-testid="settings-wipe-phrase">{SecurityText.wipeAcceptPhrase}</bdi> })}</label>
            <input
              id={`${id}-sentence`}
              data-testid="settings-wipe-sentence"
              dir="auto"
              data-autofocus
              autoComplete="off"
              value={sentence}
              onChange={(e) => setSentence(e.target.value)}
            />
          </div>
          <Problem text={model.error} testId="settings-wipe-error" />
          <DashButton type="submit" variant="filledBlue" size="large" fill data-testid="settings-wipe-continue">
            {t("settings.security.continue")}
          </DashButton>
        </form>
      )}
      {model.step === "passphrase" &&
        (model.encrypted ? (
          <PassphrasePrompt
            testId="settings-wipe-passphrase"
            error={model.lock.message ?? model.error}
            busy={model.busy}
            disabled={model.lock.blocked}
            submitLabel={t("settings.security.deleteEverything")}
            onSubmit={(p) => model.wipe(p)}
          />
        ) : (
          // No autofocus: Enter on opening this step must not delete everything (D35).
          <DashButton variant="filledBlue" size="large" fill data-testid="settings-wipe-confirm" disabled={model.busy} onClick={() => void model.wipe("")}>
            {t("settings.security.deleteEverything")}
          </DashButton>
        ))}
      {model.step === "working" && (
        <p className="sheet-text" role="status">
          {t("settings.security.deleting")}
        </p>
      )}
      {model.step === "stopped" && (
        <>
          <Problem text={model.error} testId="settings-wipe-error" />
          {/* Enter on a stop tries again; closing is a choice the text explains (D39). */}
          <DashButton variant="tintedBlue" size="large" fill data-testid="settings-wipe-retry" data-autofocus onClick={() => model.retry()}>
            {t("settings.security.tryAgain")}
          </DashButton>
        </>
      )}
    </Sheet>
  );
}
