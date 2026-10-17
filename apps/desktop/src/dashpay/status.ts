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
  | "voting"
  | "noUsername"
  | "lowCredits"
  | "emptyCredits";

/** What a card's button does: the screen maps each to its navigation or call. */
export type CardActionKind = "join" | "retry" | "unlock" | "continue" | "repair" | "votingInfo" | "chooseName" | "buyCredits";

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

export const COPY = {
  error: { title: "Could not read DashPay's state", message: "Your wallet works as usual.", action: "Try again" },
  looking: { title: "Looking for your DashPay identity…" },
  notStarted: { title: "DashPay has not started", message: "It starts when the wallet connects to the Dash network." },
  unlockToLook: { title: "Unlock to look for your DashPay identity", message: "DashPay looks for your identity once the wallet is fully unlocked." },
  lookFailed: { title: "Could not finish looking for your DashPay identity", message: "Dash Wallet looks again the next time it starts." },
  unverified: { title: "Could not confirm your DashPay identity", message: "Its keys could not be checked against this wallet's recovery phrase." },
  join: { title: "Join DashPay", message: "Request your username", action: "Join DashPay" },
  joinWaiting: { message: "Create a username, find your friends and add them to your contacts", note: "Available after sync finishes" },
  watchOnly: { title: "DashPay needs this wallet's keys", message: "A watch-only wallet cannot join DashPay." },
  registering: { title: "Creating your username", message: "Your username is being created on the Dash network. It may take some time." },
  creating: (label: string) => `Creating – @${label}`,
  paused: "Registration paused",
  repair: {
    unknown: "The payment may have been sent",
    finish: "Funds locked — finishing",
    message: "Open Tools ▸ Repair to finish the transfer.",
    action: "Open Repair",
  },
  voting: {
    title: (label: string) => `Voting – @${label}`,
    deciding: "Masternode owners are deciding whether you get this username.",
    endsOn: (date: string) => `Results on ${date}.`,
    notify: "We will notify you when voting ends.",
    temporary: (name: string) => `Until then you are @${name}.`,
    action: "What is username voting?",
  },
  noUsername: { title: "Finish username registration", message: "Your identity is ready. Use its existing credits to register a username.", action: "Choose a username" },
  // DP1-10 D8: m4 item 44's copy (Android's, its swapped titles corrected).
  lowCredits: {
    title: "Your credit balance is low",
    message: "Top-up your credits to continue making changes to your profile and adding contacts",
  },
  emptyCredits: {
    title: "Your credit balance is fully depleted",
    message: "You can continue to use DashPay for payments but you cannot update your profile or add more contacts until you top up your credit balance",
  },
  buyCredits: "Buy credits",
  unlock: "Unlock",
  continue: "Continue",
} as const;

/** D4: the step a phase is in. `failed` counts as the phase it stopped in. */
export function stepOf(phase: RegistrationPhaseName, failure: RegistrationFailure | null): CardStep {
  const at = phase === "failed" && failure ? failure.phase : phase;
  switch (at) {
    case "draft":
    case "keysPrepared":
    case "fundingSent":
      return { n: 1, text: "Processing payment" };
    case "proofWaiting":
      return { n: 2, text: "Creating identity" };
    default:
      return { n: 3, text: "Registering username" };
  }
}

/** What a running registration waits for, as a line under its step (the wizard's progress
 * screen shows the same lines, DP1-09 D11). */
export const WAITS = {
  instantSend: "Waiting for the network to confirm the payment.",
  chainLock: "Waiting for the network to confirm the payment.",
  network: "Waiting for the Dash network. It tries again on its own.",
  sync: "Waiting for the wallet to sync.",
  startup: "Waiting for DashPay to start.",
  unlock: "Unlock the wallet to continue.",
  authorize: "Waiting for your approval to continue.",
  // These two have their own card (S14).
  fundingUnknown: null,
  needsFinish: null,
} as const satisfies Record<RegistrationWaitName, string | null>;

/** DP1-09 D21/D26: the row waits for Continue's grant. It says `authorize`, or nothing runs it
 * (`running`) and it waits for a proof or the network: the engine reports an idle row's first
 * missing piece (`idle_wait`) whether or not a run waits for it, and nothing resumes such a row
 * alone (D12). An idle row held for sync or the DashPay start shows progress: once the hold
 * lifts it reads `authorize` or `unlock` and pauses, which D15's poll sees. */
export function awaitsContinue(r: RegistrationStatus): boolean {
  if (r.phase === "done" || r.phase === "failed" || r.phase === "contested" || r.failure) return false;
  return r.waiting === "authorize" || (!r.running && (r.waiting === "chainLock" || r.waiting === "network"));
}


/** D5: why a registration stopped, by its failure's code. */
export function failureText(failure: RegistrationFailure, amount: (duffs: number) => string, discreet = false): string {
  switch (failure.code) {
    case "registration.funding_insufficient":
      if (failure.needed === null) return "Not enough Dash to pay for the username.";
      // What the wallet holds stays hidden in discreet mode.
      return failure.available === null || discreet
        ? `Not enough Dash to pay for the username: ${amount(Number(failure.needed))} needed.`
        : `Not enough Dash to pay for the username: ${amount(Number(failure.needed))} needed, ${amount(Number(failure.available))} available.`;
    case "registration.coins_locked":
      return "The coins to pay with are held by another payment.";
    case "registration.islock_timeout":
      return "The network did not confirm the payment in time.";
    case "registration.not_synced":
      return "The wallet had not finished syncing.";
    // A funding budget below the lock and its fee; Continue asks for a new approval.
    case "platform.grant_exceeded":
      return "The cost went above what you approved.";
    case "platform.insufficient_credits":
      return "The identity's credits do not cover the username.";
    case "platform.unavailable":
    case "platform.timeout":
      return "The Dash network did not answer.";
    default:
      return `It stopped with ${failure.code}.`;
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
  ...rest,
});

const action = (kind: CardActionKind, label: string, disabled = false): CardAction => ({ kind, label, disabled });

/** Unlock, only while unlocked for mixing only: fully locked, the lock screen covers the shell. */
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
  const step = stepOf(r.phase, r.failure);
  const title = COPY.creating(r.label);
  if (r.waiting === "fundingUnknown" || r.waiting === "needsFinish") {
    // Unknown: the lock may or may not have gone out; needsFinish: it did.
    const what = r.waiting === "fundingUnknown" ? COPY.repair.unknown : COPY.repair.finish;
    return card("creatingRepair", "problem", title, [what, COPY.repair.message], { step, action: action("repair", COPY.repair.action) });
  }
  if (r.failure || awaitsContinue(r)) {
    const why = r.failure ? failureText(r.failure, i.amount, i.discreet) : WAITS.authorize;
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
        const percent = i.syncPercent === null ? "" : ` · ${i.syncPercent.toFixed(1)}%`;
        return card("joinWaiting", "quiet", COPY.join.title, [COPY.joinWaiting.message], {
          note: COPY.joinWaiting.note + percent,
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

export const JOIN_COPY = {
  headline: "Create username",
  pitch: "Forget about long crypto addresses, create the username, find friends and add them to your contacts",
  features: [
    { title: "Pay to usernames", text: "No more alphanumeric addresses" },
    { title: "Add your friends & family", text: "Invite your family, find your friends by searching their usernames" },
    { title: "Personalize profile", text: "Upload your picture, personalize your identity" },
  ],
  cost: (amount: string) => `A username costs about ${amount}. Usernames the network votes on cost more; the exact cost is shown before you pay.`,
  costUnknown: "Usernames the network votes on cost more than others. The exact cost is shown before you pay.",
  shortfall: (amount: string) => `You need at least ${amount} to create a username.`,
  create: "Create username",
  waiting: COPY.joinWaiting.note,
  watchOnly: "A watch-only wallet cannot join DashPay. Restore the wallet from its recovery phrase to join.",
  joined: (name: string | null) => (name ? `You have joined DashPay as @${name}.` : "You have joined DashPay."),
  votingLink: "What is username voting?",
  faqLink: "About DashPay",
} as const;

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

export const VOTING_INFO = {
  title: "What is username voting?",
  intro: "The Dash network has to vote to approve some usernames before they are created",
  items: [
    { title: "Voting is only required in some cases", text: "Any username that has a number 2–9 or 20 or more characters will be automatically approved" },
    { title: "Some usernames can be blocked", text: "If enough of the network determines that a username is inappropriate, they can block it" },
    {
      title: "A second username to use meanwhile",
      text: "While the network votes you can register an instant username too, and use it until the vote ends",
    },
    { title: "You will be notified", text: "When the voting ends, Dash Wallet tells you whether you got the username" },
    { title: "Keep your recovery phrase safe", text: "In case you lose your recovery phrase you will lose your right to your requested username" },
    {
      title: "Masternode owners vote",
      text: "Each masternode can vote to approve or block a requested username. Voting from this wallet comes in a later release",
    },
  ],
} as const;

export const FAQ = {
  title: "About DashPay",
  headline: "Pay people, not addresses",
  pitch: "DashPay replaces long cryptic addresses with usernames. Add friends as contacts, send money to a name, and keep every payment organized by person.",
  items: [
    {
      question: "What is DashPay?",
      answer:
        "DashPay is Dash's social payments experience, built on Dash Platform. You register a username, add other users as contacts, and pay them by name — no more copying addresses.",
    },
    {
      question: "What does it cost?",
      answer:
        "Creating a username is a one-time payment from your Dash balance; usernames the network votes on cost more. The exact cost is shown before you pay. Part of it becomes credits that pay for contact requests and profile changes. Payments to contacts cost the usual network fees.",
    },
    {
      question: "How private is DashPay?",
      answer:
        "Payments are private: the addresses you exchange with a contact travel inside an encrypted payload only the two of you can read, and are never published — so your payments aren't trivially linkable to your username. They are still regular transparent-chain payments, though: sophisticated chain analysis might leak information. Contact requests themselves are currently not private: anyone can see that two identities are connected. Your username and profile are also public on Dash Platform.",
    },
    {
      question: "When will contact requests become private?",
      answer:
        "Private contact requests are in development. Today the request itself is public — anyone can see that two identities are connected, even though the payment details inside it are encrypted. Once private contact requests ship, you will have the choice of having your friendship be public or private.",
    },
    {
      question: "How does this compare to Bitcoin in terms of privacy?",
      answer:
        "On Bitcoin there is no name layer, so people share and reuse addresses out in the open — once an address is known, everything it ever received is linkable to its owner. With DashPay your username is public, but it never points at a payment address: addresses are exchanged privately per contact and rotate, so paying by name doesn't publish where your money goes. Both are transparent chains, so chain analysis still applies to the coins themselves.",
    },
    {
      question: "How does this compare to Ethereum accounts in terms of privacy?",
      answer:
        "An Ethereum account is one reusable address: your entire balance and payment history sit publicly under it, and a name (like an ENS domain) typically points straight at that address for anyone to resolve. DashPay is the opposite shape — the name resolves to an identity, not a payment address, and actual payment addresses stay inside encrypted contact exchanges, fresh for each contact.",
    },
    {
      question: "What about Unstoppable Domains and similar name services?",
      answer:
        "Name services like Unstoppable Domains map a human-readable name to a fixed public address on-chain. Anyone can resolve the name and see every payment ever made to it. A DashPay username never publicly resolves to a payment address — addresses are revealed only inside the encrypted exchange with each contact, so your name and your money stay unlinked to outside observers.",
    },
    {
      question: "What if I lose my recovery phrase?",
      answer:
        "Your username belongs to the identity your recovery phrase creates. Restoring the wallet from the phrase brings the username, your contacts and your payments back. Without the phrase nobody, Dash Wallet included, can recover them.",
    },
  ],
} as const;
