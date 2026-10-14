// The recovery phrase on screen (IOS-003) and typed (IOS-007, U-02 D7, D18), with the BIP39
// passphrase and the wallet choice of the flows that take one. The grid has no copy button; the
// input is checked live and turns spell checking, autocorrect and autofill off.

import { useId, useState } from "react";
import { retryProtection, type Protection } from "../app/ui";
import type { WalletChoice } from "../onboarding/model";
import type { PhraseEntry } from "../onboarding/phraseEntry";
import { PhraseText } from "../onboarding/phraseEntry";
import { completeWord, partialWord } from "../onboarding/words";
import { DashButton } from "./basics";

export const ProtectionText = {
  preparing: "Preparing…",
  failed:
    "Dash Wallet could not keep this window out of screenshots and screen sharing, so it will not show or take a recovery phrase.",
};

/** What a view that shows or takes a recovery phrase shows until the window is kept out of
 * screen captures (U-02 D27): "Preparing…", or why it is not, with Try again. Never the secret. */
export function ProtectionPending({ protection, onCancel }: { protection: Protection; onCancel?: () => void }) {
  if (protection !== "failed") {
    return (
      <p className="sheet-text" role="status" data-testid="protection-pending">
        {ProtectionText.preparing}
      </p>
    );
  }
  return (
    <div role="alert" data-testid="protection-failed">
      <p className="form-problem">{ProtectionText.failed}</p>
      <div className="sheet-actions">
        {onCancel && (
          <DashButton variant="plainBlue" data-testid="protection-cancel" onClick={onCancel}>
            Cancel
          </DashButton>
        )}
        <DashButton variant="filledBlue" data-autofocus data-testid="protection-retry" onClick={retryProtection}>
          Try again
        </DashButton>
      </div>
    </div>
  );
}

/** Copying a phrase puts it where clipboard managers keep it: the phrase's elements refuse it
 * (D18), and their text cannot be selected (`.phrase-grid`, `.verify-*` in app.css). */
export const noCopy = (e: React.ClipboardEvent) => e.preventDefault();

/** The words, numbered, in columns. */
export function PhraseGrid({ words, testId }: { words: string[]; testId: string }) {
  return (
    <ol className="phrase-grid" data-testid={testId} translate="no" onCopy={noCopy} onCut={noCopy}>
      {words.map((word, i) => (
        <li key={i} className="phrase-cell">
          <span className="phrase-index" aria-hidden="true">
            {i + 1}
          </span>
          <span className="phrase-word">{word}</span>
        </li>
      ))}
    </ol>
  );
}

/** A textarea for a recovery phrase with the engine's verdict, the word suggestions (click, or
 * Tab for the first) and the Dash Core-only note. The text lives in the caller (`text`). */
export function PhraseInput({
  testId,
  entry,
  text,
  onChange,
}: {
  testId: string;
  entry: PhraseEntry;
  text: string;
  onChange: (text: string) => void;
}) {
  const id = useId();
  const [blurred, setBlurred] = useState(false);
  const typing = partialWord(text) !== "" && !blurred;
  const problem = entry.shownProblem(typing);
  const complete = (word: string) => onChange(completeWord(text, word));
  return (
    <div className="phrase-input">
      <div className="form-field">
        <label htmlFor={id}>Recovery phrase</label>
        <textarea
          id={id}
          className="phrase-textarea"
          data-testid={`${testId}-input`}
          data-autofocus
          rows={4}
          value={text}
          autoComplete="off"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
          translate="no"
          aria-invalid={problem ? true : undefined}
          aria-describedby={`${id}-state`}
          onChange={(e) => {
            setBlurred(false);
            onChange(e.target.value);
          }}
          onBlur={() => setBlurred(true)}
          onKeyDown={(e) => {
            if (e.key === "Tab" && !e.shiftKey && entry.suggestions.length > 0) {
              e.preventDefault();
              complete(entry.suggestions[0]);
            }
          }}
        />
      </div>
      {entry.suggestions.length > 0 && (
        <div className="phrase-suggestions" role="group" aria-label="Suggestions">
          {entry.suggestions.map((word) => (
            <button key={word} type="button" className="phrase-suggestion" data-testid={`${testId}-suggestion`} onClick={() => complete(word)}>
              {word}
            </button>
          ))}
        </div>
      )}
      <div id={`${id}-state`} aria-live="polite">
        {problem ? (
          <p className="form-problem" data-testid={`${testId}-problem`}>
            {problem}
          </p>
        ) : entry.valid ? (
          <p className="form-ok" data-testid={`${testId}-valid`}>
            {entry.coreOnly ? PhraseText.coreOnly : "The recovery phrase is valid."}
          </p>
        ) : null}
      </div>
    </div>
  );
}

/** The BIP39 passphrase ("25th word") of a restored phrase; optional. */
export function Bip39Field({
  testId,
  value,
  onChange,
  autoFocus = false,
  hint,
}: {
  testId: string;
  value: string;
  onChange: (text: string) => void;
  autoFocus?: boolean;
  hint?: string;
}) {
  const id = useId();
  return (
    <div className="form-field">
      <label htmlFor={id}>BIP39 passphrase (optional)</label>
      <input
        id={id}
        type="password"
        autoComplete="off"
        data-testid={testId}
        data-autofocus={autoFocus || undefined}
        value={value}
        aria-describedby={hint ? `${id}-hint` : undefined}
        onChange={(e) => onChange(e.target.value)}
      />
      {hint && (
        <span id={`${id}-hint`} className="form-hint">
          {hint}
        </span>
      )}
    </div>
  );
}

/** Which wallet a flow is for, when several qualify. */
export function WalletChoiceList({ wallets, testId, onChoose }: { wallets: WalletChoice[]; testId: string; onChoose: (walletId: string) => void }) {
  return (
    <div className="choice-list" role="group" aria-label="Wallets">
      {wallets.map((w) => (
        <button key={w.walletId} type="button" className="choice-row" data-testid={testId} onClick={() => onChoose(w.walletId)}>
          {w.name}
        </button>
      ))}
    </div>
  );
}
