// The screens shown instead of the wallet when its data can't be opened: a store refusal, the screen the
// engine gives as it gives it (it changed nothing); a restore the user asked for that stopped the open,
// by its state; and StorageInUse (the session before still holds the data). Each replaces the shell, so
// none ever shows onboarding, an empty wallet list or a zero balance. A restore only ever runs at the
// next start: "Restore earlier data" asks for it and restarts.

import { useId, useState, type ReactNode } from "react";
import type { Backend } from "../api/backend";
import { storeRestore, type RestoreState, type StoreButton, type StoreRefusedData, type StoreRestoreData, type StoreScreen } from "../api/storeOpen";
import { useBackend, useRestoreAvailable } from "../api/queries";
import { DashButton, DashLogo } from "../components/basics";
import { Sheet } from "../components/sheet";
import { Model, useModel } from "../onboarding/model";
import { currentLanguage, t, useT, type Translate } from "../i18n";
import { acrossParagraphs, flatten } from "../i18n/core";
import { en } from "../i18n/en";
import { FailedLink, failedLinkUrl } from "./settings/About";
import { Problem } from "./settings/parts";

const ENGLISH = flatten(en);
const ISSUES_HINT = "settings.about.links.issuesHint";

type LooseTranslate = (key: string, values?: Record<string, string>) => string;

/** The catalog's text for a `storeUpgrade.*` key in the language in use; for any other key, or one the
 * catalog lacks, the engine's English: never other copy. */
function textFor(tr: Translate, key: string, english: string, folder: string): string {
  return inCatalog(key) ? (tr as unknown as LooseTranslate)(key, { folder }) : english;
}

const inCatalog = (key: string) => key.startsWith("storeUpgrade.") && ENGLISH.has(key);

/** Left-to-right isolate (U+2066 … U+2069), around English standing in for a right-to-left language. */
const LRI = "\u2066";
const PDI = "\u2069";

/** Lines joined as the engine joins a refusal's (`RefusedScreen::text`); `null` for none. */
const joined = (texts: readonly string[]): string | null => (texts.length === 0 ? null : `${texts[0]}: ${texts.slice(1).join(" ")}`);

/** A refusal's text in the language: its lines by their keys, joined as the engine joins them. A line the
 * catalog lacks keeps the engine's English, isolated left to right in a right-to-left language. The
 * engine's text, unchanged, when it sends no lines or lines that don't join to it. */
export function screenText(tr: Translate, screen: StoreScreen, folder: string, rtl: boolean): string {
  const lines = screen.lines ?? [];
  if (joined(lines.map((l) => l.text)) !== screen.text) return screen.text;
  const english = (text: string) => (rtl ? `${LRI}${acrossParagraphs(text, LRI, PDI)}${PDI}` : text);
  return joined(lines.map((l) => (inCatalog(l.key) ? textFor(tr, l.key, l.text, folder) : english(l.text)))) ?? screen.text;
}

export type ButtonAction = "quit" | "restart" | "reopen" | "keep" | "cancelRestore" | "report" | "restore";

const ACTIONS: Record<string, ButtonAction> = {
  "storeUpgrade.restore.foreign.keep": "keep",
  "storeUpgrade.refused.restore": "restore",
  "storeUpgrade.restore.refused.ok": "reopen",
  "storeUpgrade.restore.noSpace.ok": "reopen",
  "storeUpgrade.restore.notStarted.cancel": "cancelRestore",
  "storeUpgrade.restore.notStarted.restart": "restart",
  "storeUpgrade.restore.incomplete.restart": "restart",
  "storeUpgrade.restore.notStarted.report": "report",
  "storeUpgrade.restore.incomplete.report": "report",
  "storeUpgrade.refused.report": "report",
  "settings.about.links.issues": "report",
};

/** What each button key does. A key not here renders disabled: its action comes with the screen that
 * adds it. */
export function buttonAction(key: string): ButtonAction | null {
  return Object.hasOwn(ACTIONS, key) ? ACTIONS[key] : key.endsWith(".quit") ? "quit" : null;
}

/** The buttons of a restore state's screen, by their last key segment, in order; the first has the focus,
 * except on the foreign screen, where none has it (Keep is never the default). */
export const RESTORE_BUTTONS: Record<RestoreState, readonly string[]> = {
  refused: ["ok"],
  noSpace: ["ok"],
  notStarted: ["restart", "cancel", "report", "quit"],
  incomplete: ["restart", "report", "quit"],
  foreign: ["keep", "report", "quit"],
};

/** A restore screen's button key: `storeUpgrade.restore.<state>.<k>`; the foreign screen's Report a
 * problem is `settings.about.links.issues`, as `copy::restore_foreign::screen()` names it. */
export function restoreButtonKey(state: RestoreState, k: string): string {
  return state === "foreign" && k === "report" ? "settings.about.links.issues" : `storeUpgrade.restore.${state}.${k}`;
}

type ActionsApi = Pick<Backend, "quitApp" | "restartApp" | "keepCurrentFiles" | "cancelRestore" | "requestRestore" | "openProjectLink">;

/** What a failed action shows: a refused restore request in MG's words, an `Error`'s message, else the
 * generic line. */
function problemOf(e: unknown): string {
  if (storeRestore(e)?.state === "refused") return t("storeUpgrade.restore.refused.text");
  return e instanceof Error ? e.message : t("onboarding.error.generic");
}

/** The actions of the screens instead of the wallet, through dw-app, with the latest failure. A call
 * that opens the wallet data again (Keep the current files, Cancel restore) runs `reopen` once it is
 * done; a restore asked for restarts the app. */
export class OpenFailureActions extends Model {
  problem: string | null = null;
  /** The issues page a failed Report a problem names, to copy. */
  failedLink: string | null = null;
  running = false;
  /** RESTORE-REQUESTED is written and the restart that runs it has not happened yet. */
  requested = false;

  constructor(private readonly api: ActionsApi) {
    super();
  }

  quit = () => this.#run(() => this.api.quitApp());
  restart = () => this.#run(() => this.api.restartApp());
  keep = (reopen: () => void) => this.#then(() => this.api.keepCurrentFiles(), reopen);
  cancelRestore = (reopen: () => void) => this.#then(() => this.api.cancelRestore(), reopen);
  /** The confirm's Restart and restore: the request, then the restart that runs it; after a failed
   * restart, only the restart again. */
  restore = async () => {
    if (!this.requested) this.requested = await this.#run(() => this.api.requestRestore());
    if (this.requested) await this.#run(() => this.api.restartApp());
  };
  /** The confirm closed: a request whose restart failed is dropped, so closing it leaves nothing asked. */
  dismissRestore = async () => {
    if (this.requested && (await this.#run(() => this.api.cancelRestore()))) this.requested = false;
  };
  /** Clears the last failure, as a new step starts. */
  clear = () => {
    this.problem = null;
    this.failedLink = null;
    this.changed();
  };
  report = async () => {
    this.failedLink = null;
    await this.#run(
      () => this.api.openProjectLink("issues"),
      (e) => (this.failedLink = failedLinkUrl(e)) !== null,
    );
  };

  async #then(call: () => Promise<unknown>, after: () => void) {
    if (await this.#run(call)) after();
  }

  /** Runs `call`; `quiet` takes a failure it shows itself. */
  async #run(call: () => Promise<unknown>, quiet?: (e: unknown) => boolean): Promise<boolean> {
    this.problem = null;
    this.running = true;
    this.changed();
    try {
      await call();
      return true;
    } catch (e) {
      if (!quiet?.(e)) this.problem = problemOf(e);
      return false;
    } finally {
      this.running = false;
      this.changed();
    }
  }
}

function useActions() {
  const backend = useBackend();
  const [actions] = useState(() => new OpenFailureActions(backend));
  return useModel(actions);
}

/** Runs a screen button's action: what its click does. */
export function perform(actions: OpenFailureActions, action: ButtonAction, reopen: () => void) {
  if (action === "quit") void actions.quit();
  else if (action === "restart") void actions.restart();
  else if (action === "reopen") reopen();
  else if (action === "keep") void actions.keep(reopen);
  else if (action === "cancelRestore") void actions.cancelRestore(reopen);
  else if (action === "report") void actions.report();
}

function Panel({
  id,
  testId,
  title,
  text,
  problem,
  failedLink = null,
  children,
}: {
  id: string;
  testId: string;
  title: string;
  text: string;
  problem: string | null;
  failedLink?: string | null;
  children: ReactNode;
}) {
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
          <p className="onboarding-text store-open-text" id={`${id}-body`} data-testid={`${testId}-text`}>
            {text}
          </p>
          {children}
          <Problem text={problem} testId={`${testId}-problem`} />
          {failedLink && (
            <p className="form-problem" role="alert" data-testid={`${testId}-link-failed`}>
              <FailedLink url={failedLink} />
            </p>
          )}
        </main>
      </div>
    </div>
  );
}

/** A button as a screen shows it: the engine's button and what it does (`null`: disabled). */
type ShownButton = StoreButton & { action: ButtonAction | null };

function Buttons({ id, testId, buttons, hint, busy, onAction }: { id: string; testId: string; buttons: ShownButton[]; hint: string | null; busy: boolean; onAction: (action: ButtonAction) => void }) {
  return (
    <>
      <div className="onboarding-actions">
        {buttons.map((b) => (
          <DashButton
            key={b.key}
            variant={b.destructive ? "plainRed" : b.defaultFocus ? "filledBlue" : "plainBlue"}
            size="large"
            fill
            autoFocus={b.defaultFocus}
            disabled={b.action === null || (busy && b.action !== "quit")}
            aria-describedby={hint ? `${id}-hint` : undefined}
            data-testid={`${testId}-button-${b.key}`}
            onClick={b.action === null ? undefined : () => onAction(b.action!)}
          >
            {b.label}
          </DashButton>
        ))}
      </div>
      {hint && (
        <p id={`${id}-hint`} className="field-hint" data-testid={`${testId}-hint`}>
          {hint}
        </p>
      )}
    </>
  );
}

export interface StoreRefusalViewProps {
  refusal: StoreRefusedData;
  problem: string | null;
  failedLink?: string | null;
  /** A verified snapshot is there: "Restore earlier data" shows (only then). */
  restoreAvailable?: boolean;
  busy?: boolean;
  onAction: (action: ButtonAction) => void;
}

/** The engine's screen as it gives it: title, text, a button per its buttons, and its hint under them.
 * "Restore earlier data" shows only while a verified snapshot is there. */
export function StoreRefusalView({ refusal, problem, failedLink = null, restoreAvailable = false, busy = false, onAction }: StoreRefusalViewProps) {
  const tr = useT();
  const id = useId();
  const { screen, folder } = refusal;
  const buttons = screen.buttons
    .map((b) => ({ ...b, label: textFor(tr, b.key, b.label, folder), action: buttonAction(b.key) }))
    .filter((b) => b.action !== "restore" || restoreAvailable);
  const hint = screen.hint ? textFor(tr, screen.hint.key, screen.hint.text, folder) : null;
  // Report a problem always has the issues warning under it (MG); a screen whose one hint is another line gets it too.
  const warn = buttons.some((b) => b.action === "report") && screen.hint?.key !== ISSUES_HINT;
  return (
    <Panel id={id} testId="store-refused" title={textFor(tr, screen.titleKey, screen.title, folder)} text={screenText(tr, screen, folder, currentLanguage().dir === "rtl")} problem={problem} failedLink={failedLink}>
      <Buttons id={id} testId="store-refused" buttons={buttons} hint={hint} busy={busy} onAction={onAction} />
      {warn && (
        <p className="field-hint" data-testid="store-refused-issues-hint">
          {tr(ISSUES_HINT)}
        </p>
      )}
    </Panel>
  );
}

export interface StoreRestoreViewProps {
  restore: StoreRestoreData;
  problem: string | null;
  failedLink?: string | null;
  busy?: boolean;
  onAction: (action: ButtonAction) => void;
}

/** A restore that stopped the open, by its state: the same screen for every cause. Report a problem
 * has the issues warning under the buttons; the foreign screen has what Keep means above it. */
export function StoreRestoreView({ restore, problem, failedLink = null, busy = false, onAction }: StoreRestoreViewProps) {
  const tr = useT();
  const say = tr as unknown as LooseTranslate;
  const id = useId();
  const { state, folder } = restore;
  const keys = RESTORE_BUTTONS[state];
  const foreign = state === "foreign";
  const buttons = keys.map((k, i): ShownButton => {
    const key = restoreButtonKey(state, k);
    return { key, label: say(key), destructive: false, defaultFocus: !foreign && i === 0, action: buttonAction(key) };
  });
  const reports = keys.includes("report");
  const hint = foreign ? say("storeUpgrade.restore.foreign.keepMeaning", { folder }) : reports ? tr(ISSUES_HINT) : null;
  return (
    <Panel id={id} testId="store-restore" title={say(`storeUpgrade.restore.${state}.title`)} text={say(`storeUpgrade.restore.${state}.text`, { folder })} problem={problem} failedLink={failedLink}>
      <Buttons id={id} testId="store-restore" buttons={buttons} hint={hint} busy={busy} onAction={onAction} />
      {foreign && reports && (
        <p className="field-hint" data-testid="store-restore-issues-hint">
          {tr(ISSUES_HINT)}
        </p>
      )}
    </Panel>
  );
}

/** The retry screen's buttons, as `copy::retry_copy_for` gives them: Report a problem, then Quit. */
export const RETRY_BUTTONS = ["storeUpgrade.refused.report", "storeUpgrade.refused.quit"] as const;

export interface StoreRetryViewProps {
  problem: string | null;
  failedLink?: string | null;
  busy?: boolean;
  onAction: (action: ButtonAction) => void;
}

/** The open's store upgrade stopped on the machine (`storage_retry`): free up space, then quit and open
 * again; never the earlier version, as the file may be part-upgraded. The issues warning is under the
 * buttons. */
export function StoreRetryView({ problem, failedLink = null, busy = false, onAction }: StoreRetryViewProps) {
  const tr = useT();
  const say = tr as unknown as LooseTranslate;
  const id = useId();
  const buttons = RETRY_BUTTONS.map((key, i): ShownButton => ({ key, label: say(key), destructive: false, defaultFocus: i === 0, action: buttonAction(key) }));
  return (
    <Panel id={id} testId="store-retry" title={tr("storeUpgrade.retry.noSpace.title")} text={tr("storeUpgrade.retry.noSpace.text")} problem={problem} failedLink={failedLink}>
      <Buttons id={id} testId="store-retry" buttons={buttons} hint={tr(ISSUES_HINT)} busy={busy} onAction={onAction} />
    </Panel>
  );
}

export interface RestoreConfirmViewProps {
  folder: string;
  busy: boolean;
  problem: string | null;
  onConfirm: () => void;
  onClose: () => void;
}

/** R3's confirm, the only way to a restore: Cancel has the focus and writes nothing; Restart and restore
 * asks for the restore and restarts. */
export function RestoreConfirmView({ folder, busy, problem, onConfirm, onClose }: RestoreConfirmViewProps) {
  const tr = useT();
  return (
    <Sheet title={tr("storeUpgrade.restore.confirm.title")} width="medium" testId="restore-confirm" closable={!busy} onClose={onClose}>
      {tr("storeUpgrade.restore.confirm.text", { folder })
        .split("\n\n")
        .map((paragraph, i) => (
          <p key={i} className="sheet-text" data-testid="restore-confirm-text">
            {paragraph}
          </p>
        ))}
      <Problem text={problem} testId="restore-confirm-problem" />
      <div className="sheet-actions">
        <DashButton variant="tintedGray" data-autofocus disabled={busy} data-testid="restore-confirm-cancel" onClick={onClose}>
          {tr("storeUpgrade.restore.confirm.cancel")}
        </DashButton>
        <DashButton variant="plainRed" disabled={busy} aria-busy={busy} data-testid="restore-confirm-confirm" onClick={onConfirm}>
          {tr("storeUpgrade.restore.confirm.confirm")}
        </DashButton>
      </div>
    </Sheet>
  );
}

export interface OpenFailureScreenProps {
  /** Tries the open again: after Keep the current files, Cancel restore or OK. */
  onReopen: () => void;
  /** That open is running: the buttons wait for it. */
  retrying: boolean;
}

export function StoreRefusalScreen({ refusal, onReopen, retrying }: OpenFailureScreenProps & { refusal: StoreRefusedData }) {
  const exit = useActions();
  const offersRestore = refusal.screen.buttons.some((b) => buttonAction(b.key) === "restore");
  const restoreAvailable = useRestoreAvailable(offersRestore).data === true;
  const [confirming, setConfirming] = useState(false);
  const busy = exit.running || retrying;
  const confirm = () => {
    exit.clear();
    setConfirming(true);
  };
  const close = () => {
    setConfirming(false);
    void exit.dismissRestore();
  };
  return (
    <>
      <StoreRefusalView
        refusal={refusal}
        problem={confirming ? null : exit.problem}
        failedLink={exit.failedLink}
        restoreAvailable={restoreAvailable}
        busy={busy}
        onAction={(action) => (action === "restore" ? confirm() : perform(exit, action, onReopen))}
      />
      {confirming && <RestoreConfirmView folder={refusal.folder} busy={busy} problem={exit.problem} onConfirm={() => void exit.restore()} onClose={close} />}
    </>
  );
}

export function StoreRestoreScreen({ restore, onReopen, retrying }: OpenFailureScreenProps & { restore: StoreRestoreData }) {
  const exit = useActions();
  return <StoreRestoreView restore={restore} problem={exit.problem} failedLink={exit.failedLink} busy={exit.running || retrying} onAction={(action) => perform(exit, action, onReopen)} />;
}

export function StoreRetryScreen({ retrying }: { retrying: boolean }) {
  const exit = useActions();
  return <StoreRetryView problem={exit.problem} failedLink={exit.failedLink} busy={exit.running || retrying} onAction={(action) => perform(exit, action, () => {})} />;
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
