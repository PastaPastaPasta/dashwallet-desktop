// The registration wizard's view model (DASHPAY §4 `RegistrationViewModel`, F2, F4; DP1-09): the
// name step's checklist and availability, the confirm step's cost, and the progress and result
// screens for a registration's row (the design's R1–R11). Plain TypeScript over the generated
// types, no React; the flow's own state is wizard.ts, the screens screens/dashpay/Registration.tsx.
// The step and wait copy is DP1-08's (status.ts), so Home, the chip and the wizard agree (D11).
// Copy follows Android `strings-dashpay.xml` (v11.9.1), adapted where the desktop differs.

import { hasCode, isAppError } from "../api/backend";
import type { NameAvailability } from "../api/generated/NameAvailability";
import type { FinishReport } from "../api/generated/FinishReport";
import type { NameOutcome } from "../api/generated/NameOutcome";
import type { RegistrationQuote } from "../api/generated/RegistrationQuote";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import type { UsernameCheck } from "../api/generated/UsernameCheck";
import type { UsernameRuleName } from "../api/generated/UsernameRuleName";
import { waitText } from "../onboarding/model";
import { awaitsContinue, COPY, failureText, stepOf, WAITS, type CardStep } from "./status";

/** The engine's cap (`check_username`'s `maxLength`); the field takes no more. */
export const MAX_LABEL = 23;

/** F4: `name_availability` waits for the typing to settle. */
export const AVAILABILITY_DEBOUNCE_MS = 400;

export const REG_COPY = {
  header: (n: 1 | 2, title: string) => `Step ${n} of 2 · ${title}`,
  name: {
    step: "Choose a username",
    title: "Choose your Dash username",
    field: "Username",
    contested:
      "The Dash network must vote to approve this username before it is created. Usernames with a number 2–9, or 20 characters or more, are approved at once.",
    votingLink: "What is username voting?",
    next: "Continue",
    cancel: "Cancel",
  },
  confirm: {
    step: "Confirm",
    title: "Confirm your username",
    message: (label: string) => `You have chosen @${label} as your username. Please note that the username can NOT be changed once it is registered.`,
    fee: "Upgrade fee",
    credits: "Part of it becomes credits that pay for contacts and profile changes.",
    contested:
      "The network votes on this username for about two weeks. If it is not approved, the payment is not returned; your identity keeps its credits and you can choose another username.",
    quoting: "Working out the cost…",
    quoteFailed: "Could not work out the cost.",
    shortfall: (amount: string) => `You need at least ${amount} to create this username.`,
    fromCredits: (credits: string) => `Paid from your identity's credits: up to ${BigInt(credits).toLocaleString("en-US")} credits`,
    accept: "I accept",
    pay: "Confirm & pay",
    register: "Confirm",
    back: "Back",
  },
  progress: {
    title: "Creating your username",
    message: (label: string) => `Your username @${label} is being created on the Dash network. It may take some time.`,
    close: "You can close this page or the app: the registration goes on, and Home shows its progress.",
    home: "Let me know when it's done",
    readFailed: "Could not read the registration's progress. It goes on; Home shows it.",
    authorize: (label: string) => `Paused. Confirm to finish creating @${label}.`,
    unlock: (label: string) => `Unlock the wallet to finish creating @${label}.`,
    continue: COPY.continue,
    unlockAction: COPY.unlock,
    finish: "Finish transfers",
    repair: COPY.repair.action,
  },
  recovery:
    "Your funds are safe. The payment is locked for this registration, and Dash Wallet finishes it once the network confirms it. If it does not finish, Tools ▸ Repair can finish the transfer.",
  failed: { title: "Registration failed", another: "Choose another username" },
  stopped: { title: "Registration stopped" },
  done: { title: (label: string) => `Hello @${label},`, message: "Your account is ready", action: "Done" },
  nameFailed: {
    title: "Your identity is ready",
    message: (label: string, why: string) => `@${label} could not be registered. ${why}`,
    another: "Choose another username",
  },
  voting: {
    title: "Voting for your username has begun",
    message: (label: string) => `Your username @${label} has been requested on the Dash network.`,
    action: "Done",
    info: "What is username voting?",
  },
  wedge: {
    title: "Start a new registration?",
    // m4 §2.2 "The wedge" (DEC-197), verbatim.
    text: "An earlier registration's payment could not be confirmed, and it may still appear on the network. You can start a new registration now. The earlier payment may still confirm. If it does, its funds show up as a transfer this wallet can recover later with Tools ▸ Repair ▸ Finish transfers, as a separate identity, so you may have paid for two registrations. The earlier registration can't be discarded.",
    confirm: "Start a new registration",
    cancel: "Cancel",
  },
  finished: (r: FinishReport) => `${r.resumed} resumed, ${r.completed} completed, ${r.stillPending} still pending.`,
} as const;

// ---------------------------------------------------------------------------------------------
// Step 1: the name (IOS-066, F4)

/** Android's checklist rows, plus the engine's double-hyphen rule. */
export const RULE_TEXT: Record<UsernameRuleName, string> = {
  minLength: "At least 3 characters",
  maxLength: "At most 23 characters",
  allowedCharacters: "Letters, numbers and hyphens only",
  noEdgeHyphen: "Must start and end with a letter or number",
  noDoubleHyphen: "No two hyphens in a row",
};

export type AvailabilityTone = "pending" | "ok" | "problem" | "neutral";

export interface AvailabilityLine {
  text: string;
  tone: AvailabilityTone;
}

/** The line under the checklist: `null` until the label is valid. `pending` while the typing has
 * not settled or the answer is not in. */
export function availabilityLine(check: UsernameCheck | undefined, availability: NameAvailability | undefined, pending: boolean): AvailabilityLine | null {
  if (!check?.valid) return null;
  if (pending || !availability) return { text: "Checking…", tone: "pending" };
  switch (availability.kind) {
    case "available":
      return { text: "Username available", tone: "ok" };
    case "contestOpen":
      return {
        text: "This username has already been requested, but you can request it too and let the network vote to decide if you can have it.",
        tone: "neutral",
      };
    case "taken":
      return { text: "Username taken", tone: "problem" };
    case "locked":
      return { text: "This username is blocked", tone: "problem" };
    case "invalid":
      return { text: "This username is not valid", tone: "problem" };
    case "unknown":
      return { text: "Could not check availability. You can still continue.", tone: "neutral" };
  }
}

/** D3: Continue on a valid label the network did not refuse. */
export function canChoose(check: UsernameCheck | undefined, availability: NameAvailability | undefined, pending: boolean): boolean {
  if (!check?.valid || pending || !availability) return false;
  return availability.kind === "available" || availability.kind === "contestOpen" || availability.kind === "unknown";
}

// ---------------------------------------------------------------------------------------------
// Step 2: the cost (IOS-067)

export interface ConfirmModel {
  /** The fee line's amount: the quote's total (lock and fee). */
  cost: string;
  lines: string[];
  /** D5: the quote's total above what the funding can spend now. */
  shortfall: string | null;
}

export function confirmModel(quote: RegistrationQuote, amount: (duffs: number) => string): ConfirmModel {
  const lines: string[] = [REG_COPY.confirm.credits];
  if (quote.contested) lines.push(REG_COPY.confirm.contested);
  return {
    cost: amount(quote.totalDuffs),
    lines,
    shortfall: quote.availableDuffs < quote.totalDuffs ? REG_COPY.confirm.shortfall(amount(quote.totalDuffs)) : null,
  };
}

// ---------------------------------------------------------------------------------------------
// Progress and results (R1–R11)

/** The design's screens for a row. */
export type ScreenKind =
  /** R1: running. */
  | "progress"
  /** R2: idle, waiting for a grant. */
  | "paused"
  /** R3: waiting for the full unlock. */
  | "unlock"
  /** R4, R5: the funding's outcome is unknown, or a lock waits for Finish transfers. */
  | "repair"
  /** R6: failed, retryable. */
  | "retry"
  /** R7: failed for good, nothing committed. */
  | "failed"
  /** R8: failed for good with funds committed. */
  | "stopped"
  /** R9 */
  | "done"
  /** R10: the identity exists, its name was refused. */
  | "nameFailed"
  /** R11 */
  | "voting";

export type ScreenActionKind = "home" | "continue" | "unlock" | "finish" | "repair" | "another" | "done" | "votingInfo";

export interface ScreenAction {
  kind: ScreenActionKind;
  label: string;
}

export interface ScreenModel {
  kind: ScreenKind;
  label: string;
  title: string;
  lines: string[];
  /** The three steps, while the registration is under way. */
  step: CardStep | null;
  busy: boolean;
  /** The first is the primary action. */
  actions: ScreenAction[];
}

export interface ScreenInputs {
  /** A contest for the row's label is open (the status's `contestPending`). */
  contestPending: boolean;
  /** When the contest ends (UNIX s), if known. */
  contestEndsAt: number | null;
  date: (secs: number) => string;
  amount: (duffs: number) => string;
  discreet: boolean;
}

const act = (kind: ScreenActionKind, label: string): ScreenAction => ({ kind, label });

/** R10: why a name step refused for good (m4 §2.2 "Settled after the identity"). */
export function nameFailureText(code: string): string {
  switch (code) {
    case "name.taken":
      return "Username was taken by another user.";
    case "name.contest_open":
      return "Another user requested it, and the network is voting on it.";
    case "name.locked":
      return "The network blocked this username.";
    case "name.invalid":
    case "name.unavailable_for_invite":
      return "This username is not valid.";
    case "platform.insufficient_credits":
    case "platform.grant_exceeded":
      return "The identity's credits did not cover it.";
    default:
      return `It stopped with ${code}.`;
  }
}

/** The screen for `row` (the design's R1–R11). */
export function screenModel(row: RegistrationStatus, i: ScreenInputs): ScreenModel {
  const { label } = row;
  const step = stepOf(row.phase, row.failure);
  const progressTitle = REG_COPY.progress.title;
  const base = (kind: ScreenKind, title: string, lines: string[], actions: ScreenAction[], rest: Partial<ScreenModel> = {}): ScreenModel => ({
    kind,
    label,
    title,
    lines,
    step,
    busy: false,
    actions,
    ...rest,
  });
  if (row.phase === "done") {
    if (row.nameFailure)
      return base("nameFailed", REG_COPY.nameFailed.title, [REG_COPY.nameFailed.message(label, nameFailureText(row.nameFailure))], [act("another", REG_COPY.nameFailed.another)], {
        step: null,
      });
    // A contested label ends `done` too; its contest is the status's (D4).
    return i.contestPending ? votingScreen(label, i.contestEndsAt, i.date) : doneScreen(label);
  }
  if (row.phase === "contested") return votingScreen(label, row.contestEndsAt ?? i.contestEndsAt, i.date);

  // Unknown funding first: a failed row may still wait on it (m4 §2.2 `waiting`).
  if (row.waiting === "fundingUnknown")
    return base("repair", progressTitle, [COPY.repair.unknown, REG_COPY.recovery], [act("repair", REG_COPY.progress.repair)]);
  if (row.waiting === "needsFinish")
    return base("repair", progressTitle, [COPY.repair.finish, REG_COPY.recovery], [act("finish", REG_COPY.progress.finish), act("repair", REG_COPY.progress.repair)]);

  const f = row.failure;
  if (f) {
    const why = failureText(f, i.amount, i.discreet);
    const recovery = row.fundsCommitted ? [REG_COPY.recovery] : [];
    if (f.retryable) {
      // D7: a funded row is never discarded from here.
      const actions = [act("continue", REG_COPY.progress.continue)];
      if (!row.fundsCommitted) actions.push(act("another", REG_COPY.failed.another));
      return base("retry", progressTitle, [COPY.paused, why, ...recovery], actions);
    }
    // D27: the engine may take a new start past it (its lock settled), so a new name is offered;
    // a refusal shows this row again (`registration.in_progress`).
    if (row.fundsCommitted)
      return base("stopped", REG_COPY.stopped.title, [why, REG_COPY.recovery], [act("repair", REG_COPY.progress.repair), act("another", REG_COPY.failed.another)]);
    return base("failed", REG_COPY.failed.title, [why], [act("another", REG_COPY.failed.another)], { step: null });
  }

  if (awaitsContinue(row)) return base("paused", progressTitle, [REG_COPY.progress.authorize(label)], [act("continue", REG_COPY.progress.continue)]);
  if (row.waiting === "unlock") return base("unlock", progressTitle, [REG_COPY.progress.unlock(label)], [act("unlock", REG_COPY.progress.unlockAction)]);

  const wait = row.waiting ? WAITS[row.waiting] : null;
  return base("progress", progressTitle, [REG_COPY.progress.message(label), ...(wait ? [wait] : []), REG_COPY.progress.close], [act("home", REG_COPY.progress.home)], {
    busy: true,
  });
}

/** D23: the page's row while a draft this session started is not read yet, or when the read
 * failed (review r2 L2). */
export function pendingModel(readFailed = false): ScreenModel {
  return {
    kind: "progress",
    label: "",
    title: REG_COPY.progress.title,
    lines: readFailed ? [REG_COPY.progress.readFailed] : [],
    step: null,
    busy: !readFailed,
    actions: [act("home", REG_COPY.progress.home)],
  };
}

/** The result of a name registered from credits (D6): R9, or R11 once its contest started. */
export function namedModel(label: string, outcome: NameOutcome, date: (secs: number) => string): ScreenModel {
  return outcome.kind === "contestStarted" ? votingScreen(label, outcome.endsAt, date) : doneScreen(label);
}

/** R11: the vote on `label` began. */
function votingScreen(label: string, endsAt: number | null, date: (secs: number) => string): ScreenModel {
  return {
    kind: "voting",
    label,
    title: REG_COPY.voting.title,
    lines: [REG_COPY.voting.message(label), endsAt === null ? COPY.voting.notify : COPY.voting.endsOn(date(endsAt))],
    step: null,
    busy: false,
    actions: [act("done", REG_COPY.voting.action), act("votingInfo", REG_COPY.voting.info)],
  };
}

/** R9: Hello @label. */
function doneScreen(label: string): ScreenModel {
  return { kind: "done", label, title: REG_COPY.done.title(label), lines: [REG_COPY.done.message], step: null, busy: false, actions: [act("done", REG_COPY.done.action)] };
}

/** A row a run drives: the progress screen re-reads it on a timer (D15). */
export function isRunning(row: RegistrationStatus | undefined): boolean {
  if (!row || row.failure || row.phase === "done" || row.phase === "contested" || row.phase === "failed" || awaitsContinue(row)) return false;
  return row.waiting !== "unlock" && row.waiting !== "fundingUnknown" && row.waiting !== "needsFinish";
}

// ---------------------------------------------------------------------------------------------
// Refusals of a start, a resume and a name

/** Why a start, resume or name registration was refused, as the wizard says it. */
export function startErrorText(e: unknown, amount: (duffs: number) => string, discreet = false): string {
  if (hasCode(e, "registration.funding_insufficient") && e.data?.kind === "duffs") {
    const { needed, available } = e.data;
    if (needed === null) return "Not enough Dash to pay for the username.";
    return discreet
      ? `Not enough Dash to pay for the username: ${amount(needed)} needed.`
      : `Not enough Dash to pay for the username: ${amount(needed)} needed, ${amount(available)} available.`;
  }
  if (hasCode(e, "registration.coins_locked")) return "The coins to pay with are held by another payment. Try again once it ends.";
  if (hasCode(e, "registration.not_synced")) return "Wait until the wallet has finished syncing, then try again.";
  if (hasCode(e, "registration.funding_unknown_recent")) {
    const secs = e.data?.kind === "draftWait" ? e.data.waitSecs : null;
    // m4 §2.2, verbatim.
    return secs === null
      ? "This wallet can't yet tell when the earlier registration's payment was sent. Try again later."
      : `The earlier registration's payment was sent less than an hour ago. Try again in ${waitText(secs)}.`;
  }
  if (hasCode(e, "registration.already_has_username")) return "This wallet already has a username.";
  if (hasCode(e, "platform.grant_exceeded")) return "The cost went above what you approved. Try again.";
  if (hasCode(e, "platform.insufficient_credits")) return "The identity's credits do not cover this username.";
  if (hasCode(e, "name.taken")) return "Username taken. Choose another one.";
  if (hasCode(e, "name.locked")) return "This username is blocked. Choose another one.";
  if (hasCode(e, "name.invalid")) return "This username is not valid.";
  if (hasCode(e, "name.contest_open")) return "The network is already voting on this username, and it can no longer be joined.";
  if (hasCode(e, "platform.unavailable") || hasCode(e, "platform.timeout")) return "Could not reach the Dash network. Try again.";
  if (hasCode(e, "platform.context_unavailable")) return "Wait until the wallet has finished syncing, then try again.";
  if (hasCode(e, "vault.locked") || hasCode(e, "vault.mixing_only") || hasCode(e, "platform.signer_unavailable")) return "Unlock the wallet and try again.";
  return isAppError(e) || e instanceof Error ? e.message : String(e);
}
