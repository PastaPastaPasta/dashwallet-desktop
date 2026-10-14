// The recovery phrase on screen (IOS-003) and typed (IOS-007, U-02 D7, D18). The grid has no
// copy button; the input is checked live and turns spell checking, autocorrect and autofill off.

import { useId, useState } from "react";
import type { PhraseEntry } from "../onboarding/phraseEntry";
import { PhraseText } from "../onboarding/phraseEntry";
import { completeWord, partialWord } from "../onboarding/words";

/** The words, numbered, in columns. */
export function PhraseGrid({ words, testId }: { words: string[]; testId: string }) {
  return (
    <ol className="phrase-grid" data-testid={testId} translate="no">
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
