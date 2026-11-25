// The registration wizard's flow state (DP1-09): which step the user is on, the label, the
// running call, and the draft this session started or resumed. One per wallet, kept while the app
// runs, so leaving the Join page and coming back finds the wizard where it was (D1); a relaunch
// starts empty and the engine's rows decide (the design's resume rules). Every grant goes through
// U-05's GrantModel: the passphrase sheet when one is needed.

import type { Backend } from "../api/backend";
import { hasCode } from "../api/backend";
import type { AppError } from "../api/generated/commands";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { FinishReport } from "../api/generated/FinishReport";
import type { GrantRequest } from "../api/generated/GrantRequest";
import type { InvitationInviter } from "../api/generated/InvitationInviter";
import type { NameOutcome } from "../api/generated/NameOutcome";
import type { PlatformRegistrationQuote } from "../api/generated/PlatformRegistrationQuote";
import type { RegistrationQuote } from "../api/generated/RegistrationQuote";
import type { RegistrationRequest } from "../api/generated/RegistrationRequest";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import { Model } from "../onboarding/model";
import { GrantModel } from "../tools/grant";
import { CLAIM_KEPT, claimFoundIdentity } from "./claim";
import { LIVE_SOURCES, type PaySource } from "./payWith";
import { QUOTE_REQUEST } from "./status";

export type WizardApi = Pick<
  Backend,
  | "registrationQuote"
  | "registrationResumeGrant"
  | "startRegistration"
  | "startRegistrationPastUnknownFunding"
  | "resumeRegistration"
  | "discardRegistration"
  | "finishAssetLocks"
  | "registrations"
  | "grantRequest"
  | "registerName"
  | "authorize"
  | "revokeGrant"
  | "lockStatus"
  | "paymentAuthentication"
  | "unlock"
  | "unlockMixingOnly"
>;

/** `core`: a new identity funded from the wallet; `credits`: a name for an identity the wallet
 * has, paid from its credits (D6); `instant` refuses a contested label (DP1-10 D13);
 * `invitation`: the claim of a stashed invitation (DP5-03 D1), which pays for both. */
export type WizardMode =
  | { kind: "core" }
  | { kind: "credits"; identity: string; instant?: boolean }
  | { kind: "invitation"; linkId: string; contestedAllowed: boolean; inviter: InvitationInviter | null };

/** `source`: "Make your username private", the step before the name (C-funding F19); `instant`:
 * the instant username beside a contested one (D4 reversed), between the name and Confirm. */
export type WizardStep = "source" | "name" | "instant" | "confirm";

export type RunningCall = "start" | "resume" | "discard" | "finish" | "name";

/** A Core-funded request for `label` (DP1-02's `CoreBalance` funding; no profile, D4/F2). */
export const coreRequest = (label: string, temporaryLabel: string | null = null): RegistrationRequest => ({
  label,
  temporaryLabel,
  funding: { kind: "coreBalance" },
  initialProfile: null,
});

/** C-funding F4: a request paid from the Platform balance for `label`, with the numbers its quote
 * showed (the start refuses `registration.quote_changed` for others). A quote of the source passes
 * zeros: the engine plans its own. */
export const platformRequest = (
  label: string,
  quote: Pick<PlatformRegistrationQuote, "fundingCredits" | "feeReserveCredits"> = { fundingCredits: "0", feeReserveCredits: "0" },
  temporaryLabel: string | null = null,
): RegistrationRequest => ({
  label,
  temporaryLabel,
  funding: { kind: "platformAddresses", fundingCredits: quote.fundingCredits, feeReserveCredits: quote.feeReserveCredits },
  initialProfile: null,
});

/** C-funding F5: a request paid from the Shielded balance for `label`, at the denomination its
 * quote showed (the start refuses `registration.quote_changed` for another). A quote of the source
 * passes zero: the engine picks its own. */
export const shieldedRequest = (label: string, denomination = "0", temporaryLabel: string | null = null): RegistrationRequest => ({
  label,
  temporaryLabel,
  funding: { kind: "shielded", denomination },
  initialProfile: null,
});

/** What Confirm starts under: the quote's grant, and its Platform numbers or Shielded readiness
 * when it is one. */
export type ConfirmQuote = Pick<RegistrationQuote, "grant" | "platform" | "readiness">;

/** DP5-03: the claim of the stashed invitation `linkId` for `label`; `requestInviter: false` asks
 * no one (D7). */
export const invitationRequest = (label: string, linkId: string, requestInviter: boolean): RegistrationRequest => ({
  label,
  temporaryLabel: null,
  funding: { kind: "invitation", linkId, requestInviter },
  initialProfile: null,
});

/** D10: Finish transfers' grant: one duff, as a top-up lock's apply needs a funding budget (none
 * is spent), and the credits DP1-02's quote bounds. */
export async function finishGrant(api: Pick<Backend, "registrationQuote">, walletId: string): Promise<GrantRequest> {
  const quote = await api.registrationQuote(walletId, QUOTE_REQUEST);
  return { maxDuffs: 1, maxCredits: quote.grant.maxCredits };
}

const platformOp = (g: GrantRequest) => ({ kind: "platformOp" as const, maxDuffs: g.maxDuffs, maxCredits: g.maxCredits });

export class RegistrationWizard extends Model {
  /** `null` while the wizard is not open: the Join page shows its intro or a row's screen. */
  step: WizardStep | null = null;
  mode: WizardMode = { kind: "core" };
  /** Where a new identity is paid from: nothing each time the wizard opens, never the last choice
   * (C-funding F18, mobile parity). Only a source this build can start is ever chosen
   * (`LIVE_SOURCES`); the name step is not reached without one. */
  source: PaySource | null = null;
  /** The wizard was opened on the source step: Back from the name returns there. */
  picked = false;
  label = "";
  /** The instant offer shows over the name step (D4 reversed): the label is contested. */
  offer = false;
  /** The instant username the start registers beside the contested label; `null` for none. */
  temporaryLabel: string | null = null;
  /** The claim's "Send a contact request" check (DP5-03 D7). */
  requestInviter = true;
  /** The draft this session started or resumed: its result screen shows until Done. */
  draft: string | null = null;
  /** A name registered from credits this session (D6): its result shows until Done. */
  named: { label: string; result: NameOutcome } | null = null;
  running: RunningCall | null = null;
  /** The last call's refusal, as the screen says it (`startErrorText`); cleared by the next call. */
  error: unknown = null;
  /** `registration.in_progress` past wedged fundings (m4 §2.2): the blockers the user is asked
   * to start past, each one the engine named so far. */
  wedge: string[] | null = null;
  /** Finish transfers' report, once it ran. */
  finished: FinishReport | null = null;
  /** When the last plan was refused, at the start or at run time (F4, F22): Confirm pays only a
   * quote read since. */
  requoteAt: number | null = null;
  readonly grants: GrantModel;

  constructor(
    private readonly api: WizardApi,
    readonly walletId: string,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.subscribe(() => this.changed());
  }

  /** Opens step 1, or the source step first with nothing chosen (C-funding F18, F19). A new
   * identity's name step without the source step pays from the Dash wallet. */
  begin(mode: WizardMode, step: "name" | "source" = "name") {
    this.mode = mode;
    this.step = step;
    this.source = step === "source" ? null : "core";
    this.picked = step === "source";
    this.label = "";
    this.offer = false;
    this.temporaryLabel = null;
    this.requestInviter = true;
    this.draft = null;
    this.named = null;
    this.error = null;
    this.wedge = null;
    this.requoteAt = null;
    this.changed();
  }

  setSource(source: PaySource) {
    this.source = source;
    this.changed();
  }

  /** The source step → the name, for a chosen source this build can start. */
  pickSource() {
    if (this.source === null || !LIVE_SOURCES.has(this.source)) return;
    this.step = "name";
    this.error = null;
    this.changed();
  }

  /** "Continue without privacy": the Dash wallet, then the name. */
  withoutPrivacy() {
    this.source = "core";
    this.pickSource();
  }

  setLabel(label: string) {
    this.label = label;
    this.changed();
  }

  setTemporaryLabel(label: string) {
    this.temporaryLabel = label;
    this.changed();
  }

  setRequestInviter(on: boolean) {
    this.requestInviter = on;
    this.changed();
  }

  /** Whether Confirm pays from the Platform balance: a new identity whose source step chose it. */
  get fromPlatform(): boolean {
    return this.mode.kind === "core" && this.source === "platform";
  }

  /** Whether Confirm pays from the Shielded balance: a new identity whose source step chose it. */
  get fromShielded(): boolean {
    return this.mode.kind === "core" && this.source === "shielded";
  }

  /** What Confirm starts: the claim in invitation mode, else a registration paid from the chosen
   * source, with the numbers of `quote` when that is the Platform balance. */
  request(quote: ConfirmQuote | null = null): RegistrationRequest {
    const m = this.mode;
    if (m.kind === "invitation") return invitationRequest(this.label, m.linkId, this.requestInviter && m.inviter !== null);
    const temp = this.temporaryLabel;
    if (this.fromPlatform) return platformRequest(this.label, quote?.platform ?? undefined, temp);
    if (this.fromShielded) return shieldedRequest(this.label, quote?.readiness?.denomination ?? undefined, temp);
    return coreRequest(this.label, temp);
  }

  /** The wizard offers an instant username beside a contested one: a new identity, which owns no
   * name yet. A name from credits is one `register_name`, and a claim's invitation pays for one
   * name: the voting card offers theirs once the contest is open. */
  get offersInstant(): boolean {
    return this.mode.kind === "core";
  }

  /** Step 1 → 2, for the label the screen has checked (`canChoose`, on the trimmed label). A
   * contested label the wizard offers an instant username beside asks first (`offer`). */
  next(contested = false) {
    this.label = this.label.trim();
    this.temporaryLabel = null;
    this.error = null;
    if (contested && this.offersInstant) this.offer = true;
    else this.step = "confirm";
    this.changed();
  }

  /** The offer's "Create instant username": the form, prefilled with `suggestion`. */
  createInstant(suggestion: string) {
    this.offer = false;
    this.temporaryLabel = suggestion;
    this.step = "instant";
    this.changed();
  }

  /** The offer's "No, thanks": Confirm, for the contested label alone. */
  declineInstant() {
    this.offer = false;
    this.temporaryLabel = null;
    this.step = "confirm";
    this.changed();
  }

  /** The offer closed: back to the name. */
  dismissOffer() {
    this.offer = false;
    this.changed();
  }

  /** The instant form → Confirm, for the name the screen has checked (`instantCheck`). */
  confirmInstant() {
    if (this.temporaryLabel === null) return;
    this.temporaryLabel = this.temporaryLabel.trim();
    this.step = "confirm";
    this.error = null;
    this.changed();
  }

  back() {
    if (this.step === "confirm") this.step = this.temporaryLabel !== null ? "instant" : "name";
    else if (this.step === "instant") {
      this.step = "name";
      this.temporaryLabel = null;
    } else if (this.step === "name" && this.picked) this.step = "source";
    else this.step = null;
    this.error = null;
    this.wedge = null;
    this.grants.cancel();
    this.changed();
  }

  /** The page shows `draft`, the registration the status says goes on: its result then stays
   * shown until Done, as this session's own does. */
  adopt(draft: string) {
    if (this.draft !== null || this.step !== null) return;
    this.draft = draft;
    this.changed();
  }

  /** Back to the Join page's own state: the result was seen, or the wizard was left. */
  close() {
    this.step = null;
    this.draft = null;
    this.named = null;
    this.error = null;
    this.wedge = null;
    this.finished = null;
    this.grants.cancel();
    this.changed();
  }

  /** Confirm & pay (D8: one start per Confirm). The quote's grant, then `start_registration`. A
   * refusal naming another registration shows that one; one past a wedged funding asks first. */
  async confirm(quote: ConfirmQuote, past: string[] = []): Promise<void> {
    // A Platform start takes its numbers from a Platform quote; a quote without them authorizes
    // nothing (F4).
    if ((this.fromPlatform && quote.platform === null) || (this.fromShielded && (quote.readiness?.denomination ?? null) === null)) {
      this.error = { code: "registration.quote_changed", message: "not a quote of the chosen source", data: null } satisfies AppError;
      this.requoteAt = Date.now();
      this.changed();
      return;
    }
    const request = this.request(quote);
    await this.#call("start", async () => {
      try {
        const draft = await this.#withGrant(quote.grant, (g) =>
          past.length === 0
            ? this.api.startRegistration(this.walletId, request, g.id)
            : this.api.startRegistrationPastUnknownFunding(this.walletId, request, g.id, past),
        );
        if (draft !== null) this.#started(draft);
      } catch (e) {
        // The refused quote is never paid again: Pay waits for one read since.
        if (hasCode(e, "registration.quote_changed")) this.requoteAt = Date.now();
        throw e;
      }
    });
  }

  /** The wedge copy was confirmed: start past every blocker named so far. */
  async confirmWedge(quote: ConfirmQuote): Promise<void> {
    if (this.wedge !== null) await this.confirm(quote, this.wedge);
  }

  dismissWedge() {
    this.wedge = null;
    this.changed();
  }

  /** Continue (R2, R6): the row's label quoted again for the grant, or a claim's or a
   * Platform-funded row's own grant (`registration_resume_grant`), then `resume_registration`. */
  async resume(row: RegistrationStatus): Promise<void> {
    await this.#call("resume", async () => {
      // A claim row's grant is the row's (D20): its invitation's quote refuses a claim that landed.
      // A Platform-funded row's too: a quote of its addresses refuses while its payment
      // holds them, and a Core quote's grant carries duffs; a Shielded-funded row's lease takes none.
      const grant = row.invitation || row.fromPlatform || row.fromShielded
        ? await this.api.registrationResumeGrant(this.walletId, row.draft)
        : (await this.api.registrationQuote(this.walletId, coreRequest(row.label, row.temporaryLabel))).grant;
      const done = await this.#withGrant(grant, (g) => this.api.resumeRegistration(this.walletId, row.draft, g.id).then(() => true));
      if (done) this.draft = row.draft;
    });
  }

  /** Choose another username (R6, R7, R10): an unfinished row is discarded first, and only while
   * nothing is committed (D7); after a refused name the identity pays from its credits. */
  async another(row: RegistrationStatus | null): Promise<void> {
    // A funded row that may run again is never left for a new one (D7); one stopped for good is
    // kept as it is, and the start decides (D27).
    const stopped = row?.phase === "failed" && row.failure?.retryable === false;
    if (row && row.phase !== "done" && row.fundsCommitted && !stopped) return;
    // A done row's next name is its identity's; without one it would be a second lock.
    if (row?.phase === "done" && !row.identity) return;
    await this.#call("discard", async () => {
      // D24: a claim refused for the seed's other identity is kept; it refuses the next claim.
      if (row && row.phase !== "done" && !row.fundsCommitted && !claimFoundIdentity(row)) {
        await this.api.discardRegistration(this.walletId, row.draft).catch((e: unknown) => {
          if (!row.invitation || !hasCode(e, "invalid_argument")) throw e;
          // D11: a claim that may have reached the network is kept. One that can run is continued;
          // one stopped for good stays as it stopped, and its invitation's screen is shown below.
          if (!stopped) throw CLAIM_KEPT;
        });
      }
      // A claim discarded goes back to its invitation's screen, which reads the link again (D1).
      if (row?.invitation && row.phase !== "done") return this.close();
      const identity = row?.phase === "done" ? row.identity : null;
      if (identity) this.begin({ kind: "credits", identity });
      else this.begin({ kind: "core" }, "source");
    });
  }

  /** Back to Confirm for `row`'s name and source with a new quote (F22), its row discarded
   * while nothing is committed. Nothing plans until the user confirms again. */
  async reconfirm(row: RegistrationStatus): Promise<void> {
    if (row.fundsCommitted) return;
    await this.#call("discard", async () => {
      await this.api.discardRegistration(this.walletId, row.draft);
      this.begin({ kind: "core" }, "source");
      this.source = row.fromPlatform ? "platform" : row.fromShielded ? "shielded" : "core";
      this.label = row.label;
      this.temporaryLabel = row.temporaryLabel;
      this.step = "confirm";
      this.requoteAt = Date.now();
    });
  }

  /** Whether a quote read at `readAt` may be paid: after a refused plan, only one read since. */
  quoteStands(readAt: number): boolean {
    return this.requoteAt === null || readAt >= this.requoteAt;
  }

  /** Finish transfers (R5, D10). */
  async finish(): Promise<void> {
    await this.#call("finish", async () => {
      const report = await this.#withGrant(await finishGrant(this.api, this.walletId), (g) => this.api.finishAssetLocks(this.walletId, g.id));
      if (report) this.finished = report;
    });
  }

  /** The credits path's Confirm (D6): `register_name` under the grant `grant_request` sizes. */
  async registerName(identity: string): Promise<void> {
    const label = this.label;
    await this.#call("name", async () => {
      const grant = await this.api.grantRequest(this.walletId, identity, { kind: "registerName", label });
      const outcome = await this.#withGrant(grant, (g) => this.api.registerName(this.walletId, identity, label, g.id));
      if (outcome) {
        this.named = { label, result: outcome };
        this.step = null;
      }
    });
  }

  #started(draft: string) {
    this.draft = draft;
    this.step = null;
    this.wedge = null;
  }

  /** `run` under a `platformOp` grant of `caps`; `null` when the user cancelled the passphrase. */
  async #withGrant<T>(caps: GrantRequest, run: (g: AuthGrant) => Promise<T>): Promise<T | null> {
    const got = await this.grants.withGrant(platformOp(caps), this.walletId, run);
    return got ? got.value : null;
  }

  /** One call at a time; its refusal is kept for the screen. */
  async #call(kind: RunningCall, run: () => Promise<void>) {
    if (this.running) return;
    this.running = kind;
    this.error = null;
    this.changed();
    try {
      await run();
    } catch (e) {
      const draft = hasCode(e, "registration.in_progress") && e.data?.kind === "draft" ? e.data.draft : null;
      if (draft !== null && kind === "start") await this.#inProgress(draft, e);
      else this.error = e;
    } finally {
      this.running = null;
      this.changed();
    }
  }

  /** `registration.in_progress{draft}` at a start: a registration of the wallet goes on. If its
   * funding is unknown and wedged, the user may start past it (the wedge copy); otherwise the
   * wizard shows it instead of starting a second one. */
  async #inProgress(draft: string, e: unknown) {
    const rows = await this.api.registrations(this.walletId).catch(() => [] as RegistrationStatus[]);
    const row = rows.find((r) => r.draft === draft);
    if (row?.waiting === "fundingUnknown") {
      this.wedge = [...(this.wedge ?? []).filter((d) => d !== draft), draft];
      return;
    }
    // A funding entry with no registration of its own (no label) has no screen to show.
    if (row && row.label !== "") {
      this.#started(draft);
      // A row stopped for good shows again: say why no new one started (D27).
      if (row.failure?.retryable === false) this.error = e;
      return;
    }
    this.error = e;
  }
}

const wizards = new Map<string, RegistrationWizard>();

/** The wallet's wizard, created on first use and kept while the app runs. */
export function wizardFor(api: WizardApi, walletId: string): RegistrationWizard {
  let w = wizards.get(walletId);
  if (!w) {
    w = new RegistrationWizard(api, walletId);
    wizards.set(walletId, w);
  }
  return w;
}

/** Tests: forget every wizard (a fresh app). */
export function resetWizards() {
  wizards.clear();
}
