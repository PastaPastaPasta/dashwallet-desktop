// Onboarding (IOS-002, IOS-004, IOS-007, IOS-010; U-02 D1–D7, D17): a full-window surface over
// the shell for the first wallet and for Wallet ▸ Add Wallet…. The steps are `OnboardingModel`'s;
// this renders them. The restore text is the only secret held here, in the textarea's state,
// until the flow ends.

import { useEffect, useId, useRef, useState } from "react";
import { errorMessage } from "../api/backend";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { useBackend, useRestored, useWallets } from "../api/queries";
import { walletReadyToOpen } from "../components/emptyStates";
import { useSecretScreen } from "../app/ui";
import { DashButton, DashLogo, NetworkCapsule } from "../components/basics";
import { NetworkPicker, useNetworkPicker } from "../components/networkPicker";
import { OnboardingRatesChoice, useRatesConsent } from "../fiat/consent";
import { NewPassphraseForm, PassphrasePrompt } from "../components/passphrase";
import { Bip39Field, noCopy, PhraseGrid, PhraseInput, ProtectionPending } from "../components/phrase";
import { useModel } from "../onboarding/model";
import { OnboardingModel, type OnboardingStep } from "../onboarding/onboarding";
import { useT } from "../i18n";
import { PassphraseText } from "../onboarding/passphrase";
import { RecoveryText } from "../onboarding/recovery";
import { chooseFilePath, fileErrorText } from "../app/files";
import type { WalletFileInfo } from "../api/generated/WalletFileInfo";
import { useLockTimer } from "./LockScreen";

const BACK_STEPS: ReadonlySet<OnboardingStep> = new Set([
  "choosePassphrase",
  "unlockVault",
  "showPhrase",
  "verifyPhrase",
  "restorePhrase",
  "restoreOptions",
  "restoreFile",
]);

export function Onboarding({
  network,
  inert = false,
  onCancel,
  onDone,
}: {
  network: NetworkInfo;
  /** Another screen (the first-run notice) is over it: no focus, hidden from assistive tech. */
  inert?: boolean;
  /** Closes Add Wallet; `null` for the first wallet, which cannot be skipped. */
  onCancel: (() => void) | null;
  onDone: (walletId: string | null) => void | Promise<void>;
}) {
  const t = useT();
  const backend = useBackend();
  const [model] = useState(() => new OnboardingModel(backend, network.kind, { files: { api: backend, network: network.dirName } }));
  useModel(model);
  useLockTimer(model.lock);
  const { step } = model;
  const [restoreText, setRestoreText] = useState("");
  useEffect(() => () => model.dispose(), [model]);
  // The restore text goes with the flow: once the wallet is added, or back at Welcome.
  useEffect(() => {
    if (step === "done" || step === "welcome") setRestoreText("");
  }, [step]);
  // The whole flow, from Welcome: no step shows until the window is kept out of screen
  // captures (D15, D27).
  const protection = useSecretScreen();
  const ready = protection === "on";

  // Each step starts on its first field, else its title, so screen readers hear the step.
  const body = useRef<HTMLElement>(null);
  useEffect(() => {
    const target = body.current?.querySelector<HTMLElement>("[data-autofocus]") ?? body.current?.querySelector<HTMLElement>("h1");
    target?.focus();
  }, [step, ready, model.file?.step]);

  return (
    <div className="onboarding" data-testid="onboarding" inert={inert}>
      <div className="onboarding-panel">
        <header className="onboarding-header">
          <span className="onboarding-header-side">
            {ready && BACK_STEPS.has(step) && (
              <DashButton variant="plainBlue" size="small" data-testid="onboarding-back" disabled={model.busy} onClick={() => model.back()}>
                {t("onboarding.back")}
              </DashButton>
            )}
          </span>
          <span className="onboarding-brand">
            <DashLogo className="onboarding-logo" />
            <NetworkCapsule network={network} />
          </span>
          <span className="onboarding-header-side" />
        </header>
        <main className="onboarding-body" ref={body} data-testid={ready ? `onboarding-${step}` : "onboarding-preparing"}>
          {ready ? (
            <Step model={model} network={network} onCancel={onCancel} onDone={onDone} restoreText={restoreText} setRestoreText={setRestoreText} />
          ) : (
            <ProtectionPending protection={protection} onCancel={onCancel} />
          )}
        </main>
      </div>
    </div>
  );
}

function Title({ children }: { children: string }) {
  return (
    <h1 className="onboarding-title" tabIndex={-1}>
      {children}
    </h1>
  );
}

function Step({
  model,
  network,
  onCancel,
  onDone,
  restoreText,
  setRestoreText,
}: {
  model: OnboardingModel;
  network: NetworkInfo;
  onCancel: (() => void) | null;
  onDone: (walletId: string | null) => void | Promise<void>;
  restoreText: string;
  setRestoreText: (text: string) => void;
}) {
  const t = useT();
  switch (model.step) {
    case "welcome":
      return <Welcome model={model} onCancel={onCancel} />;
    case "choosePassphrase":
      return <ChoosePassphrase model={model} />;
    case "unlockVault":
      return (
        <>
          <Title>{t("onboarding.unlock.title")}</Title>
          <p className="onboarding-text">{t("onboarding.unlock.text")}</p>
          <PassphrasePrompt
            testId="onboarding-unlock"
            error={model.lock.message}
            busy={model.busy}
            disabled={model.lock.blocked}
            submitLabel={t("onboarding.unlock.submit")}
            onSubmit={(p) => model.unlockVault(p)}
          />
        </>
      );
    case "showPhrase":
      return <ShowPhrase model={model} />;
    case "verifyPhrase":
      return <VerifyPhrase model={model} />;
    case "restorePhrase":
      return (
        <>
          <Title>{t("onboarding.restore.title")}</Title>
          <p className="onboarding-text">{t("onboarding.restore.text")}</p>
          <PhraseInput
            testId="restore"
            entry={model.entry}
            text={restoreText}
            onChange={(text) => {
              setRestoreText(text);
              void model.updateRestoreText(text);
            }}
          />
          <DashButton
            variant="filledBlue"
            size="large"
            fill
            disabled={!model.entry.valid}
            data-testid="restore-continue"
            onClick={() => model.continueRestore()}
          >
            {t("onboarding.continue")}
          </DashButton>
        </>
      );
    case "restoreOptions":
      return <RestoreOptions model={model} />;
    case "restoreFile":
      return <RestoreFile model={model} network={network.dirName} />;
    case "working":
      return (
        <>
          <Title>{t("onboarding.working.title")}</Title>
          <p className="onboarding-text" role="status">
            {t("onboarding.working.text")}
          </p>
        </>
      );
    case "done":
      return <Done model={model} network={network} onDone={onDone} />;
    case "failed":
      return (
        <>
          <Title>{t("onboarding.failed.title")}</Title>
          <p className="form-problem" role="alert" data-testid="onboarding-failure">
            {model.failure}
          </p>
          <DashButton variant="filledBlue" size="large" fill data-autofocus data-testid="onboarding-retry" onClick={() => model.back()}>
            {t("onboarding.back")}
          </DashButton>
        </>
      );
  }
}

/** The last step; on Mainnet it carries the price disclosure (MP-01), saved by "Open wallet".
 * A list read that fails after the wallet is saved stays here: it says so and tries the read again,
 * and Open wallet does not stay disabled. */
function Done({ model, network, onDone }: { model: OnboardingModel; network: NetworkInfo; onDone: (walletId: string | null) => void | Promise<void> }) {
  const t = useT();
  const consent = useRatesConsent(network);
  const wallets = useWallets();
  const [ratesOn, setRatesOn] = useState(true);
  const [opening, setOpening] = useState(false);
  const listFailed = !walletReadyToOpen(model.walletId, wallets.data) && wallets.isError;
  const open = () => {
    setOpening(true);
    const finish = () => {
      void Promise.resolve(onDone(model.walletId)).finally(() => setOpening(false));
    };
    if (!consent.needed) return finish();
    void consent.answer(ratesOn).finally(finish);
  };
  return (
    <>
      <Title>{t("onboarding.ready.title")}</Title>
      <p className="onboarding-text">
        {model.flow === "file" && model.restoredCount > 1
          ? t("onboarding.ready.restoredMany", { count: model.restoredCount })
          : model.flow === "create"
            ? t("onboarding.ready.created")
            : t("onboarding.ready.restored")}
      </p>
      {model.restoredSkipped && (
        <p className="onboarding-text" role="status" data-testid="onboarding-file-skipped">
          {model.restoredSkipped.keys > 0 && t("onboarding.file.keysSkipped", { count: model.restoredSkipped.keys })}
          {model.restoredSkipped.keys > 0 && model.restoredSkipped.scripts > 0 && " "}
          {model.restoredSkipped.scripts > 0 && t("onboarding.file.scriptsSkipped", { count: model.restoredSkipped.scripts })}
        </p>
      )}
      {consent.needed && <OnboardingRatesChoice consent={consent} on={ratesOn} setOn={setRatesOn} />}
      {listFailed && (
        <p className="form-problem" role="alert" data-testid="onboarding-list-error">
          {t("onboarding.ready.listFailed")}
          {wallets.error ? ` ${errorMessage(wallets.error)}` : ""}
        </p>
      )}
      <DashButton variant="filledBlue" size="large" fill data-autofocus data-testid={listFailed ? "onboarding-list-retry" : "onboarding-open"} disabled={opening} onClick={open}>
        {listFailed ? t("onboarding.ready.retry") : t("onboarding.ready.open")}
      </DashButton>
    </>
  );
}

function Welcome({ model, onCancel }: { model: OnboardingModel; onCancel: (() => void) | null }) {
  // A wallet is made on the network this window runs on. Choosing the other one moves the window
  // there (the page boots again), so until then neither action may go on. "Add a wallet" adds to
  // this network and has no picker: switching networks is Settings' (row 74).
  // While the network's data is a restore not yet synced (RESTORED; a damaged store set up again in a
  // new folder starts so), a wallet comes only from its recovery phrase: no new wallet, no file.
  const t = useT();
  const restoreOnly = useRestored().data === true;
  const picker = useNetworkPicker();
  const showPicker = onCancel === null && (picker.available || picker.loading);
  const wait = showPicker && (picker.loading || picker.switching !== null);
  return (
    <>
      <Title>{onCancel ? t("onboarding.welcome.addTitle") : t("onboarding.welcome.title")}</Title>
      <p className="onboarding-text" data-testid="onboarding-welcome-text">
        {restoreOnly ? t(onCancel ? "onboarding.welcome.restoreOnlyAdd" : "onboarding.welcome.restoreOnly") : t("onboarding.welcome.text")}
      </p>
      {/* Without the command (a build before it) the first screen shows no picker. */}
      {showPicker && <NetworkPicker state={picker} testId="onboarding-network" />}
      <div className="onboarding-actions">
        {!restoreOnly && (
          <DashButton variant="filledBlue" size="large" fill disabled={model.busy || wait} data-testid="onboarding-create" onClick={() => void model.startCreate()}>
            {t("onboarding.welcome.create")}
          </DashButton>
        )}
        <DashButton
          variant={restoreOnly ? "filledBlue" : "plainBlue"}
          size="large"
          fill
          disabled={model.busy || wait}
          data-testid="onboarding-restore"
          onClick={() => void model.startRestore()}
        >
          {t("onboarding.welcome.restore")}
        </DashButton>
        {model.file && !restoreOnly && (
          <DashButton variant="plainBlue" size="large" fill disabled={model.busy || wait} data-testid="onboarding-restore-file" onClick={() => void model.startFileRestore()}>
            {t("onboarding.welcome.restoreFile")}
          </DashButton>
        )}
        {onCancel && (
          <DashButton variant="plainBlue" size="large" fill data-testid="onboarding-cancel" onClick={onCancel}>
            {t("common.cancel")}
          </DashButton>
        )}
      </div>
      {!restoreOnly && (
        <details className="onboarding-advanced">
          <summary>{t("onboarding.advanced")}</summary>
          <label className="check-row">
            <input
              type="checkbox"
              data-testid="onboarding-24-words"
              checked={model.wordCount === 24}
              onChange={(e) => model.setWordCount(e.target.checked ? 24 : 12)}
            />
            {t("onboarding.welcome.words24")}
          </label>
        </details>
      )}
    </>
  );
}

/** What the file is, in the catalog's words (the Settings sheet's plan, U-04 D21). */
function fileKindText(t: ReturnType<typeof useT>, info: WalletFileInfo): string {
  switch (info.kind) {
    case "dwBackup":
      return t("onboarding.file.kind.dwBackup", { count: info.walletCount });
    case "dumpWallet":
      return t("onboarding.file.kind.dumpWallet");
    case "walletDatSqlite":
      return t(info.encrypted ? "onboarding.file.kind.walletDatEncrypted" : "onboarding.file.kind.walletDat");
    case "walletDatBdb":
      return t(info.encrypted === true ? "onboarding.file.kind.walletDatEncrypted" : "onboarding.file.kind.walletDat");
    case "psbt":
      return t("onboarding.file.kind.psbt");
    case "unknown":
      return t("onboarding.file.kind.unknown");
  }
}

/** Why the file stops here, in the catalog's words (the plan's own sentence is Settings' English). */
function refusalText(t: ReturnType<typeof useT>, info: WalletFileInfo, network: string): string {
  if (info.kind === "psbt") return t("onboarding.file.psbtRefusal");
  if (info.kind === "unknown") return t("onboarding.file.unknownRefusal");
  if (info.kind === "dwBackup" && info.network === null) return t("onboarding.file.unknownNetwork");
  const theirs = info.kind === "dwBackup" || info.kind === "dumpWallet" ? info.network?.dirName : undefined;
  if (theirs && theirs !== network) return t("onboarding.file.otherNetwork", { network: theirs });
  return t("onboarding.file.emptyBackup");
}

/** Restore from a file: a path (typed or chosen), then the file's passphrase or a name when it
 * needs one. The passphrase lives in this step's state only until Restore takes it. */
function RestoreFile({ model, network }: { model: OnboardingModel; network: string }) {
  const t = useT();
  const backend = useBackend();
  const id = useId();
  const file = model.file!;
  const [path, setPath] = useState(file.path);
  const [chooseError, setChooseError] = useState<string | null>(null);
  const reading = file.step === "inspecting";
  const plan = file.plan;
  const choose = async () => {
    setChooseError(null);
    const generation = file.generation;
    try {
      const chosen = await chooseFilePath(backend, "walletFile", false, null);
      // The user left this selection while the dialog was open.
      if (chosen === null || generation !== file.generation || model.step !== "restoreFile") return;
      setPath(chosen);
      await file.inspect(chosen);
    } catch (e) {
      setChooseError(fileErrorText(e));
    }
  };
  if (file.step === "path" || reading || !plan || !file.info)
    return (
      <>
        <Title>{t("onboarding.file.title")}</Title>
        <p className="onboarding-text">{t("onboarding.file.text")}</p>
        <form
          className="onboarding-form"
          onSubmit={(e) => {
            e.preventDefault();
            void file.inspect(path);
          }}
        >
          <DashButton type="button" variant="tintedBlue" size="large" fill data-autofocus data-testid="restore-file-choose" disabled={reading} onClick={() => void choose()}>
            {t("onboarding.file.choose")}
          </DashButton>
          <div className="form-field">
            <label htmlFor={`${id}-path`}>{t("onboarding.file.path")}</label>
            <input
              id={`${id}-path`}
              className="mono"
              data-testid="restore-file-path"
              autoComplete="off"
              spellCheck={false}
              value={path}
              disabled={reading}
              aria-describedby={`${id}-path-help`}
              onChange={(e) => setPath(e.target.value)}
            />
            <p id={`${id}-path-help`} className="form-hint">
              {t("onboarding.file.pathHelp")}
            </p>
          </div>
          {(file.error || chooseError) && (
            <p className="form-problem" role="alert" data-testid="restore-file-error">
              {chooseError ?? file.error}
            </p>
          )}
          <DashButton type="submit" variant="filledBlue" size="large" fill data-testid="restore-file-inspect" disabled={reading || path.trim() === ""}>
            {reading ? t("onboarding.file.inspecting") : t("onboarding.file.inspect")}
          </DashButton>
        </form>
      </>
    );
  return <FileDetails key={`${file.generation} ${file.path}`} model={model} network={network} />;
}

/** The inspected file: what it is, then its passphrase or a name. Keyed by the selection, so a
 * passphrase typed for one file is never sent for another. */
function FileDetails({ model, network }: { model: OnboardingModel; network: string }) {
  const t = useT();
  const id = useId();
  const file = model.file!;
  const plan = file.plan!;
  const info = file.info!;
  const [passphrase, setPassphrase] = useState("");
  const [name, setName] = useState("");
  const backup = plan.kind === "dwBackup";
  return (
    <>
      <Title>{t("onboarding.file.title")}</Title>
      <p className="onboarding-text" data-testid="restore-file-kind">
        {fileKindText(t, info)}
      </p>
      <p className="form-hint mono" data-testid="restore-file-chosen">
        {file.path}
      </p>
      <form
        className="onboarding-form"
        onSubmit={(e) => {
          e.preventDefault();
          const p = plan.passphrase ? passphrase : null;
          setPassphrase("");
          void model.restoreFromFile(p, name);
        }}
      >
        {plan.refusal ? (
          <p className="form-problem" role="alert" data-testid="restore-file-refusal">
            {refusalText(t, info, network)}
          </p>
        ) : (
          <>
            {plan.passphrase && (
              <div className="form-field">
                <label htmlFor={`${id}-pass`}>{t(backup ? "onboarding.file.backupPassphrase" : "onboarding.file.walletDatPassphrase")}</label>
                <input
                  id={`${id}-pass`}
                  type="password"
                  data-testid="restore-file-passphrase"
                  data-autofocus
                  autoComplete="off"
                  value={passphrase}
                  aria-describedby={backup ? `${id}-pass-help` : undefined}
                  onChange={(e) => {
                    setPassphrase(e.target.value);
                    file.edited();
                  }}
                />
                {backup && (
                  <p id={`${id}-pass-help`} className="form-hint">
                    {t("onboarding.file.backupPassphraseHelp")}
                  </p>
                )}
              </div>
            )}
            {plan.named && (
              <div className="form-field">
                <label htmlFor={`${id}-name`}>{t("onboarding.file.name")}</label>
                <input
                  id={`${id}-name`}
                  data-testid="restore-file-name"
                  autoComplete="off"
                  maxLength={64}
                  value={name}
                  onChange={(e) => {
                    setName(e.target.value);
                    file.edited();
                  }}
                />
              </div>
            )}
          </>
        )}
        {file.error && (
          <p className="form-problem" role="alert" data-testid="restore-file-error">
            {file.error}
          </p>
        )}
        {plan.refusal ? (
          <DashButton type="button" variant="filledBlue" size="large" fill data-autofocus data-testid="restore-file-other" onClick={() => file.back()}>
            {t("onboarding.file.otherFile")}
          </DashButton>
        ) : (
          <DashButton type="submit" variant="filledBlue" size="large" fill data-testid="restore-file-restore" disabled={model.busy}>
            {t("onboarding.file.restore")}
          </DashButton>
        )}
      </form>
    </>
  );
}

function ChoosePassphrase({ model }: { model: OnboardingModel }) {
  const t = useT();
  return (
    <>
      <Title>{t("onboarding.passphrase.title")}</Title>
      <p className="onboarding-text">{t("onboarding.passphrase.text")}</p>
      <NewPassphraseForm
        testId="onboarding-passphrase"
        error={model.passphraseError}
        submitLabel={t("onboarding.continue")}
        onEdit={() => model.passphraseEdited()}
        onSubmit={(text, confirmation) => model.submitPassphrase(text, confirmation)}
      >
        <p className="onboarding-note">{PassphraseText.warning}</p>
      </NewPassphraseForm>
      <details className="onboarding-advanced">
        <summary>{t("onboarding.advanced")}</summary>
        <p className="onboarding-note">{t("onboarding.passphrase.noPassphrase")}</p>
        <DashButton variant="plainBlue" disabled={model.busy} data-testid="onboarding-skip-passphrase" onClick={() => void model.skipEncryption()}>
          {t("onboarding.passphrase.skip")}
        </DashButton>
      </details>
    </>
  );
}

function ShowPhrase({ model }: { model: OnboardingModel }) {
  const t = useT();
  const [written, setWritten] = useState(false);
  return (
    <>
      <Title>{t("onboarding.showPhrase.title")}</Title>
      <p className="onboarding-text">{RecoveryText.writeDown}</p>
      <p className="onboarding-warning" role="note">
        {RecoveryText.warning}
      </p>
      <PhraseGrid words={model.phraseWords} testId="onboarding-phrase-words" />
      <label className="check-row">
        <input type="checkbox" data-testid="onboarding-written-down" checked={written} onChange={(e) => setWritten(e.target.checked)} />
        {t("onboarding.showPhrase.writtenDown")}
      </label>
      <DashButton
        variant="filledBlue"
        size="large"
        fill
        disabled={!written}
        data-testid="onboarding-phrase-continue"
        onClick={() => model.confirmWrittenDown()}
      >
        {t("onboarding.continue")}
      </DashButton>
    </>
  );
}

function VerifyPhrase({ model }: { model: OnboardingModel }) {
  const t = useT();
  return (
    <>
      <Title>{t("onboarding.verify.title")}</Title>
      <p className="onboarding-text">{t("onboarding.verify.text")}</p>
      <ol className="verify-slots" onCopy={noCopy}>
        {model.verifyChallenge.map((position, i) => (
          <li key={position} className="verify-slot" data-testid="verify-slot" data-filled={i < model.verifiedCount}>
            <span className="verify-slot-label">{t("onboarding.verify.word", { number: String(position + 1) })}</span>
            <span className="verify-slot-word">{model.verifiedWords[i] ?? ""}</span>
          </li>
        ))}
      </ol>
      <div className={model.verifyMistake ? "verify-chips verify-chips-mistake" : "verify-chips"} role="group" aria-label={t("onboarding.verify.words")} onCopy={noCopy}>
        {model.verifyChips.map((chip) => (
          <button
            key={chip.id}
            type="button"
            className="verify-chip"
            data-testid="verify-chip"
            disabled={chip.used || model.busy}
            onClick={() => void model.select(chip.id)}
          >
            {chip.word}
          </button>
        ))}
      </div>
      {/* A new element per wrong chip, so each one is announced. */}
      <p key={model.verifyMistakes} className="form-problem" role="alert" data-testid="verify-mistake">
        {model.verifyMistake ? t("onboarding.verify.mistake") : ""}
      </p>
    </>
  );
}

function RestoreOptions({ model }: { model: OnboardingModel }) {
  const t = useT();
  const id = useId();
  const scan = model.scanStart;
  const { birthHeight, coreCompat } = model.options;
  return (
    <>
      <Title>{t("onboarding.options.title")}</Title>
      <Bip39Field
        testId="restore-bip39"
        autoFocus
        hint={t("onboarding.options.bip39Hint")}
        value={model.bip39Passphrase}
        onChange={(text) => model.setBip39Passphrase(text)}
      />
      <label className="check-row">
        <input
          type="checkbox"
          data-testid="restore-core-compat"
          checked={coreCompat}
          disabled={model.entry.coreOnly}
          onChange={(e) => model.setCoreCompatible(e.target.checked)}
        />
        {t("onboarding.options.coreCompat")}
      </label>
      <div className="scan-start" role="group" aria-labelledby={`${id}-scan-title`}>
        <span id={`${id}-scan-title`} className="scan-start-title">
          {t("onboarding.options.scanFrom")}
        </span>
        <label className="check-row">
          <input type="radio" name={`${id}-scan`} data-testid="restore-scan-genesis" checked={scan === "genesis"} onChange={() => model.setScanStart("genesis")} />
          {t("onboarding.options.genesis")}
        </label>
        <label className="check-row">
          <input type="radio" name={`${id}-scan`} data-testid="restore-scan-date" checked={scan === "date"} onChange={() => model.setScanStart("date")} />
          {t("onboarding.options.date")}
        </label>
        {scan === "date" && (
          <input
            type="date"
            className="scan-input"
            aria-label={t("onboarding.options.dateLabel")}
            data-testid="restore-date"
            value={model.scanDate}
            onChange={(e) => model.setScanDate(e.target.value)}
          />
        )}
        <label className="check-row">
          <input type="radio" name={`${id}-scan`} data-testid="restore-scan-height" checked={scan === "height"} onChange={() => model.setScanStart("height")} />
          {t("onboarding.options.height")}
        </label>
        {scan === "height" && (
          <input
            type="number"
            min={0}
            step={1}
            className="scan-input"
            aria-label={t("onboarding.options.heightLabel")}
            data-testid="restore-height"
            value={model.scanHeight}
            onChange={(e) => model.setScanHeight(e.target.value)}
          />
        )}
        {model.birthDateError ? (
          <p className="form-problem" role="alert">
            {model.birthDateError}
          </p>
        ) : (
          <>
            <p className="form-hint" data-testid="restore-scan-start">
              {birthHeight ? t("onboarding.options.startsAt", { height: birthHeight }) : t("onboarding.options.startsAtFirst")}
            </p>
            {!birthHeight && (
              <p className="form-hint" data-testid="restore-first-block-slow">
                {t("onboarding.options.firstBlockSlow")}
              </p>
            )}
          </>
        )}
      </div>
      <DashButton
        variant="filledBlue"
        size="large"
        fill
        disabled={model.birthDateError !== null || model.busy}
        data-testid="restore-finish"
        onClick={() => void model.finishRestore()}
      >
        {t("onboarding.options.finish")}
      </DashButton>
    </>
  );
}
