// Show Recovery Phrase (IOS-003, IOS-108, U-02 D13): Wallet ▸ Show Recovery Phrase…. The phrase
// is in the view model only while the sheet shows it; closing the sheet drops it.

import { useEffect, useState } from "react";
import { useBackend } from "../api/queries";
import { useSecretScreen } from "../app/ui";
import { DashButton } from "../components/basics";
import { PassphrasePrompt } from "../components/passphrase";
import { PhraseGrid, WalletChoiceList } from "../components/phrase";
import { Sheet } from "../components/sheet";
import { useModel } from "../onboarding/model";
import { RecoveryPhraseModel, RecoveryText } from "../onboarding/recovery";
import { useLockTimer } from "./LockScreen";

export function RecoveryPhraseSheet({ walletId, onClose }: { walletId: string | null; onClose: () => void }) {
  const backend = useBackend();
  const [model] = useState(() => new RecoveryPhraseModel(backend));
  useModel(model);
  useLockTimer(model.lock);
  useEffect(() => {
    void model.load(walletId);
    return () => model.dispose();
  }, [model, walletId]);
  // The whole sheet, so protection is on before the phrase shows (D15).
  useSecretScreen(true);
  const name = model.wallets.find((w) => w.walletId === model.walletId)?.name;
  return (
    <Sheet title={RecoveryText.title} width="medium" testId="recovery-phrase" focusKey={model.step} onClose={onClose}>
      {(model.step === "loading" || model.step === "working") && (
        <p className="sheet-text" role="status">
          {model.step === "loading" ? "Loading…" : "Checking…"}
        </p>
      )}
      {model.step === "unavailable" && <p className="sheet-text">{RecoveryText.noPhrase}</p>}
      {model.step === "failed" && (
        <p className="form-problem" role="alert" data-testid="recovery-error">
          {model.error}
        </p>
      )}
      {model.step === "wallet" && (
        <>
          <p className="sheet-text">Which wallet's recovery phrase do you want to see?</p>
          <WalletChoiceList wallets={model.wallets} testId="recovery-wallet" onChoose={(id) => model.choose(id)} />
        </>
      )}
      {model.step === "confirm" && (
        <>
          {name && model.wallets.length > 1 && <p className="sheet-text">{name}</p>}
          <p className="sheet-warning" role="note">
            {RecoveryText.warning}
          </p>
          {model.encrypted ? (
            <PassphrasePrompt
              testId="recovery-passphrase"
              error={model.lock.message}
              busy={model.lock.working}
              disabled={model.lock.blocked}
              submitLabel="Show recovery phrase"
              onSubmit={(p) => model.reveal(p)}
            />
          ) : (
            <>
              {model.error && (
                <p className="form-problem" role="alert" data-testid="recovery-error">
                  {model.error}
                </p>
              )}
              <DashButton variant="filledBlue" size="large" fill data-autofocus data-testid="recovery-reveal" onClick={() => void model.reveal("")}>
                Show recovery phrase
              </DashButton>
            </>
          )}
        </>
      )}
      {model.step === "shown" && (
        <>
          <p className="sheet-text">{RecoveryText.writeDown}</p>
          <PhraseGrid words={model.words} testId="recovery-words" />
          {model.hasBip39 && (
            <p className="sheet-warning" data-testid="recovery-bip39">
              This wallet also uses a BIP39 passphrase. You need it with the recovery phrase to restore the wallet.
            </p>
          )}
          <DashButton variant="filledBlue" size="large" fill data-autofocus data-testid="recovery-done" onClick={onClose}>
            Done
          </DashButton>
        </>
      )}
    </Sheet>
  );
}
