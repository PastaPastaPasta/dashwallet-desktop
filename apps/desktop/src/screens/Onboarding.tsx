// Onboarding (IOS-002, IOS-004, IOS-007, IOS-010; U-02 D1–D7, D17): a full-window surface over
// the shell for the first wallet and for Wallet ▸ Add Wallet…. The steps are `OnboardingModel`'s;
// this renders them. The restore text is the only secret held here, in the textarea's state,
// until the flow ends.

import { useEffect, useId, useRef, useState } from "react";
import { errorMessage } from "../api/backend";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { useBackend, useWallets } from "../api/queries";
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
import { useLockTimer } from "./LockScreen";

const BACK_STEPS: ReadonlySet<OnboardingStep> = new Set([
  "choosePassphrase",
  "unlockVault",
  "showPhrase",
  "verifyPhrase",
  "restorePhrase",
  "restoreOptions",
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
  const [model] = useState(() => new OnboardingModel(backend, network.kind));
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
  }, [step, ready]);

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
      <p className="onboarding-text">{model.flow === "restore" ? t("onboarding.ready.restored") : t("onboarding.ready.created")}</p>
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
  const t = useT();
  const picker = useNetworkPicker();
  const showPicker = onCancel === null && (picker.available || picker.loading);
  const wait = showPicker && (picker.loading || picker.switching !== null);
  return (
    <>
      <Title>{onCancel ? t("onboarding.welcome.addTitle") : t("onboarding.welcome.title")}</Title>
      <p className="onboarding-text">{t("onboarding.welcome.text")}</p>
      {/* Without the command (a build before it) the first screen shows no picker. */}
      {showPicker && <NetworkPicker state={picker} testId="onboarding-network" />}
      <div className="onboarding-actions">
        <DashButton variant="filledBlue" size="large" fill disabled={model.busy || wait} data-testid="onboarding-create" onClick={() => void model.startCreate()}>
          {t("onboarding.welcome.create")}
        </DashButton>
        <DashButton variant="plainBlue" size="large" fill disabled={model.busy || wait} data-testid="onboarding-restore" onClick={() => void model.startRestore()}>
          {t("onboarding.welcome.restore")}
        </DashButton>
        {onCancel && (
          <DashButton variant="plainBlue" size="large" fill data-testid="onboarding-cancel" onClick={onCancel}>
            {t("common.cancel")}
          </DashButton>
        )}
      </div>
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
          <p className="form-hint" data-testid="restore-scan-start">
            {birthHeight ? t("onboarding.options.startsAt", { height: birthHeight }) : t("onboarding.options.startsAtFirst")}
          </p>
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
