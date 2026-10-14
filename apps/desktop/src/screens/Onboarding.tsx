// Onboarding (IOS-002, IOS-004, IOS-007, IOS-010; U-02 D1–D7, D17): a full-window surface over
// the shell for the first wallet and for Wallet ▸ Add Wallet…. The steps are `OnboardingModel`'s;
// this renders them. The restore text is the only secret held here, in the textarea's state,
// until the flow ends.

import { useEffect, useId, useRef, useState } from "react";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import { useBackend } from "../api/queries";
import { useSecretScreen } from "../app/ui";
import { DashButton, DashLogo, NetworkCapsule } from "../components/basics";
import { NewPassphraseForm, PassphrasePrompt } from "../components/passphrase";
import { Bip39Field, noCopy, PhraseGrid, PhraseInput, ProtectionPending } from "../components/phrase";
import { useModel } from "../onboarding/model";
import { OnboardingModel, type OnboardingStep } from "../onboarding/onboarding";
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
  onCancel,
  onDone,
}: {
  network: NetworkInfo;
  /** Closes Add Wallet; `null` for the first wallet, which cannot be skipped. */
  onCancel: (() => void) | null;
  onDone: (walletId: string | null) => void;
}) {
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
    <div className="onboarding" data-testid="onboarding">
      <div className="onboarding-panel">
        <header className="onboarding-header">
          <span className="onboarding-header-side">
            {ready && BACK_STEPS.has(step) && (
              <DashButton variant="plainBlue" size="small" data-testid="onboarding-back" disabled={model.busy} onClick={() => model.back()}>
                Back
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
            <Step model={model} onCancel={onCancel} onDone={onDone} restoreText={restoreText} setRestoreText={setRestoreText} />
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
  onCancel,
  onDone,
  restoreText,
  setRestoreText,
}: {
  model: OnboardingModel;
  onCancel: (() => void) | null;
  onDone: (walletId: string | null) => void;
  restoreText: string;
  setRestoreText: (text: string) => void;
}) {
  switch (model.step) {
    case "welcome":
      return <Welcome model={model} onCancel={onCancel} />;
    case "choosePassphrase":
      return <ChoosePassphrase model={model} />;
    case "unlockVault":
      return (
        <>
          <Title>Unlock wallet</Title>
          <p className="onboarding-text">Enter your wallet passphrase to add a wallet.</p>
          <PassphrasePrompt
            testId="onboarding-unlock"
            error={model.lock.message}
            busy={model.busy}
            disabled={model.lock.blocked}
            submitLabel="Unlock"
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
          <Title>Restore a wallet</Title>
          <p className="onboarding-text">Enter the wallet's recovery phrase: 12, 15, 18, 21 or 24 words.</p>
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
            Continue
          </DashButton>
        </>
      );
    case "restoreOptions":
      return <RestoreOptions model={model} />;
    case "working":
      return (
        <>
          <Title>Adding the wallet</Title>
          <p className="onboarding-text" role="status">
            Adding the wallet…
          </p>
        </>
      );
    case "done":
      return (
        <>
          <Title>Your wallet is ready</Title>
          <p className="onboarding-text">
            {model.flow === "restore"
              ? "Dash Wallet now scans the blockchain for the wallet's transactions. This can take a while."
              : "Keep your recovery phrase safe. It is the only way to restore this wallet."}
          </p>
          <DashButton variant="filledBlue" size="large" fill data-autofocus data-testid="onboarding-open" onClick={() => onDone(model.walletId)}>
            Open wallet
          </DashButton>
        </>
      );
    case "failed":
      return (
        <>
          <Title>Could not add the wallet</Title>
          <p className="form-problem" role="alert" data-testid="onboarding-failure">
            {model.failure}
          </p>
          <DashButton variant="filledBlue" size="large" fill data-autofocus data-testid="onboarding-retry" onClick={() => model.back()}>
            Back
          </DashButton>
        </>
      );
  }
}

function Welcome({ model, onCancel }: { model: OnboardingModel; onCancel: (() => void) | null }) {
  return (
    <>
      <Title>{onCancel ? "Add a wallet" : "Welcome to Dash Wallet"}</Title>
      <p className="onboarding-text">Create a new wallet, or restore one with its recovery phrase.</p>
      <div className="onboarding-actions">
        <DashButton variant="filledBlue" size="large" fill disabled={model.busy} data-testid="onboarding-create" onClick={() => void model.startCreate()}>
          Create a new wallet
        </DashButton>
        <DashButton variant="plainBlue" size="large" fill disabled={model.busy} data-testid="onboarding-restore" onClick={() => void model.startRestore()}>
          Restore from recovery phrase
        </DashButton>
        {onCancel && (
          <DashButton variant="plainBlue" size="large" fill data-testid="onboarding-cancel" onClick={onCancel}>
            Cancel
          </DashButton>
        )}
      </div>
      <details className="onboarding-advanced">
        <summary>Advanced options</summary>
        <label className="check-row">
          <input
            type="checkbox"
            data-testid="onboarding-24-words"
            checked={model.wordCount === 24}
            onChange={(e) => model.setWordCount(e.target.checked ? 24 : 12)}
          />
          Create a 24-word recovery phrase
        </label>
      </details>
    </>
  );
}

function ChoosePassphrase({ model }: { model: OnboardingModel }) {
  return (
    <>
      <Title>Choose a passphrase</Title>
      <p className="onboarding-text">
        The passphrase encrypts the wallet on this computer. You need it to unlock the wallet and to send Dash.
      </p>
      <NewPassphraseForm
        testId="onboarding-passphrase"
        error={model.passphraseError}
        submitLabel="Continue"
        onEdit={() => model.passphraseEdited()}
        onSubmit={(text, confirmation) => model.submitPassphrase(text, confirmation)}
      >
        <p className="onboarding-note">{PassphraseText.warning}</p>
      </NewPassphraseForm>
      <details className="onboarding-advanced">
        <summary>Advanced options</summary>
        <p className="onboarding-note">
          Without a passphrase, the wallet's key is kept in this computer's secret store, and anyone who can use your
          account on this computer can spend your Dash.
        </p>
        <DashButton variant="plainBlue" disabled={model.busy} data-testid="onboarding-skip-passphrase" onClick={() => void model.skipEncryption()}>
          Continue without a passphrase
        </DashButton>
      </details>
    </>
  );
}

function ShowPhrase({ model }: { model: OnboardingModel }) {
  const [written, setWritten] = useState(false);
  return (
    <>
      <Title>Your recovery phrase</Title>
      <p className="onboarding-text">{RecoveryText.writeDown}</p>
      <p className="onboarding-warning" role="note">
        {RecoveryText.warning}
      </p>
      <PhraseGrid words={model.phraseWords} testId="onboarding-phrase-words" />
      <label className="check-row">
        <input type="checkbox" data-testid="onboarding-written-down" checked={written} onChange={(e) => setWritten(e.target.checked)} />
        I have written down my recovery phrase
      </label>
      <DashButton
        variant="filledBlue"
        size="large"
        fill
        disabled={!written}
        data-testid="onboarding-phrase-continue"
        onClick={() => model.confirmWrittenDown()}
      >
        Continue
      </DashButton>
    </>
  );
}

function VerifyPhrase({ model }: { model: OnboardingModel }) {
  return (
    <>
      <Title>Verify your recovery phrase</Title>
      <p className="onboarding-text">Choose the words in the order of their numbers.</p>
      <ol className="verify-slots" onCopy={noCopy}>
        {model.verifyChallenge.map((position, i) => (
          <li key={position} className="verify-slot" data-testid="verify-slot" data-filled={i < model.verifiedCount}>
            <span className="verify-slot-label">Word {position + 1}</span>
            <span className="verify-slot-word">{model.verifiedWords[i] ?? ""}</span>
          </li>
        ))}
      </ol>
      <div className={model.verifyMistake ? "verify-chips verify-chips-mistake" : "verify-chips"} role="group" aria-label="Words" onCopy={noCopy}>
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
        {model.verifyMistake ? "Wrong word. Try again." : ""}
      </p>
    </>
  );
}

function RestoreOptions({ model }: { model: OnboardingModel }) {
  const id = useId();
  const scan = model.scanStart;
  const { birthHeight, coreCompat } = model.options;
  return (
    <>
      <Title>Restore options</Title>
      <Bip39Field
        testId="restore-bip39"
        autoFocus
        hint="Only if the wallet was created with one. It is not your wallet passphrase."
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
        Dash Core compatible (a recovery phrase from Dash Core)
      </label>
      <fieldset className="scan-start">
        <legend>Scan the blockchain from</legend>
        <label className="check-row">
          <input type="radio" name={`${id}-scan`} data-testid="restore-scan-genesis" checked={scan === "genesis"} onChange={() => model.setScanStart("genesis")} />
          The beginning (slowest, finds every transaction)
        </label>
        <label className="check-row">
          <input type="radio" name={`${id}-scan`} data-testid="restore-scan-date" checked={scan === "date"} onChange={() => model.setScanStart("date")} />
          The date the wallet was created
        </label>
        {scan === "date" && (
          <input
            type="date"
            className="scan-input"
            aria-label="Wallet creation date"
            data-testid="restore-date"
            value={model.scanDate}
            onChange={(e) => model.setScanDate(e.target.value)}
          />
        )}
        <label className="check-row">
          <input type="radio" name={`${id}-scan`} data-testid="restore-scan-height" checked={scan === "height"} onChange={() => model.setScanStart("height")} />
          A block height
        </label>
        {scan === "height" && (
          <input
            type="number"
            min={0}
            step={1}
            className="scan-input"
            aria-label="Block height"
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
            {birthHeight ? `The scan starts at block ${birthHeight.toLocaleString("en-US")}.` : "The scan starts at the first block."}
          </p>
        )}
      </fieldset>
      <DashButton
        variant="filledBlue"
        size="large"
        fill
        disabled={model.birthDateError !== null || model.busy}
        data-testid="restore-finish"
        onClick={() => void model.finishRestore()}
      >
        Restore wallet
      </DashButton>
    </>
  );
}
