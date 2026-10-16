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
import { failureText } from "../onboarding/model";

export const CoinJoinText = {
  subtitle: "Mixing your Dash coins will make your transactions more private",
  start: "Start Mixing",
  stop: "Stop Mixing",
  retry: "Try again",
  openSettings: "Open Settings",
  dismiss: "Dismiss",
  notStarted: "Not Started",
  starting: "Starting…",
  nextStep: "Waiting for the next mixing step",
  mixing: "Mixing",
  paused: "Paused",
  stopping: "Stopping…",
  fullyMixed: "Fully Mixed",
  stopped: "Mixing stopped",
  readFailed: "Could not read the mixing status",
  disabledTitle: "CoinJoin is turned off",
  disabledLine: "Turn it on in Settings ▸ CoinJoin.",
  watchOnly: "Watch-only wallets cannot mix.",
  stoppedLocked: "Mixing stopped because the wallet was locked.",
  stoppedSession: "The network session closed.",
  stoppedWallet: "The wallet was closed.",
  minimumBalance: (amount: string) => `CoinJoin requires at least ${amount} to use.`,
  mixedOf: (mixed: string, of: string) => `${mixed} of ${of}`,
  levelTitle: "Select mixing level",
  levelSubtitle: "You can change or stop the mixing level at any time",
  levelLive: "A new level applies from the next mixing session.",
  custom: (rounds: number) => `Custom: ${rounds} rounds`,
  customHint: "Set in Settings ▸ CoinJoin.",
  stopTitle: "Are you sure you want to stop mixing?",
  stopMessage: "Coins mixed so far stay mixed. Open sessions end and their coins are released.",
  keepOpen: "Mixing runs while Dash Wallet is open and the wallet is unlocked, at least for mixing. Quitting Dash Wallet or locking the wallet stops it; start it again here.",
  mixingOnly: "Unlocked for mixing only: mixing runs, sending stays locked.",
  mixingOnlyHow: "To mix while sending stays locked, lock the wallet and unlock it with “For mixing only”.",
  autoLock: (after: string) => `Auto lock is on (${after}): when it locks the wallet, mixing stops.`,
  persists:
    "These settings are kept for this network and apply to every wallet. Mixing itself is not kept: it stops when Dash Wallet quits, the wallet closes or the wallet locks.",
  vaultLocked: "Unlock the wallet, at least for mixing, and try again.",
  disabled: "CoinJoin is turned off in Settings ▸ CoinJoin.",
  // dash-qt's Options ▸ CoinJoin (QT-046, `optionsdialog.ui`).
  enableFeatures: "Enable CoinJoin features",
  enableFeaturesTip: "Turning this off stops mixing in every wallet.",
  mixingRounds: "Mixing rounds",
  mixingRoundsTip:
    "This setting determines the amount of individual masternodes that an input will be mixed through. More rounds of mixing gives a higher degree of privacy, but also costs more in fees.",
  targetBalance: "Target balance (DASH)",
  targetBalanceTip: "This amount acts as a threshold to turn off mixing once it's reached.",
  multiSession: "Enable multi-session",
  multiSessionTip: "Whether to use experimental mode with multiple mixing sessions per block. Note: You must use this feature carefully.",
  parallelSessions: "Parallel sessions",
  parallelSessionsTip: "Use this many separate masternodes in parallel to mix funds.",
  inputsPerDenomination: "Inputs per denomination",
  inputsPerDenominationTip: "How many inputs of each denominated amount are created. Lower these numbers if you want fewer smaller denominations.",
  denomsTarget: "Target",
  denomsMaximum: "Maximum",
  restoreDefaults: "Restore defaults…",
  restoreTitle: "Restore the CoinJoin defaults?",
  restoreMessage: "Every value on this tab goes back to Dash Core's default. Mixing that runs keeps running.",
  saved: "Saved.",
  notSaved: (why: string) => `The settings were not saved: ${why}`,
  // QT-042's tooltip and the panel's rows, for Session details.
  details: "Session details",
  overallProgress: "Overall progress",
  denominated: "Denominated",
  partiallyMixed: "Partially mixed",
  mixed: "Mixed",
  averageRounds: (average: string, rounds: number) => `Denominated inputs have ${average} of ${rounds} rounds on average`,
  amountAndRounds: "Amount and Rounds",
  roundsOf: (n: number) => (n === 1 ? `${n} Round` : `${n} Rounds`),
  notEnoughInputs: "Not enough compatible inputs to mix the target amount.",
  queueSize: "Queue size",
  sessions: "Sessions",
  noSessions: "No open sessions",
  submittedDenominations: "Submitted Denom",
} as const;

/** Core's session status line (`strAutoDenomResult` / `GetStatus`, research 02 §9.4). */
export function statusLine(code: CoinJoinStatusCode): string {
  switch (code.kind) {
    case "idle":
      return "CoinJoin is idle.";
    case "syncInProgress":
      return "Can't mix while sync in progress.";
    case "walletLocked":
      return "Wallet is locked.";
    case "mixingInProgress":
      return "Mixing in progress…";
    case "noMasternodes":
      return "No Masternodes detected.";
    case "notEnoughFunds":
      return "Not enough funds to mix.";
    case "unconfirmedDenominated":
      return "Found unconfirmed denominated outputs, will wait till they confirm to continue.";
    case "noCompatibleMasternode":
      return "No compatible Masternode found.";
    case "noCompatibleInputs":
      return "Can't mix: no compatible inputs found!";
    case "tryingToConnect":
      return "Trying to connect…";
    case "noQueueToJoin":
      return "Failed to find mixing queue to join";
    case "noRandomMasternode":
      return "Can't find random Masternode.";
    case "failedToStartQueue":
      return "Failed to start a new mixing queue";
    case "waitingInQueue":
      return "Submitted to masternode, waiting in queue .";
    case "signing":
      return "Found enough users, signing…";
    case "masternode":
      return `Masternode: ${poolMessageText(code.message)}`;
  }
}

/** Core `CoinJoin::GetMessageByID`. */
const POOL_MESSAGES: Record<CoinJoinPoolMessage, string> = {
  alreadyHave: "Already have that input.",
  denom: "No matching denominations found for mixing.",
  entriesFull: "Entries are full.",
  existingTx: "Not compatible with existing transactions.",
  fees: "Transaction fees are too high.",
  invalidCollateral: "Collateral not valid.",
  invalidInput: "Input is not valid.",
  invalidScript: "Invalid script detected.",
  invalidTx: "Transaction not valid.",
  maximum: "Entry exceeds maximum size.",
  mnList: "Not in the Masternode list.",
  mode: "Incompatible mode.",
  queueFull: "Masternode queue is full.",
  recent: "Last queue was created too recently.",
  session: "Session not complete!",
  missingTx: "Missing input transaction information.",
  version: "Incompatible version.",
  noError: "No errors detected.",
  success: "Transaction created successfully.",
  entriesAdded: "Your entries added successfully.",
  sizeMismatch: "Inputs vs outputs size mismatch.",
  nonStandardPubkey: "Unknown response.",
  notAMasternode: "Unknown response.",
};

export const poolMessageText = (m: CoinJoinPoolMessage) => POOL_MESSAGES[m];

export const POOL_STATE_TEXT: Record<CoinJoinPoolState, string> = {
  idle: "Idle",
  queue: "Queued",
  acceptingEntries: "Accepting entries",
  signing: "Signing",
  error: "Error",
};

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

const ACTION_LABEL: Record<ViewActionKind, string> = {
  start: CoinJoinText.start,
  stop: CoinJoinText.stop,
  retry: CoinJoinText.retry,
  settings: CoinJoinText.openSettings,
};

const action = (kind: ViewActionKind, disabled = false): ViewAction => ({ kind, disabled, label: ACTION_LABEL[kind] });

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
  return { ...facts, state: "off", title: CoinJoinText.notStarted, action: action("start") };
}

/** What a screen reader hears when the view changes: the state, and why for a stop or a refusal.
 * A running mixer's status line moves up to once a second: it is shown, not announced. */
export function announcement(view: CoinJoinView): string {
  const running = view.onHome && !view.dismissable;
  return running || !view.line ? view.title : `${view.title}. ${view.line}`;
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
  if (hasCode(e, "coinjoin.insufficient_funds")) {
    const min = e.data?.kind === "limit" ? e.data.duffs : null;
    return CoinJoinText.minimumBalance(min === null ? "the minimum balance" : amount(min));
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
    title: "Intermediate",
    rounds: 4,
    description: "Advanced users who have a very high level of technical expertise can determine your transaction history",
    time: "up to 30 minutes",
  },
  {
    id: "advanced",
    title: "Advanced",
    rounds: 8,
    description: "It would be very difficult for advanced users with any level of technical expertise to determine your transaction history",
    time: "Multiple hours",
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

const LABELS: Record<FormField, string> = {
  rounds: CoinJoinText.mixingRounds,
  targetAmountDash: "Target balance",
  maxSessions: CoinJoinText.parallelSessions,
  denomsGoal: `${CoinJoinText.inputsPerDenomination}: ${CoinJoinText.denomsTarget}`,
  denomsHardCap: `${CoinJoinText.inputsPerDenomination}: ${CoinJoinText.denomsMaximum}`,
};

const fmt = (n: number) => n.toLocaleString("en-US");

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
      return { field, text: `${LABELS[field]} must be a whole number from ${fmt(min)} to ${fmt(max)}.` };
    values[field] = n;
  }
  if (values.denomsGoal > values.denomsHardCap) return { field: "denomsGoal", text: "The target number of inputs cannot be above the maximum." };
  return { ok: { ...current, ...values } };
}
