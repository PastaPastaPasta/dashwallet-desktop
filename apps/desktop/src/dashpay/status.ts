// The DashPay status surfaces' view model (DASHPAY §4 `DashPayStatusModel` and
// `JoinDashPayViewModel`, F1; DP1-08): the Home card for each banner state, the Join page's
// readiness, and their copy. Plain TypeScript over the generated types, no React; the queries are
// in queries.ts and the screens in screens/dashpay. Copy follows the mobile wallets (iOS
// `JoinDashPayView.swift`, `JoinDashPayScreen.swift`, `VotingInfoScreen.swift`; Android
// `strings-dashpay.xml`), adapted where the desktop differs (DP1-08 D-rows).

import type { DashPayStatus } from "../api/generated/DashPayStatus";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import type { LockStateName } from "../api/generated/LockStateName";
import type { RegistrationFailure } from "../api/generated/RegistrationFailure";
import type { RegistrationPhaseName } from "../api/generated/RegistrationPhaseName";
import type { RegistrationRequest } from "../api/generated/RegistrationRequest";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import type { RegistrationWaitName } from "../api/generated/RegistrationWaitName";
import type { StartupStatus } from "../api/generated/StartupStatus";
import { RESCAN_TEXT } from "../app/rescan";
import { oneDecimal } from "../format/sync";
import { messageTable, t } from "../i18n";


/** Each rendered state of the Home card (the design's state inventory, S2–S17; DP1-10's S19, S20). */
export type CardState =
  | "error"
  | "looking"
  | "notStarted"
  | "unlockToLook"
  | "lookFailed"
  | "join"
  | "joinWaiting"
  | "watchOnly"
  | "registering"
  | "creating"
  | "creatingUnlock"
  | "creatingPaused"
  | "creatingRepair"
  | "creatingHeld"
  | "voting"
  | "noUsername"
  | "lowCredits"
  | "emptyCredits";

/** What a card's button does: the screen maps each to its navigation or call. */
export type CardActionKind = "join" | "retry" | "unlock" | "continue" | "moveToCore" | "repair" | "votingInfo" | "chooseName" | "buyCredits";

export interface CardAction {
  kind: CardActionKind;
  label: string;
  disabled: boolean;
}

/** The card's look: `pending` for a registration or a vote under way, `problem` when the user has
 * something to fix, `quiet` for the rest. */
export type CardTone = "quiet" | "pending" | "problem";

/** A registration's step (D4): Android's three, as iOS's Home row counts them. */
export interface CardStep {
  n: 1 | 2 | 3;
  text: string;
}

export interface CardModel {
  state: CardState;
  tone: CardTone;
  title: string;
  /** Lines under the title, each its own paragraph. */
  lines: string[];
  step: CardStep | null;
  /** A spinner beside the title: DashPay is busy and nothing waits for the user. */
  busy: boolean;
  /** The pill under the card (iOS "Available after sync finishes"). */
  note: string | null;
  action: CardAction | null;
  /** A second button beside `action` (a held Platform payment's Move to Dash wallet). */
  secondary: CardAction | null;
}

export interface StatusInputs {
  status: DashPayStatus | undefined;
  /** The status read failed. */
  statusError: boolean;
  /** The main identity's summary, `undefined` while unread. */
  main: IdentitySummary | undefined;
  /** The registration the `registering` status names, `undefined` while unread or missing. */
  registration: RegistrationStatus | undefined;
  lock: LockStateName | undefined;
  /** Sync progress (0–100) while syncing, else `null`. */
  syncPercent: number | null;
  /** A contest's end (UNIX s) as the screen shows a date. */
  date: (secs: number) => string;
  /** Duffs with the unit. */
  amount: (duffs: number) => string;
  /** Discreet mode: the card leaves out the wallet's balance. */
  discreet: boolean;
}

/** `table` with `rest`'s members, its getters kept (a spread would read them once). */
export const withTable = <A extends object, B extends object>(table: A, rest: B): A & B => Object.defineProperties(rest, Object.getOwnPropertyDescriptors(table)) as A & B;

const at = <K extends string>(prefix: string, keys: readonly K[]) => messageTable(`dashpay.card.${prefix}`, keys);

export const COPY = {
  error: at("error", ["title", "message", "action"]),
  looking: at("looking", ["title"]),
  notStarted: at("notStarted", ["title", "message"]),
  unlockToLook: at("unlockToLook", ["title", "message"]),
  lookFailed: at("lookFailed", ["title", "message"]),
  unverified: at("unverified", ["title", "message"]),
  join: at("join", ["title", "message", "action"]),
  joinWaiting: at("joinWaiting", ["note"]),
  watchOnly: at("watchOnly", ["title", "message"]),
  registering: at("registering", ["title", "message"]),
  creating: (label: string) => t("dashpay.card.creating", { username: `@${label}` }),
  get paused() {
    return t("dashpay.card.paused");
  },
  repair: at("repair", ["unknown", "finish", "message", "action"]),
  voting: {
    title: (label: string) => t("dashpay.card.voting.title", { username: `@${label}` }),
    get deciding() {
      return t("dashpay.card.voting.deciding");
    },
    endsOn: (date: string) => t("dashpay.card.voting.endsOn", { date }),
    get notify() {
      return t("dashpay.card.voting.notify");
    },
    temporary: (name: string) => t("dashpay.card.voting.temporary", { username: `@${name}` }),
    get action() {
      return t("dashpay.card.voting.action");
    },
  },
  noUsername: at("noUsername", ["title", "message", "action"]),
  // DP1-10 D8: m4 item 44's copy (Android's, its swapped titles corrected).
  lowCredits: at("lowCredits", ["title", "message"]),
  emptyCredits: at("emptyCredits", ["title", "message"]),
  get buyCredits() {
    return t("dashpay.card.buyCredits");
  },
  get unlock() {
    return t("dashpay.card.unlock");
  },
  get continue() {
    return t("dashpay.card.continue");
  },
  get moveToCore() {
    return t("dashpay.card.moveToCore");
  },
};

/** D4: the step a phase is in. `failed` counts as the phase it stopped in. An invitation's claim
 * (`claim`) pays nothing: its first steps are the check and the claim (DP5-03 D10). */
export function stepOf(phase: RegistrationPhaseName, failure: RegistrationFailure | null, claim = false): CardStep {
  const at = phase === "failed" && failure ? failure.phase : phase;
  if (claim) {
    if (at === "draft") return { n: 1, text: t("dashpay.step.checkingInvitation") };
    if (at === "keysPrepared") return { n: 2, text: t("dashpay.step.identity") };
    return { n: 3, text: t("dashpay.step.username") };
  }
  switch (at) {
    case "draft":
    case "keysPrepared":
    case "fundingSent":
      return { n: 1, text: t("dashpay.step.payment") };
    case "proofWaiting":
      return { n: 2, text: t("dashpay.step.identity") };
    default:
      return { n: 3, text: t("dashpay.step.username") };
  }
}

/** What a running registration waits for, as a line under its step (the wizard's progress
 * screen shows the same lines, DP1-09 D11). */
export const WAITS = {
  get instantSend() {
    return t("dashpay.wait.payment");
  },
  get chainLock() {
    return t("dashpay.wait.payment");
  },
  get network() {
    return t("dashpay.wait.network");
  },
  get sync() {
    return t("dashpay.wait.sync");
  },
  get startup() {
    return t("dashpay.wait.startup");
  },
  get unlock() {
    return t("dashpay.wait.unlock");
  },
  get authorize() {
    return t("dashpay.wait.authorize");
  },
  get paymentUnknown() {
    return t("dashpay.wait.paymentUnknown");
  },
  get paymentHeld() {
    return t("dashpay.wait.paymentHeld");
  },
  // These two have their own card (S14).
  fundingUnknown: null,
  needsFinish: null,
} satisfies Record<RegistrationWaitName, string | null>;

/** DP1-09 D21/D26: the row waits for Continue's grant. It says `authorize`, or nothing runs it
 * (`running`) and it waits for a proof or the network: the engine reports an idle row's first
 * missing piece (`idle_wait`) whether or not a run waits for it, and nothing resumes such a row
 * alone (D12). An idle row held for sync or the DashPay start shows progress: once the hold
 * lifts it reads `authorize` or `unlock` and pauses, which D15's poll sees. */
export function awaitsContinue(r: RegistrationStatus): boolean {
  if (r.phase === "done" || r.phase === "failed" || r.phase === "contested" || r.failure) return false;
  return r.waiting === "authorize" || (!r.running && (r.waiting === "chainLock" || r.waiting === "network"));
}


/** DP5-03: an invitation claim's refusals, on its row (`failureText`) and on the claim screen
 * (claim.ts's `CLAIM_COPY`). */
export const CLAIM_REFUSAL = {
  get hasIdentity() {
    return t("dashpay.refusal.hasIdentity");
  },
  claimed: messageTable("dashpay.refusal.claimed", ["title", "text"]),
  expired: messageTable("dashpay.refusal.expired", ["title", "text"]),
  get invalid() {
    return t("dashpay.refusal.invalid");
  },
};

/** D5: why a registration stopped, by its failure's code. */
export function failureText(failure: RegistrationFailure, amount: (duffs: number) => string, discreet = false, fromPlatform = false): string {
  switch (failure.code) {
    // C-funding F10: a create that executed as a paid failure, from the Platform balance.
    case "registration.refunded":
      return fromPlatform ? t("dashpay.failure.refundedPlatform") : t("dashpay.failure.stoppedWith", { code: failure.code });
    case "registration.funding_insufficient":
      if (failure.needed === null) return t("dashpay.failure.fundingShort");
      // What the wallet holds stays hidden in discreet mode.
      return failure.available === null || discreet
        ? t("dashpay.failure.fundingNeeded", { needed: amount(Number(failure.needed)) })
        : t("dashpay.failure.fundingNeededAvailable", { needed: amount(Number(failure.needed)), available: amount(Number(failure.available)) });
    case "registration.coins_locked":
      return t("dashpay.failure.coinsLocked");
    case "registration.islock_timeout":
      return t("dashpay.failure.islockTimeout");
    case "registration.not_synced":
      return t("dashpay.failure.notSynced");
    case "rescan_interrupted":
      return RESCAN_TEXT.interrupted;
    // A funding budget below the lock and its fee; Continue asks for a new approval.
    case "platform.grant_exceeded":
      return t("dashpay.failure.grantExceeded");
    case "platform.insufficient_credits":
      return t("dashpay.failure.insufficientCredits");
    case "platform.unavailable":
    case "platform.timeout":
      return t("dashpay.failure.unavailable");
    case "invitation.already_has_identity":
      return CLAIM_REFUSAL.hasIdentity;
    case "invitation.claimed":
      return t("dashpay.refusal.claimedSentence");
    case "invitation.expired":
      return t("dashpay.refusal.expiredSentence");
    case "invitation.invalid":
      return CLAIM_REFUSAL.invalid;
    default:
      return t("dashpay.failure.stoppedWith", { code: failure.code });
  }
}

const card = (state: CardState, tone: CardTone, title: string, lines: string[] = [], rest: Partial<CardModel> = {}): CardModel => ({
  state,
  tone,
  title,
  lines,
  step: null,
  busy: false,
  note: null,
  action: null,
  secondary: null,
  ...rest,
});

const action = (kind: CardActionKind, label: string, disabled = false): CardAction => ({ kind, label, disabled });

/** Unlock, only while unlocked for mixing only: fully locked, the lock screen replaces the shell. */
const unlockAction = (lock: LockStateName | undefined): CardAction | null => (lock === "unlockedMixingOnly" ? action("unlock", COPY.unlock) : null);

/** S3–S6: DashPay has not said whether the wallet has an identity (BL-153 D2, D16). */
function startupCard(startup: StartupStatus, lock: LockStateName | undefined): CardModel {
  switch (startup) {
    case "notRun":
      return card("notStarted", "quiet", COPY.notStarted.title, [COPY.notStarted.message]);
    case "identityUnsettled":
      // Fully unlocked, the bring-up runs again at once.
      return card("unlockToLook", "quiet", COPY.unlockToLook.title, [COPY.unlockToLook.message], { action: unlockAction(lock) });
    // The identity's keys do not check against the seed: a restart does not change that.
    case "seedBindingUnverified":
      return card("lookFailed", "problem", COPY.unverified.title, [COPY.unverified.message]);
    case "discoveryFailed":
    case "identityScanIncomplete":
    case "partialNoIdentity":
    case "partialAccountsPending":
      return card("lookFailed", "problem", COPY.lookFailed.title, [COPY.lookFailed.message]);
    // Running, or an outcome read from a snapshot (D16): the pass will say.
    case "starting":
    case "ready":
    case "noIdentity":
      return card("looking", "quiet", COPY.looking.title, [], { busy: true });
  }
}

/** S10–S14: the registration the status names, by its row. */
function registeringCard(r: RegistrationStatus | undefined, i: StatusInputs): CardModel {
  if (!r) return card("registering", "pending", COPY.registering.title, [COPY.registering.message], { busy: true });
  const step = stepOf(r.phase, r.failure, r.invitation !== null);
  const title = COPY.creating(r.label);
  if (r.waiting === "fundingUnknown" || r.waiting === "needsFinish") {
    // Unknown: the lock may or may not have gone out; needsFinish: it did.
    const what = r.waiting === "fundingUnknown" ? COPY.repair.unknown : COPY.repair.finish;
    return card("creatingRepair", "problem", title, [what, COPY.repair.message], { step, action: action("repair", COPY.repair.action) });
  }
  // F22: a Platform payment that may still go through: Continue retries it, Move to Dash
  // wallet frees the balance instead.
  if (r.waiting === "paymentHeld" && !r.failure)
    return card("creatingHeld", "problem", title, [WAITS.paymentHeld], {
      step,
      action: action("continue", COPY.continue),
      secondary: action("moveToCore", COPY.moveToCore),
    });
  if (r.failure || awaitsContinue(r)) {
    const why = r.failure ? failureText(r.failure, i.amount, i.discreet, r.fromPlatform) : WAITS.authorize;
    return card("creatingPaused", "problem", title, [COPY.paused, why], { step, action: action("continue", COPY.continue) });
  }
  if (r.waiting === "unlock") return card("creatingUnlock", "problem", title, [WAITS.unlock], { step, action: unlockAction(i.lock) });
  const wait = r.waiting ? WAITS[r.waiting] : null;
  return card("creating", "pending", title, wait ? [wait] : [], { step, busy: true });
}

/** What the main identity's names say (D23): it has a name; they are not known yet (unread, a
 * sync pass holds them, or the engine's names pass has not ended); they could not be fetched; or
 * it owns none. Only the last asks for a username. */
export type NamesState = "named" | "updating" | "unknown" | "none";

export function namesState(main: IdentitySummary | undefined): NamesState {
  if (main?.mainName) return "named";
  if (!main || main.namesUpdating || main.namesCheck === "pending") return "updating";
  return main.namesCheck === "failed" ? "unknown" : "none";
}

/**
 * The Home card for the wallet's DashPay state (S1–S18), or `null` where Home shows none (D2):
 * the status unread, a `ready` identity with a name, or one whose names are not known yet. A named
 * identity whose credits are low or depleted has DP1-10's card (S19, S20, D8).
 */
export function homeCard(i: StatusInputs): CardModel | null {
  if (i.statusError) return card("error", "problem", COPY.error.title, [COPY.error.message], { action: action("retry", COPY.error.action) });
  const s = i.status;
  switch (s?.kind) {
    case undefined:
      return null;
    case "startupIncomplete":
      return startupCard(s.startup, i.lock);
    case "noIdentity":
      if (s.reason === "watchOnly") return card("watchOnly", "quiet", COPY.watchOnly.title, [COPY.watchOnly.message]);
      if (s.reason === "waitingForSync") {
        return card("joinWaiting", "quiet", COPY.join.title, [COPY.join.message], {
          note: i.syncPercent === null ? COPY.joinWaiting.note : t("dashpay.card.joinWaiting.notePercent", { percent: oneDecimal(i.syncPercent) }),
          action: action("join", COPY.join.action, true),
        });
      }
      return card("join", "quiet", COPY.join.title, [COPY.join.message], { action: action("join", COPY.join.action) });
    case "registering":
      return registeringCard(i.registration?.draft === s.draft ? i.registration : undefined, i);
    case "contestPending": {
      const lines: string[] = [COPY.voting.deciding, s.endsAt === null ? COPY.voting.notify : COPY.voting.endsOn(i.date(s.endsAt))];
      // The temporary name shows on a live read only (DASHPAY F5).
      const temporary = i.main && !i.main.namesUpdating && i.main.mainName !== s.label ? i.main.mainName : null;
      if (temporary) lines.push(COPY.voting.temporary(temporary));
      return card("voting", "pending", COPY.voting.title(s.label), lines, { action: action("votingInfo", COPY.voting.action) });
    }
    case "ready": {
      const names = namesState(i.main);
      if (names === "none")
        return card("noUsername", "quiet", COPY.noUsername.title, [COPY.noUsername.message], { action: action("chooseName", COPY.noUsername.action) });
      const level = names === "named" ? i.main?.creditLevel : null;
      if (level !== "low" && level !== "empty") return null;
      const copy = level === "low" ? COPY.lowCredits : COPY.emptyCredits;
      return card(level === "low" ? "lowCredits" : "emptyCredits", "problem", copy.title, [copy.message], { action: action("buyCredits", COPY.buyCredits) });
    }
  }
}

// ---------------------------------------------------------------------------------------------
// The Join page (`JoinDashPayViewModel`)

/** D6: a valid, non-contested label (20 characters, a digit 2–9). A non-contested name's cost does
 * not depend on its label, so its quote is the cost of any such username. */
export const QUOTE_LABEL = "dashpaydesktopquote2";

export const QUOTE_REQUEST: RegistrationRequest = {
  label: QUOTE_LABEL,
  temporaryLabel: null,
  funding: { kind: "coreBalance" },
  initialProfile: null,
};

/** The quote's figure: its total rounded up to 0.001 Dash, so "about" never undersells it. */
export const roundedCost = (totalDuffs: number) => Math.ceil(totalDuffs / 100_000) * 100_000;

const JOIN_FEATURES = ["usernames", "friends", "profile"] as const;

export const JOIN_COPY = withTable(messageTable("dashpay.join", ["headline", "pitch", "costUnknown", "create", "watchOnly", "votingLink", "faqLink"]), {
  get features() {
    return JOIN_FEATURES.map((k) => ({ title: t(`dashpay.join.features.${k}.title`), text: t(`dashpay.join.features.${k}.text`) }));
  },
  cost: (amount: string) => t("dashpay.join.cost", { amount }),
  shortfall: (amount: string) => t("dashpay.join.shortfall", { amount }),
  get waiting() {
    return COPY.joinWaiting.note;
  },
  joined: (name: string | null) => (name ? t("dashpay.join.joinedAs", { username: `@${name}` }) : t("dashpay.join.joined")),
});

/** What the Join page shows under its intro (J1–J7). */
export type JoinModel =
  /** J1, J2, J7: the costs, a shortfall, Create username (disabled until synced). */
  | { kind: "join"; cost: string; shortfall: string | null; waiting: boolean }
  /** J3 */
  | { kind: "watchOnly" }
  /** J4–J6: the Home card for the state, with its action. */
  | { kind: "card"; card: CardModel }
  /** An identity with a name, or one whose names are not known yet. */
  | { kind: "joined"; username: string | null };

export function joinModel(
  i: StatusInputs,
  /** The quote's total (duffs), `null` while unread or when it failed. */
  costDuffs: number | null,
  /** What the funding can spend now (duffs, the quote's `availableDuffs`, DP1-09 D5), `null`
   * while unread. */
  available: number | null,
): JoinModel {
  const s = i.status;
  if (!i.statusError && s?.kind === "noIdentity") {
    if (s.reason === "watchOnly") return { kind: "watchOnly" };
    const waiting = s.reason === "waitingForSync";
    if (costDuffs === null) return { kind: "join", cost: JOIN_COPY.costUnknown, shortfall: null, waiting };
    const price = i.amount(roundedCost(costDuffs));
    return {
      kind: "join",
      cost: JOIN_COPY.cost(price),
      // D18, as DP1-09 D5 measures it: the coins a start can spend, and no shortfall while the
      // balance is still syncing in.
      shortfall: !waiting && available !== null && available < costDuffs ? JOIN_COPY.shortfall(price) : null,
      waiting,
    };
  }
  // The credits cards are My Profile's business: the Join page says the user has joined.
  const c = homeCard(i);
  if (c && c.state !== "lowCredits" && c.state !== "emptyCredits") return { kind: "card", card: c };
  // Unread: the intro alone, its button held until the status says.
  if (!s) return { kind: "join", cost: JOIN_COPY.costUnknown, shortfall: null, waiting: true };
  return { kind: "joined", username: i.main?.mainName ?? null };
}

// ---------------------------------------------------------------------------------------------
// Voting info (IOS-134, AND-071) and the FAQ (IOS-129)

const VOTING_ITEMS = ["someCases", "blocked", "second", "notified", "phrase", "masternodes"] as const;

export const VOTING_INFO = withTable(messageTable("dashpay.votingInfo", ["title", "intro"]), {
  get items() {
    return VOTING_ITEMS.map((k) => ({ title: t(`dashpay.votingInfo.items.${k}.title`), text: t(`dashpay.votingInfo.items.${k}.text`) }));
  },
});

const FAQ_ITEMS = ["what", "cost", "privacy", "privateRequests", "bitcoin", "ethereum", "nameServices", "lostPhrase"] as const;

export const FAQ = withTable(messageTable("dashpay.faq", ["title", "headline", "pitch"]), {
  get items() {
    return FAQ_ITEMS.map((k) => ({ question: t(`dashpay.faq.items.${k}.question`), answer: t(`dashpay.faq.items.${k}.answer`) }));
  },
});
