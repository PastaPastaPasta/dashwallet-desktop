// The registration wizard's flow state (DP1-09): which step the user is on, the label, the
// running call, and the draft this session started or resumed. One per wallet, kept while the app
// runs, so leaving the Join page and coming back finds the wizard where it was (D1); a relaunch
// starts empty and the engine's rows decide (the design's resume rules). Every grant goes through
// U-05's GrantModel: the passphrase sheet when one is needed.

import type { Backend } from "../api/backend";
import { hasCode } from "../api/backend";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { FinishReport } from "../api/generated/FinishReport";
import type { GrantRequest } from "../api/generated/GrantRequest";
import type { InvitationInviter } from "../api/generated/InvitationInviter";
import type { NameOutcome } from "../api/generated/NameOutcome";
import type { RegistrationRequest } from "../api/generated/RegistrationRequest";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import { Model } from "../onboarding/model";
import { GrantModel } from "../tools/grant";
import { CLAIM_KEPT } from "./claim";
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

export type WizardStep = "name" | "confirm";

export type RunningCall = "start" | "resume" | "discard" | "finish" | "name";

/** A Core-funded request for `label` (DP1-02's `CoreBalance` funding; no profile, D4/F2). */
export const coreRequest = (label: string): RegistrationRequest => ({
  label,
  temporaryLabel: null,
  funding: { kind: "coreBalance" },
  initialProfile: null,
});

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
  label = "";
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
  readonly grants: GrantModel;

  constructor(
    private readonly api: WizardApi,
    readonly walletId: string,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.subscribe(() => this.changed());
  }

  /** Opens step 1. */
  begin(mode: WizardMode) {
    this.mode = mode;
    this.step = "name";
    this.label = "";
    this.requestInviter = true;
    this.draft = null;
    this.named = null;
    this.error = null;
    this.wedge = null;
    this.changed();
  }

  setLabel(label: string) {
    this.label = label;
    this.changed();
  }

  setRequestInviter(on: boolean) {
    this.requestInviter = on;
    this.changed();
  }

  /** What Confirm starts: the claim in invitation mode, else a Core-funded registration. */
  request(): RegistrationRequest {
    const m = this.mode;
    return m.kind === "invitation" ? invitationRequest(this.label, m.linkId, this.requestInviter && m.inviter !== null) : coreRequest(this.label);
  }

  /** Step 1 → 2, for the label the screen has checked (`canChoose`, on the trimmed label). */
  next() {
    this.label = this.label.trim();
    this.step = "confirm";
    this.error = null;
    this.changed();
  }

  back() {
    this.step = this.step === "confirm" ? "name" : null;
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
  async confirm(grant: GrantRequest, past: string[] = []): Promise<void> {
    const request = this.request();
    await this.#call("start", async () => {
      const draft = await this.#withGrant(grant, (g) =>
        past.length === 0
          ? this.api.startRegistration(this.walletId, request, g.id)
          : this.api.startRegistrationPastUnknownFunding(this.walletId, request, g.id, past),
      );
      if (draft !== null) this.#started(draft);
    });
  }

  /** The wedge copy was confirmed: start past every blocker named so far. */
  async confirmWedge(grant: GrantRequest): Promise<void> {
    if (this.wedge !== null) await this.confirm(grant, this.wedge);
  }

  dismissWedge() {
    this.wedge = null;
    this.changed();
  }

  /** Continue (R2, R6): the row's label quoted again for the grant (no call sizes a resume's),
   * then `resume_registration`. */
  async resume(row: RegistrationStatus): Promise<void> {
    await this.#call("resume", async () => {
      // A claim row's grant is the row's (D20): its invitation's quote refuses a claim that landed.
      const grant = row.invitation
        ? await this.api.registrationResumeGrant(this.walletId, row.draft)
        : (await this.api.registrationQuote(this.walletId, { ...coreRequest(row.label), temporaryLabel: row.temporaryLabel })).grant;
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
      if (row && row.phase !== "done" && !row.fundsCommitted)
        await this.api.discardRegistration(this.walletId, row.draft).catch((e: unknown) => {
          if (!row.invitation || !hasCode(e, "invalid_argument")) throw e;
          // D11: a claim that may have reached the network is kept. One that can run is continued;
          // one stopped for good stays as it stopped, and its invitation's screen is shown below.
          if (!stopped) throw CLAIM_KEPT;
        });
      // A claim discarded goes back to its invitation's screen, which reads the link again (D1).
      if (row?.invitation && row.phase !== "done") return this.close();
      const identity = row?.phase === "done" ? row.identity : null;
      this.begin(identity ? { kind: "credits", identity } : { kind: "core" });
    });
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
