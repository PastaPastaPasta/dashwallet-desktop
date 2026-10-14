// Forgot passphrase (IOS-014, U-02 D11): a sheet over the lock screen. The recovery unlocks the
// vault, so the sheet stays over the shell to say what happened; closing it drops the phrase.

import { useEffect, useState } from "react";
import { useBackend } from "../api/queries";
import { useSecretScreen } from "../app/ui";
import { DashButton } from "../components/basics";
import { NewPassphraseForm } from "../components/passphrase";
import { Bip39Field, PhraseInput, WalletChoiceList } from "../components/phrase";
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
  // The fields go once the phrase step ends: the model keeps the accepted phrase for the reset
  // (and drops it on Back or after the reset), the screen has no use for it any more.
  const entering = model.step === "phrase";
  useEffect(() => {
    if (entering) return;
    setPhrase("");
    setBip39("");
  }, [entering]);
  // The whole sheet, so protection is on before a phrase is typed (D15).
  useSecretScreen(true);
  return (
    <Sheet
      title={ForgotText.title}
      width="medium"
      testId="forgot-passphrase"
      focusKey={model.step}
      // The reset unlocks the vault and may leave wallets watch-only: Done must say so.
      closable={model.step !== "working"}
      onClose={onClose}
    >
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
          <WalletChoiceList wallets={model.wallets} testId="forgot-wallet" onChoose={(id) => model.chooseWallet(id)} />
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
          <Bip39Field
            testId="forgot-bip39"
            value={bip39}
            onChange={(text) => {
              setBip39(text);
              model.setBip39Passphrase(text);
            }}
          />
          <div className="sheet-actions">
            {model.wallets.length > 1 && (
              <DashButton variant="plainBlue" data-testid="forgot-back" onClick={() => model.back()}>
                Back
              </DashButton>
            )}
            <DashButton variant="filledBlue" disabled={!model.entry.valid} data-testid="forgot-continue" onClick={() => model.continuePhrase()}>
              Continue
            </DashButton>
          </div>
        </>
      );
    case "newPassphrase":
      return (
        <>
          <NewPassphraseForm
            testId="forgot-new"
            error={model.error}
            submitLabel="Reset passphrase"
            onEdit={() => model.passphraseEdited()}
            onSubmit={(text, confirmation) => model.submitNewPassphrase(text, confirmation)}
          />
          <DashButton variant="plainBlue" data-testid="forgot-back" onClick={() => model.back()}>
            Back
          </DashButton>
        </>
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
