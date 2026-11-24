// The registration wizard's view model (DASHPAY §4 `RegistrationViewModel`, F2, F4; DP1-09): the
// name step's checklist and availability, the confirm step's cost, and the progress and result
// screens for a registration's row (the design's R1–R11). Plain TypeScript over the generated
// types, no React; the flow's own state is wizard.ts, the screens screens/dashpay/Registration.tsx.
// The step and wait copy is DP1-08's (status.ts), so Home, the chip and the wizard agree (D11).
// Copy follows Android `strings-dashpay.xml` (v11.9.1), adapted where the desktop differs.

import { hasCode, isAppError } from "../api/backend";
import { FLOW_NAME } from "../api/lockReport";
import type { NameAvailability } from "../api/generated/NameAvailability";
import type { FinishReport } from "../api/generated/FinishReport";
import type { LockKind } from "../api/generated/LockKind";
import type { NameOutcome } from "../api/generated/NameOutcome";
import type { RegistrationQuote } from "../api/generated/RegistrationQuote";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import type { UsernameCheck } from "../api/generated/UsernameCheck";
import type { UsernameRuleName } from "../api/generated/UsernameRuleName";
import { waitText } from "../onboarding/model";
import { formatLocale, messageTable, t } from "../i18n";
import { listed } from "../network/choice";
import { awaitsContinue, COPY, failureText, heldWithBoth, stepOf, WAITS, withTable, type CardStep } from "./status";

/** The engine's cap (`check_username`'s `maxLength`); the field takes no more. */
export const MAX_LABEL = 23;

/** F4: `name_availability` waits for the typing to settle. */
export const AVAILABILITY_DEBOUNCE_MS = 400;

/** The kinds whose transfers couldn't be read, `""` for none. */
export const unreadText = (kinds: readonly LockKind[]): string =>
  kinds.length > 0 ? t("dashpay.finished.unread", { kinds: listed(kinds.map((k) => t(`dashpay.finished.kind.${k}`))) }) : "";

const kindName = (kind: LockKind) => (kind === "shielded" ? t("dashpay.finished.kind.shielded") : FLOW_NAME.platformAddressFund);

/** Each kind's part of a report that has an "Add from Dash wallet" row, so the two are told apart. */
const byKindText = (r: FinishReport): string[] =>
  r.byKind.some((k) => k.kind === "platformAddress")
    ? r.byKind.map((k) =>
        k.stillPending > 0
          ? t("dashpay.finished.byKindPending", { kind: kindName(k.kind), completed: k.completed, stillPending: k.stillPending })
          : t("dashpay.finished.byKind", { kind: kindName(k.kind), completed: k.completed }),
      )
    : [];

/** Finish transfers' report. */
const finishedText = (r: FinishReport): string => {
  const counts =
    r.alreadyUsed > 0
      ? t("dashpay.finished.countsUsed", { resumed: r.resumed, completed: r.completed, stillPending: r.stillPending, alreadyUsed: r.alreadyUsed })
      : t("dashpay.finished.counts", { resumed: r.resumed, completed: r.completed, stillPending: r.stillPending });
  return [counts, ...byKindText(r), ...(r.unread.length > 0 ? [unreadText(r.unread)] : [])].join(" ");
};

const reg = <K extends string>(prefix: string, keys: readonly K[]) => messageTable(`dashpay.registration.${prefix}`, keys);
const username = (label: string) => ({ username: `@${label}` });

export const REG_COPY = {
  header: (n: 1 | 2, title: string) => t("dashpay.registration.header", { n, title }),
  name: reg("name", ["step", "title", "field", "contested", "votingLink", "instantTitle", "instantOnly", "next", "cancel"]),
  confirm: withTable(reg("confirm", ["step", "title", "fee", "credits", "contested", "quoting", "quoteFailed", "accept", "pay", "register", "back"]), {
    message: (label: string) => t("dashpay.registration.confirm.message", username(label)),
    shortfall: (amount: string) => t("dashpay.registration.confirm.shortfall", { amount }),
    fromCredits: (credits: string) => t("dashpay.registration.confirm.fromCredits", { credits: new Intl.NumberFormat(formatLocale(undefined)).format(BigInt(credits)) }),
  }),
  progress: withTable(reg("progress", ["title", "close", "home", "readFailed", "finish"]), {
    message: (label: string) => t("dashpay.registration.progress.message", username(label)),
    authorize: (label: string) => t("dashpay.registration.progress.authorize", username(label)),
    unlock: (label: string) => t("dashpay.registration.progress.unlock", username(label)),
    get continue() {
      return COPY.continue;
    },
    get moveToCore() {
      return COPY.moveToCore;
    },
    get unlockAction() {
      return COPY.unlock;
    },
    get repair() {
      return COPY.repair.action;
    },
  }),
  get recovery() {
    return t("dashpay.registration.recovery");
  },
  failed: reg("failed", ["title", "another"]),
  stopped: reg("stopped", ["title"]),
  done: withTable(reg("done", ["message", "next", "action", "requestSent", "requestRefused"]), {
    title: (label: string) => t("dashpay.registration.done.title", username(label)),
  }),
  nameFailed: withTable(reg("nameFailed", ["title", "another"]), {
    message: (label: string, why: string) => t("dashpay.registration.nameFailed.message", { ...username(label), why }),
  }),
  voting: withTable(reg("voting", ["title", "action", "info"]), {
    message: (label: string) => t("dashpay.registration.voting.message", username(label)),
  }),
  wedge: reg("wedge", ["title", "text", "confirm", "cancel"]),
  announced: (title: string, lines: string[]) => t("dashpay.registration.announced", { title, lines: lines.join(" ") }),
  finished: finishedText,
};

// ---------------------------------------------------------------------------------------------
// Step 1: the name (IOS-066, F4)

/** Android's checklist rows, plus the engine's double-hyphen rule. */
export const RULE_TEXT: Readonly<Record<UsernameRuleName, string>> = reg("rule", ["minLength", "maxLength", "allowedCharacters", "noEdgeHyphen", "noDoubleHyphen"]);

export type AvailabilityTone = "pending" | "ok" | "problem" | "neutral";

export interface AvailabilityLine {
  text: string;
  tone: AvailabilityTone;
}

/** The line under the checklist: `null` until the label is valid. `pending` while the typing has
 * not settled or the answer is not in. */
export function availabilityLine(check: UsernameCheck | undefined, availability: NameAvailability | undefined, pending: boolean): AvailabilityLine | null {
  if (!check?.valid) return null;
  if (pending || !availability) return { text: t("dashpay.registration.availability.checking"), tone: "pending" };
  switch (availability.kind) {
    case "available":
      return { text: t("dashpay.registration.availability.available"), tone: "ok" };
    case "contestOpen":
      return {
        text: t("dashpay.registration.availability.contestOpen"),
        tone: "neutral",
      };
    case "taken":
      return { text: t("dashpay.registration.availability.taken"), tone: "problem" };
    case "locked":
      return { text: t("dashpay.registration.availability.locked"), tone: "problem" };
    case "invalid":
      return { text: t("dashpay.registration.availability.invalid"), tone: "problem" };
    case "unknown":
      return { text: t("dashpay.registration.availability.unknown"), tone: "neutral" };
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
  | "voting"
  /** A Platform payment that may still go through: Continue or Move to Dash wallet. */
  | "held";

export type ScreenActionKind = "home" | "continue" | "moveToCore" | "unlock" | "finish" | "repair" | "another" | "done" | "wallet" | "votingInfo" | Exclude<WelcomeStepKind, "share">;

/** R9's next steps (PRODUCT #4). */
export type WelcomeStepKind = "editProfile" | "findFriends" | "share";

export const shareTitle = () => t("dashpay.registration.welcome.share.title");

const WELCOME_KINDS: readonly WelcomeStepKind[] = ["editProfile", "findFriends", "share"];

export const welcomeSteps = (): { kind: WelcomeStepKind; title: string; text: string }[] =>
  WELCOME_KINDS.map((kind) => ({ kind, title: t(`dashpay.registration.welcome.${kind}.title`), text: t(`dashpay.registration.welcome.${kind}.text`) }));

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
      return t("dashpay.registration.nameFailure.taken");
    case "name.contest_open":
      return t("dashpay.registration.nameFailure.contestOpen");
    case "name.locked":
      return t("dashpay.registration.nameFailure.locked");
    case "name.invalid":
    case "name.unavailable_for_invite":
      return t("dashpay.registration.nameFailure.invalid");
    case "platform.insufficient_credits":
    case "platform.grant_exceeded":
      return t("dashpay.registration.nameFailure.credits");
    default:
      return t("dashpay.failure.stoppedWith", { code });
  }
}

/** The screen for `row` (the design's R1–R11). */
export function screenModel(row: RegistrationStatus, i: ScreenInputs): ScreenModel {
  const { label } = row;
  const step = stepOf(row.phase, row.failure, row.invitation !== null);
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
    if (i.contestPending) return votingScreen(label, i.contestEndsAt, i.date);
    const done = doneScreen(label);
    // A claim's request to its inviter (DP5-03 C2).
    const note = requestNote(row);
    return note ? { ...done, lines: [...done.lines, note] } : done;
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

  if (row.waiting === "paymentHeld")
    return base("held", progressTitle, [REG_COPY.progress.message(label), heldWithBoth()], [act("continue", REG_COPY.progress.continue), act("moveToCore", REG_COPY.progress.moveToCore)]);
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

/** DP5-03 C2 (F2, D8): a claim's Done note on the request to its inviter; `null` when none was
 * asked (no inviter, or the user opted out). */
export function requestNote(row: RegistrationStatus): string | null {
  const inv = row.invitation;
  if (!inv?.inviter || row.phase !== "done") return null;
  return inv.requestFailure ? REG_COPY.done.requestRefused : REG_COPY.done.requestSent;
}

/** R9: the welcome; its next steps are WELCOME_STEPS. */
function doneScreen(label: string): ScreenModel {
  return { kind: "done", label, title: REG_COPY.done.title(label), lines: [REG_COPY.done.message], step: null, busy: false, actions: [act("wallet", REG_COPY.done.action)] };
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
    if (needed === null) return t("dashpay.failure.fundingShort");
    return discreet
      ? t("dashpay.failure.fundingNeeded", { needed: amount(needed) })
      : t("dashpay.failure.fundingNeededAvailable", { needed: amount(needed), available: amount(available) });
  }
  if (hasCode(e, "registration.coins_locked")) return t("dashpay.registration.startError.coinsLocked");
  if (hasCode(e, "registration.not_synced")) return t("dashpay.registration.startError.notSynced");
  if (hasCode(e, "registration.funding_unknown_recent")) {
    const secs = e.data?.kind === "draftWait" ? e.data.waitSecs : null;
    // m4 §2.2, verbatim.
    return secs === null
      ? t("dashpay.registration.startError.fundingUnknownLater")
      : t("dashpay.registration.startError.fundingUnknownWait", { wait: waitText(secs) });
  }
  if (hasCode(e, "registration.already_has_username")) return t("dashpay.registration.startError.hasUsername");
  if (hasCode(e, "registration.in_progress")) return t("dashpay.registration.startError.inProgress");
  if (hasCode(e, "platform.grant_exceeded")) return t("dashpay.registration.startError.grantExceeded");
  if (hasCode(e, "platform.insufficient_credits")) return t("dashpay.registration.startError.insufficientCredits");
  if (hasCode(e, "name.taken")) return t("dashpay.registration.startError.taken");
  if (hasCode(e, "name.locked")) return t("dashpay.registration.startError.locked");
  if (hasCode(e, "name.invalid")) return t("dashpay.registration.startError.invalid");
  if (hasCode(e, "name.contest_open")) return t("dashpay.registration.startError.contestOpen");
  if (hasCode(e, "platform.unavailable") || hasCode(e, "platform.timeout")) return t("dashpay.registration.startError.unreachable");
  if (hasCode(e, "platform.context_unavailable")) return t("dashpay.registration.startError.notSynced");
  if (hasCode(e, "vault.locked") || hasCode(e, "vault.mixing_only") || hasCode(e, "platform.signer_unavailable")) return t("dashpay.registration.startError.unlock");
  return isAppError(e) || e instanceof Error ? e.message : String(e);
}
