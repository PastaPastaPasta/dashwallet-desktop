// The CoinJoin surfaces' view model (U-06): one state per wallet for the Home card and the page
// (docs/design/U-06-coinjoin.md S1–S14), the mixing levels, Settings ▸ CoinJoin's checks, and the
// copy. Plain TypeScript over the generated types, no React; the reads are in queries.ts and the
// screens in screens/coinjoin. Copy: Android's CoinJoin screens (`strings-dashpay.xml`) for the
// mobile parts, dash-qt and Core verbatim for the rest (research 02 §9, the Swift app's
// `L10n+CoinJoin.swift`).

import { hasCode } from "../api/backend";
import type { CoinJoinBalances } from "../api/generated/CoinJoinBalances";
import type { CoinJoinLimits } from "../api/generated/CoinJoinLimits";
import type { CoinJoinPoolMessage } from "../api/generated/CoinJoinPoolMessage";
import type { CoinJoinPoolState } from "../api/generated/CoinJoinPoolState";
import type { CoinJoinSettings } from "../api/generated/CoinJoinSettings";
import type { CoinJoinStatus } from "../api/generated/CoinJoinStatus";
import type { CoinJoinStatusCode } from "../api/generated/CoinJoinStatusCode";
import type { LockStateName } from "../api/generated/LockStateName";
import { messageTable, t } from "../i18n";
import { en } from "../i18n/en";
import { failureText } from "../onboarding/model";

export const CoinJoinText = {
  get subtitle() {
    return t("coinjoin.subtitle");
  },
  get start() {
    return t("coinjoin.start");
  },
  get stop() {
    return t("coinjoin.stop");
  },
  get retry() {
    return t("coinjoin.retry");
  },
  get openSettings() {
    return t("coinjoin.openSettings");
  },
  get dismiss() {
    return t("coinjoin.dismiss");
  },
  get notStarted() {
    return t("coinjoin.notStarted");
  },
  get starting() {
    return t("coinjoin.starting");
  },
  get nextStep() {
    return t("coinjoin.nextStep");
  },
  get mixing() {
    return t("coinjoin.mixing");
  },
  get paused() {
    return t("coinjoin.paused");
  },
  get stopping() {
    return t("coinjoin.stopping");
  },
  get fullyMixed() {
    return t("coinjoin.fullyMixed");
  },
  get stopped() {
    return t("coinjoin.stopped");
  },
  get readFailed() {
    return t("coinjoin.readFailed");
  },
  get disabledTitle() {
    return t("coinjoin.disabledTitle");
  },
  get disabledLine() {
    return t("coinjoin.disabledLine");
  },
  get watchOnly() {
    return t("coinjoin.watchOnly");
  },
  get stoppedLocked() {
    return t("coinjoin.stoppedLocked");
  },
  get stoppedSession() {
    return t("coinjoin.stoppedSession");
  },
  get stoppedWallet() {
    return t("coinjoin.stoppedWallet");
  },
  minimumBalance: (amount: string) => t("coinjoin.minimumBalance", { amount }),
  mixedOf: (mixed: string, of: string) => t("coinjoin.mixedOf", { mixed, of }),
  get levelTitle() {
    return t("coinjoin.levelTitle");
  },
  get levelSubtitle() {
    return t("coinjoin.levelSubtitle");
  },
  get levelLive() {
    return t("coinjoin.levelLive");
  },
  custom: (rounds: number) => t("coinjoin.custom", { rounds }),
  get customHint() {
    return t("coinjoin.customHint");
  },
  get stopTitle() {
    return t("coinjoin.stopTitle");
  },
  get stopMessage() {
    return t("coinjoin.stopMessage");
  },
  get keepOpen() {
    return t("coinjoin.keepOpen");
  },
  get mixingOnly() {
    return t("coinjoin.mixingOnly");
  },
  get mixingOnlyHow() {
    return t("coinjoin.mixingOnlyHow");
  },
  autoLock: (after: string) => t("coinjoin.autoLock", { after }),
  get persists() {
    return t("coinjoin.persists");
  },
  get vaultLocked() {
    return t("coinjoin.vaultLocked");
  },
  get disabled() {
    return t("coinjoin.disabled");
  },
  get moveRunning() {
    return t("coinjoin.moveRunning");
  },
  // dash-qt's Options ▸ CoinJoin (QT-046, `optionsdialog.ui`).
  get enableFeatures() {
    return t("coinjoin.enableFeatures");
  },
  get enableFeaturesTip() {
    return t("coinjoin.enableFeaturesTip");
  },
  get mixingRounds() {
    return t("coinjoin.mixingRounds");
  },
  get mixingRoundsTip() {
    return t("coinjoin.mixingRoundsTip");
  },
  get targetBalance() {
    return t("coinjoin.targetBalance");
  },
  get targetBalanceTip() {
    return t("coinjoin.targetBalanceTip");
  },
  get multiSession() {
    return t("coinjoin.multiSession");
  },
  get multiSessionTip() {
    return t("coinjoin.multiSessionTip");
  },
  get parallelSessions() {
    return t("coinjoin.parallelSessions");
  },
  get parallelSessionsTip() {
    return t("coinjoin.parallelSessionsTip");
  },
  get inputsPerDenomination() {
    return t("coinjoin.inputsPerDenomination");
  },
  get inputsPerDenominationTip() {
    return t("coinjoin.inputsPerDenominationTip");
  },
  get denomsTarget() {
    return t("coinjoin.denomsTarget");
  },
  get denomsMaximum() {
    return t("coinjoin.denomsMaximum");
  },
  get restoreDefaults() {
    return t("coinjoin.restoreDefaults");
  },
  get restoreTitle() {
    return t("coinjoin.restoreTitle");
  },
  get restoreMessage() {
    return t("coinjoin.restoreMessage");
  },
  get saved() {
    return t("coinjoin.saved");
  },
  notSaved: (why: string) => t("coinjoin.notSaved", { why }),
  // QT-042's tooltip and the panel's rows, for Session details.
  get details() {
    return t("coinjoin.details");
  },
  get overallProgress() {
    return t("coinjoin.overallProgress");
  },
  get denominated() {
    return t("coinjoin.denominated");
  },
  get partiallyMixed() {
    return t("coinjoin.partiallyMixed");
  },
  get mixed() {
    return t("coinjoin.mixed");
  },
  averageRounds: (average: string, rounds: number) => t("coinjoin.averageRounds", { average, rounds }),
  get amountAndRounds() {
    return t("coinjoin.amountAndRounds");
  },
  roundsOf: (n: number) => t("coinjoin.roundsOf", { count: n }),
  get notEnoughInputs() {
    return t("coinjoin.notEnoughInputs");
  },
  get queueSize() {
    return t("coinjoin.queueSize");
  },
  get sessions() {
    return t("coinjoin.sessions");
  },
  get noSessions() {
    return t("coinjoin.noSessions");
  },
  get submittedDenominations() {
    return t("coinjoin.submittedDenominations");
  },
};

/** Core's session status line (`strAutoDenomResult` / `GetStatus`, research 02 §9.4). */
export function statusLine(code: CoinJoinStatusCode): string {
  switch (code.kind) {
    case "idle":
      return t("coinjoin.status.idle");
    case "syncInProgress":
      return t("coinjoin.status.syncInProgress");
    case "walletLocked":
      return t("coinjoin.status.walletLocked");
    case "mixingInProgress":
      return t("coinjoin.status.mixingInProgress");
    case "noMasternodes":
      return t("coinjoin.status.noMasternodes");
    case "notEnoughFunds":
      return t("coinjoin.status.notEnoughFunds");
    case "unconfirmedDenominated":
      return t("coinjoin.status.unconfirmedDenominated");
    case "noCompatibleMasternode":
      return t("coinjoin.status.noCompatibleMasternode");
    case "noCompatibleInputs":
      return t("coinjoin.status.noCompatibleInputs");
    case "tryingToConnect":
      return t("coinjoin.status.tryingToConnect");
    case "noQueueToJoin":
      return t("coinjoin.status.noQueueToJoin");
    case "noRandomMasternode":
      return t("coinjoin.status.noRandomMasternode");
    case "failedToStartQueue":
      return t("coinjoin.status.failedToStartQueue");
    case "waitingInQueue":
      return t("coinjoin.status.waitingInQueue");
    case "signing":
      return t("coinjoin.status.signing");
    case "masternode":
      return t("coinjoin.status.masternode", { message: poolMessageText(code.message) });
  }
}

/** Core `CoinJoin::GetMessageByID`. */
const POOL_MESSAGES: Readonly<Record<CoinJoinPoolMessage, string>> = messageTable(
  "coinjoin.poolMessage",
  Object.keys(en.coinjoin.poolMessage satisfies Record<CoinJoinPoolMessage, string>) as CoinJoinPoolMessage[],
);

export const poolMessageText = (m: CoinJoinPoolMessage) => POOL_MESSAGES[m];

export const POOL_STATE_TEXT: Readonly<Record<CoinJoinPoolState, string>> = messageTable(
  "coinjoin.poolState",
  Object.keys(en.coinjoin.poolState satisfies Record<CoinJoinPoolState, string>) as CoinJoinPoolState[],
);

/** D4: a running mixer that cannot move until sync, masternodes, funds or confirmations do. */
const WAITING = new Set<CoinJoinStatusCode["kind"]>([
  "syncInProgress",
  "noMasternodes",
  "noCompatibleMasternode",
  "noRandomMasternode",
  "notEnoughFunds",
  "noCompatibleInputs",
  "unconfirmedDenominated",
  "walletLocked",
]);

/** The design's S-rows. */
export type ViewState =
  | "loading"
  | "readFailed"
  | "stopping"
  | "starting"
  | "paused"
  | "finished"
  | "mixing"
  | "disabled"
  | "watchOnly"
  | "locked"
  | "error"
  | "tooLittle"
  | "done"
  | "off";

export type ViewActionKind = "start" | "stop" | "retry" | "settings";

export interface ViewAction {
  kind: ViewActionKind;
  label: string;
  disabled: boolean;
}

export interface CoinJoinView {
  state: ViewState;
  title: string;
  /** Under the title: Core's status line while mixing, why it stopped or cannot start. */
  line: string | null;
  /** `progress.overallPercent`, floored; `null` without a status. */
  percent: number | null;
  /** "X of Y tDASH" (D3), X at most Y; masked in discreet mode; `null` without a status. */
  mixed: string | null;
  action: ViewAction | null;
  /** The Home card shows it (D1). */
  onHome: boolean;
  /** A stop the user did not ask for: the Home card offers Dismiss (D1). */
  dismissable: boolean;
  /** Waiting for the engine (starting, stopping): a spinner. */
  busy: boolean;
}

export interface ViewInputs {
  status: CoinJoinStatus | undefined;
  /** The last status read failed (a refetch too: the cached answer may be wrong now). */
  readFailed: boolean;
  readError?: unknown;
  /** An amount with its unit ("1.2345 tDASH"). */
  amount: (duffs: number) => string;
  /** Discreet mode: amounts show as this instead. */
  masked: string | null;
  /** The wallet's total balance; undefined until read. */
  total?: number;
  /** A start this window saw (D16); absent if it never read the mixer stopped. */
  start?: SeenStart;
  /** `Date.now()` for `start`. */
  now: number;
}

/** How long a start this window saw can read as Starting… (D16): the engine's first step runs 5–15
 * s after Start (dw-coinjoin `AUTO_TIMEOUT_MIN/MAX`), and takes a few seconds. */
export const START_WINDOW_MS = 20_000;

/** A start this window saw (D16): the marks it last read with the mixer stopped, and when it first
 * read the mixer running after that (absent until then). */
export interface SeenStart {
  marks: MixerMarks;
  since?: number;
}

/** What a mixer's step moves (D16): its balances, and the wallet's total, which a funding
 * transaction's fee moves even when it spends and makes only coins the four balances leave out. */
export interface MixerMarks {
  balances: CoinJoinBalances;
  /** Absent when the balance had not been read by then: one read later is not the mixer's step. */
  total?: number;
}

const sameBalances = (a: CoinJoinBalances, b: CoinJoinBalances) =>
  a.anonymizable === b.anonymizable && a.denominated === b.denominated && a.normalizedAnonymized === b.normalizedAnonymized && a.fullyMixed === b.fullyMixed;

export const sameMarks = (a: MixerMarks, b: MixerMarks) => a.total === b.total && sameBalances(a.balances, b.balances);

/** Whether a step moved anything since `marks`: a total read only since then does not count. */
const movedSince = (marks: MixerMarks, now: MixerMarks) => !sameBalances(marks.balances, now.balances) || (marks.total !== undefined && marks.total !== now.total);

const actionLabel = (kind: ViewActionKind): string => {
  switch (kind) {
    case "start":
      return CoinJoinText.start;
    case "stop":
      return CoinJoinText.stop;
    case "retry":
      return CoinJoinText.retry;
    case "settings":
      return CoinJoinText.openSettings;
  }
};

const action = (kind: ViewActionKind, disabled = false): ViewAction => ({ kind, disabled, label: actionLabel(kind) });

/** S1–S14: the first row that matches wins. */
export function coinjoinView({ status, readFailed, readError, amount, masked, total, start, now }: ViewInputs): CoinJoinView {
  const base = { line: null, percent: null, mixed: null, onHome: false, dismissable: false, busy: false };
  if (readFailed) return { ...base, state: "readFailed", title: CoinJoinText.readFailed, line: failureText(readError), action: action("retry") };
  if (!status) return { ...base, state: "loading", title: "", action: null };
  const percent = Math.max(0, Math.min(100, Math.floor(status.progress.overallPercent)));
  const shown = (duffs: number) => masked ?? amount(duffs);
  // The target can be lowered below what is already mixed: X stops at Y, as the percentage does.
  const target = status.amountAndRounds.amount;
  const facts = { ...base, percent, mixed: CoinJoinText.mixedOf(shown(Math.min(status.balances.fullyMixed, target)), shown(target)) };
  const line = statusLine(status.status);
  const full = percent >= 100;
  const running = (state: ViewState, title: string, withLine: boolean, act = action("stop")): CoinJoinView => ({
    ...facts,
    state,
    title,
    line: withLine ? line : null,
    action: act,
    onHome: true,
    busy: state === "starting" || state === "stopping",
  });
  if (status.state === "stopping") return running("stopping", CoinJoinText.stopping, false, action("stop", true));
  if (status.state === "mixing") {
    // A fully mixed mixer waits for new coins: "Not enough funds to mix." below the target, idle
    // above it (dw-engine `auto_denominate` does not rebalance). Neither is a pause or a start.
    if (full) return running("finished", CoinJoinText.fullyMixed, status.status.kind !== "idle");
    // The engine reports `idle` before its first step, after a funding step, and while it waits
    // a block after one (also across Stop and Start): Starting… only for a start this window saw,
    // for a bounded time, while nothing has moved since it read the mixer stopped (D16).
    if (status.status.kind === "idle") {
      const moved = start !== undefined && movedSince(start.marks, { balances: status.balances, total });
      if (start && !moved && now - (start.since ?? now) < START_WINDOW_MS) return running("starting", CoinJoinText.starting, false);
      return { ...running("mixing", CoinJoinText.mixing, false), line: start && !moved ? CoinJoinText.nextStep : null };
    }
    if (WAITING.has(status.status.kind)) return running("paused", CoinJoinText.paused, true);
    return running("mixing", CoinJoinText.mixing, true);
  }
  const u = status.unavailable;
  if (u?.kind === "disabled") return { ...facts, state: "disabled", title: CoinJoinText.disabledTitle, line: CoinJoinText.disabledLine, action: action("settings") };
  if (u?.kind === "watchOnly") return { ...facts, state: "watchOnly", title: CoinJoinText.watchOnly, action: null };
  const stoppedBy = (state: ViewState, why: string): CoinJoinView => ({
    ...facts,
    state,
    title: CoinJoinText.stopped,
    line: why,
    action: action("start", u !== null),
    onHome: true,
    dismissable: true,
  });
  if (status.stopReason === "vaultLocked") return stoppedBy("locked", CoinJoinText.stoppedLocked);
  if (status.stopReason === "sessionClosed") return stoppedBy("error", CoinJoinText.stoppedSession);
  if (status.stopReason === "walletUnloaded") return stoppedBy("error", CoinJoinText.stoppedWallet);
  if (u?.kind === "insufficientFunds")
    return { ...facts, state: "tooLittle", title: CoinJoinText.notStarted, line: CoinJoinText.minimumBalance(amount(u.minDuffs)), action: action("start", true) };
  if (full) return { ...facts, state: "done", title: CoinJoinText.fullyMixed, action: action("start") };
  // After the user's own Stop the screen says so (0.1.x row 25); before any run, after a restart
  // and after a Settings turn-off it is Not Started. No line, not on Home: it is not news.
  return { ...facts, state: "off", title: status.stopReason === "userRequested" ? CoinJoinText.stopped : CoinJoinText.notStarted, action: action("start") };
}

/** What a screen reader hears when the view changes: the state, and why for a stop or a refusal.
 * A running mixer's status line moves up to once a second: it is shown, not announced. */
export function announcement(view: CoinJoinView): string {
  const running = view.onHome && !view.dismissable;
  return running || !view.line ? view.title : t("coinjoin.announcement", { title: view.title, line: view.line });
}

/** What a refused settings write says (D11): the engine's `invalid_argument` names the field. */
export function settingsFailureText(e: unknown): string {
  if (hasCode(e, "invalid_argument")) return CoinJoinText.notSaved(e.message);
  return failureText(e);
}

/** What a failed Start or Stop says (D9). */
export function actionFailureText(e: unknown, amount: (duffs: number) => string): string {
  if (hasCode(e, "coinjoin.vault_locked")) return CoinJoinText.vaultLocked;
  if (hasCode(e, "coinjoin.disabled")) return CoinJoinText.disabled;
  if (hasCode(e, "coinjoin.watch_only")) return CoinJoinText.watchOnly;
  if (hasCode(e, "coinjoin.move_running")) return CoinJoinText.moveRunning;
  if (hasCode(e, "coinjoin.insufficient_funds")) {
    const min = e.data?.kind === "limit" ? e.data.duffs : null;
    return min === null ? t("coinjoin.minimumBalanceUnknown") : CoinJoinText.minimumBalance(amount(min));
  }
  return failureText(e);
}

/** The lock state's line on the page (the design's "Lock and passphrase"). */
export function lockLine(lock: LockStateName | undefined): string | null {
  switch (lock) {
    case "unlockedMixingOnly":
      return CoinJoinText.mixingOnly;
    case "unlocked":
      return CoinJoinText.mixingOnlyHow;
    default:
      return null;
  }
}

// Mixing levels (D2): Android's `CoinJoinMode`, as engine rounds.

export type LevelId = "intermediate" | "advanced";

export interface Level {
  id: LevelId;
  title: string;
  rounds: number;
  description: string;
  time: string;
}

export const LEVELS: Level[] = [
  {
    id: "intermediate",
    get title() {
      return t("coinjoin.level.intermediate.title");
    },
    rounds: 4,
    get description() {
      return t("coinjoin.level.intermediate.description");
    },
    get time() {
      return t("coinjoin.level.intermediate.time");
    },
  },
  {
    id: "advanced",
    get title() {
      return t("coinjoin.level.advanced.title");
    },
    rounds: 8,
    get description() {
      return t("coinjoin.level.advanced.description");
    },
    get time() {
      return t("coinjoin.level.advanced.time");
    },
  },
];

/** The level `rounds` is, or `custom`. */
export const levelOf = (rounds: number): LevelId | "custom" => LEVELS.find((l) => l.rounds === rounds)?.id ?? "custom";

// Settings ▸ CoinJoin (D11).

/** The tab's number fields as typed. */
export interface SettingsForm {
  rounds: string;
  targetAmountDash: string;
  maxSessions: string;
  denomsGoal: string;
  denomsHardCap: string;
}

export type FormField = keyof SettingsForm;

export const formOf = (s: CoinJoinSettings): SettingsForm => ({
  rounds: String(s.rounds),
  targetAmountDash: String(s.targetAmountDash),
  maxSessions: String(s.maxSessions),
  denomsGoal: String(s.denomsGoal),
  denomsHardCap: String(s.denomsHardCap),
});

const fieldOutOfRangeText = (field: FormField, min: number, max: number): string => {
  switch (field) {
    case "rounds":
      return t("coinjoin.settingsCheck.rounds", { min, max });
    case "targetAmountDash":
      return t("coinjoin.settingsCheck.targetAmountDash", { min, max });
    case "maxSessions":
      return t("coinjoin.settingsCheck.maxSessions", { min, max });
    case "denomsGoal":
      return t("coinjoin.settingsCheck.denomsGoal", { min, max });
    case "denomsHardCap":
      return t("coinjoin.settingsCheck.denomsHardCap", { min, max });
  }
};

/** The form as settings over `current`, checked against the engine's ranges; else the first
 * field that is wrong and why. */
export function checkSettings(
  form: SettingsForm,
  current: CoinJoinSettings,
  limits: CoinJoinLimits,
): { ok: CoinJoinSettings } | { field: FormField; text: string } {
  const ranges: Record<FormField, [number, number]> = {
    rounds: [limits.minRounds, limits.maxRounds],
    targetAmountDash: [limits.minAmountDash, limits.maxAmountDash],
    maxSessions: [limits.minSessions, limits.maxSessions],
    denomsGoal: [limits.minDenoms, limits.maxDenoms],
    denomsHardCap: [limits.minDenoms, limits.maxDenoms],
  };
  const values = {} as Record<FormField, number>;
  for (const field of Object.keys(ranges) as FormField[]) {
    // Commas only as thousands separators ("2,500"; not "2,5,00").
    const typed = form[field].trim();
    const text = /^\d{1,3}(,\d{3})+$/.test(typed) ? typed.replaceAll(",", "") : typed;
    const [min, max] = ranges[field];
    const n = /^\d+$/.test(text) ? Number(text) : NaN;
    if (!Number.isSafeInteger(n) || n < min || n > max)
      return { field, text: fieldOutOfRangeText(field, min, max) };
    values[field] = n;
  }
  if (values.denomsGoal > values.denomsHardCap) return { field: "denomsGoal", text: t("coinjoin.settingsCheck.denomsGoalAboveMax") };
  return { ok: { ...current, ...values } };
}
