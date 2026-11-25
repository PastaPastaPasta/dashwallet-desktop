// The screens shown instead of the wallet when its data can't be opened: a store refusal, the screen the
// engine gives as it gives it (it changed nothing), and StorageInUse (the session before still holds the
// data). Each replaces the shell, so neither ever shows onboarding, an empty wallet list or a zero balance.

import { useId, useState, type ReactNode } from "react";
import type { Backend } from "../api/backend";
import type { StoreRefusedData } from "../api/storeOpen";
import { useBackend } from "../api/queries";
import { DashButton, DashLogo } from "../components/basics";
import { Model, useModel } from "../onboarding/model";
import { t, useT, type Translate } from "../i18n";
import { flatten } from "../i18n/core";
import { en } from "../i18n/en";
import { Problem } from "./settings/parts";

const ENGLISH = flatten(en);

/** The catalog's text for a `storeUpgrade.*` key in the language in use; for any other key, or one the
 * catalog lacks, the engine's English: never other copy. */
function textFor(tr: Translate, key: string, english: string, folder: string): string {
  return key.startsWith("storeUpgrade.") && ENGLISH.has(key) ? (tr as unknown as (key: string, values: Record<string, string>) => string)(key, { folder }) : english;
}

/** What each button key does. A key not here renders disabled: its action comes with the screen that
 * adds it (Report a problem, Restore earlier data). */
export function buttonAction(key: string): "quit" | null {
  return key.endsWith(".quit") ? "quit" : null;
}

/** Quit and restart through dw-app, with the latest failure. */
export class OpenFailureActions extends Model {
  problem: string | null = null;

  constructor(private readonly api: Pick<Backend, "quitApp" | "restartApp">) {
    super();
  }

  quit = () => this.#run(() => this.api.quitApp());
  restart = () => this.#run(() => this.api.restartApp());

  async #run(call: () => Promise<null>) {
    this.problem = null;
    this.changed();
    try {
      await call();
    } catch (e) {
      this.problem = e instanceof Error ? e.message : t("onboarding.error.generic");
      this.changed();
    }
  }
}

function useActions() {
  const backend = useBackend();
  const [actions] = useState(() => new OpenFailureActions(backend));
  return useModel(actions);
}

function Panel({ id, testId, title, text, problem, children }: { id: string; testId: string; title: string; text: string; problem: string | null; children: ReactNode }) {
  return (
    <div className="onboarding" role="alertdialog" aria-modal="true" aria-labelledby={`${id}-title`} aria-describedby={`${id}-body`} data-testid={testId}>
      <div className="onboarding-panel">
        <header className="onboarding-header">
          <span className="onboarding-header-side" />
          <span className="onboarding-brand">
            <DashLogo className="onboarding-logo" />
          </span>
          <span className="onboarding-header-side" />
        </header>
        <main className="onboarding-body">
          <h1 className="onboarding-title" id={`${id}-title`}>
            {title}
          </h1>
          <p className="onboarding-text" id={`${id}-body`} data-testid={`${testId}-text`}>
            {text}
          </p>
          {children}
          <Problem text={problem} testId={`${testId}-problem`} />
        </main>
      </div>
    </div>
  );
}

export interface StoreRefusalViewProps {
  refusal: StoreRefusedData;
  problem: string | null;
  onQuit: () => void;
}

/** The engine's screen as it gives it: title, text, a button per its buttons, and its hint under them. */
export function StoreRefusalView({ refusal, problem, onQuit }: StoreRefusalViewProps) {
  const tr = useT();
  const id = useId();
  const { screen, folder } = refusal;
  return (
    <Panel id={id} testId="store-refused" title={textFor(tr, screen.titleKey, screen.title, folder)} text={textFor(tr, screen.textKey, screen.text, folder)} problem={problem}>
      <div className="onboarding-actions">
        {screen.buttons.map((b) => {
          const action = buttonAction(b.key);
          return (
            <DashButton
              key={b.key}
              variant={b.destructive ? "plainRed" : b.defaultFocus ? "filledBlue" : "plainBlue"}
              size="large"
              fill
              autoFocus={b.defaultFocus}
              disabled={action === null}
              aria-describedby={screen.hint ? `${id}-hint` : undefined}
              data-testid={`store-refused-button-${b.key}`}
              onClick={action === "quit" ? onQuit : undefined}
            >
              {textFor(tr, b.key, b.label, folder)}
            </DashButton>
          );
        })}
      </div>
      {screen.hint && (
        <p id={`${id}-hint`} className="field-hint" data-testid="store-refused-hint">
          {textFor(tr, screen.hint.key, screen.hint.text, folder)}
        </p>
      )}
    </Panel>
  );
}

export function StoreRefusalScreen({ refusal }: { refusal: StoreRefusedData }) {
  const exit = useActions();
  return <StoreRefusalView refusal={refusal} problem={exit.problem} onQuit={() => void exit.quit()} />;
}

export interface StorageInUseViewProps {
  retryable: boolean;
  /** Try again's open is running: the button waits for it. */
  retrying: boolean;
  problem: string | null;
  onRetry: () => void;
  onRestart: () => void;
}

/** `retryable: false` (a build where only a restart frees the data): no Try again, in the text or as a button. */
export function StorageInUseView({ retryable, retrying, problem, onRetry, onRestart }: StorageInUseViewProps) {
  const tr = useT();
  const id = useId();
  return (
    <Panel id={id} testId="storage-in-use" title={tr("app.storageInUse.title")} text={retryable ? tr("app.storageInUse.text") : tr("app.storageInUse.textRestartOnly")} problem={problem}>
      <div className="onboarding-actions">
        {retryable && (
          <DashButton variant="filledBlue" size="large" fill autoFocus disabled={retrying} aria-busy={retrying} data-testid="storage-in-use-retry" onClick={onRetry}>
            {tr("app.storageInUse.tryAgain")}
          </DashButton>
        )}
        <DashButton variant={retryable ? "plainBlue" : "filledBlue"} size="large" fill autoFocus={!retryable} data-testid="storage-in-use-restart" onClick={onRestart}>
          {tr("app.storageInUse.restart")}
        </DashButton>
      </div>
    </Panel>
  );
}

export function StorageInUseScreen({ retryable, retrying, onRetry }: { retryable: boolean; retrying: boolean; onRetry: () => void }) {
  const exit = useActions();
  return <StorageInUseView retryable={retryable} retrying={retrying} problem={exit.problem} onRetry={onRetry} onRestart={() => void exit.restart()} />;
}
