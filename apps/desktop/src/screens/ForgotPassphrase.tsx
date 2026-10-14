// Forgot passphrase (IOS-014, U-02 D11): a sheet over the lock screen. The recovery unlocks the
// vault, so the sheet stays over the shell to say what happened; closing it drops the phrase.

import { useEffect, useId, useState } from "react";
import { useBackend } from "../api/queries";
import { useSecretScreen } from "../app/ui";
import { DashButton } from "../components/basics";
import { NewPassphraseForm } from "../components/passphrase";
import { PhraseInput } from "../components/phrase";
import { Sheet } from "../components/sheet";
import { ForgotPassphraseModel, ForgotText } from "../onboarding/forgot";
import { useModel } from "../onboarding/model";

export function ForgotPassphraseSheet({ onClose }: { onClose: () => void }) {
  const backend = useBackend();
  const [model] = useState(() => new ForgotPassphraseModel(backend));
  useModel(model);
  useEffect(() => {
    void model.load();
    return () => model.dispose();
  }, [model]);
  const [phrase, setPhrase] = useState("");
  const [bip39, setBip39] = useState("");
  // The fields go when the model drops its copies: on leaving the phrase steps, and on a submit
  // whatever its outcome (it passes through `working`).
  const entering = model.step === "phrase" || model.step === "newPassphrase";
  useEffect(() => {
    if (entering) return;
    setPhrase("");
    setBip39("");
  }, [entering]);
  useSecretScreen(model.step === "phrase");
  return (
    <Sheet title={ForgotText.title} width="medium" testId="forgot-passphrase" focusKey={model.step} onClose={onClose}>
      <ForgotStep model={model} phrase={phrase} setPhrase={setPhrase} bip39={bip39} setBip39={setBip39} onClose={onClose} />
    </Sheet>
  );
}

function ForgotStep({
  model,
  phrase,
  setPhrase,
  bip39,
  setBip39,
  onClose,
}: {
  model: ForgotPassphraseModel;
  phrase: string;
  setPhrase: (text: string) => void;
  bip39: string;
  setBip39: (text: string) => void;
  onClose: () => void;
}) {
  const id = useId();
  switch (model.step) {
    case "loading":
    case "working":
      return (
        <p className="sheet-text" role="status">
          {model.step === "loading" ? "Loading…" : "Resetting the passphrase…"}
        </p>
      );
    case "unavailable":
      return <p className="sheet-text">No wallet's keys are in this vault, so there is nothing to recover.</p>;
    case "wallet":
      return (
        <>
          <p className="sheet-text">Which wallet's recovery phrase do you have?</p>
          <div className="choice-list" role="group" aria-label="Wallets">
            {model.wallets.map((w) => (
              <button key={w.walletId} type="button" className="choice-row" data-testid="forgot-wallet" onClick={() => model.chooseWallet(w.walletId)}>
                {w.name}
              </button>
            ))}
          </div>
        </>
      );
    case "consequences":
      return (
        <>
          <p className="sheet-warning" role="note" data-testid="forgot-consequences">
            {ForgotText.consequences(model.others)}
          </p>
          <div className="sheet-actions">
            <DashButton variant="plainBlue" data-testid="forgot-back" onClick={() => model.back()}>
              Back
            </DashButton>
            <DashButton variant="filledBlue" data-autofocus data-testid="forgot-accept" onClick={() => model.acceptConsequences()}>
              Continue
            </DashButton>
          </div>
        </>
      );
    case "phrase":
      return (
        <>
          <p className="sheet-text">{ForgotText.prompt}</p>
          {model.error && (
            <p className="form-problem" role="alert" data-testid="forgot-error">
              {model.error}
            </p>
          )}
          <PhraseInput
            testId="forgot-phrase"
            entry={model.entry}
            text={phrase}
            onChange={(text) => {
              setPhrase(text);
              void model.updatePhrase(text);
            }}
          />
          <div className="form-field">
            <label htmlFor={`${id}-bip39`}>BIP39 passphrase (optional)</label>
            <input
              id={`${id}-bip39`}
              type="password"
              autoComplete="off"
              data-testid="forgot-bip39"
              value={bip39}
              onChange={(e) => {
                setBip39(e.target.value);
                model.setBip39Passphrase(e.target.value);
              }}
            />
          </div>
          <DashButton
            variant="filledBlue"
            size="large"
            fill
            disabled={!model.entry.valid}
            data-testid="forgot-continue"
            onClick={() => model.continuePhrase()}
          >
            Continue
          </DashButton>
        </>
      );
    case "newPassphrase":
      return (
        <NewPassphraseForm
          testId="forgot-new"
          strength={model.passphraseStrength}
          error={model.error}
          submitLabel="Reset passphrase"
          onDraft={(text) => model.updatePassphraseDraft(text)}
          onSubmit={(text, confirmation) => model.submitNewPassphrase(text, confirmation)}
        />
      );
    case "done":
      return (
        <>
          <p className="sheet-text" role="status" data-testid="forgot-done">
            {ForgotText.reset}
          </p>
          {model.walletsWithoutSecrets.length > 0 && (
            <p className="sheet-warning" data-testid="forgot-watch-only">
              {ForgotText.walletsWithoutSecrets(model.walletsWithoutSecrets)}
            </p>
          )}
          <DashButton variant="filledBlue" size="large" fill data-autofocus data-testid="forgot-close" onClick={onClose}>
            Done
          </DashButton>
        </>
      );
    case "failed":
      return (
        <>
          <p className="form-problem" role="alert" data-testid="forgot-error">
            {model.error}
          </p>
          <DashButton variant="filledBlue" data-autofocus data-testid="forgot-retry" onClick={() => model.back()}>
            Back
          </DashButton>
        </>
      );
  }
}
