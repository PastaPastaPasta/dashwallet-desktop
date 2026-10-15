// The contact actions one surface runs (DP2-09 D3, D16): Accept, Ignore, Unignore, saving the
// private details and enabling DashPay's keys, for the window's wallet and its main identity.
// Each Platform write sizes its `platformOp` grant with `grant_request` and gets it through
// U-05's GrantModel (the passphrase sheet when needed). One action runs at a time; the screens
// read `running` to disable their buttons and show the outcome where the action was (D4).

import { hasCode, isAppError, type Backend } from "../api/backend";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { GrantAction } from "../api/generated/GrantAction";
import type { PrivateContactDetails } from "../api/generated/PrivateContactDetails";
import type { PublishStateName } from "../api/generated/PublishStateName";
import { Model } from "../onboarding/model";
import { ENABLE_FIRST } from "./pane";
import type { GrantModel } from "../tools/grant";

export type ActionApi = Pick<
  Backend,
  "grantRequest" | "dashpayAcceptRequest" | "dashpayIgnoreContact" | "dashpayUnignoreContact" | "dashpaySetPrivateDetails" | "dashpayEnableKeys"
>;

export type ActionKind = "accept" | "ignore" | "unignore" | "details" | "enable";

/** What an action did, said where it ran; `contact` is `null` for Enable. */
export interface ActionOutcome {
  kind: ActionKind;
  contact: string | null;
  ok: boolean;
  text: string;
}

/** What the screens re-read after an action, besides what the engine's signal re-reads: the
 * contacts of the identity, its identities (keys), or its cached network search (D6). */
export type Refresh = "contacts" | "identities" | "search";

/** D4: the copy of a refusal. */
export function actionError(e: unknown): string {
  if (hasCode(e, "platform.insufficient_credits")) return "Not enough credits. Top up your credits and try again.";
  if (hasCode(e, "contact.ineligible")) return ENABLE_FIRST;
  if (hasCode(e, "contact.already_contact")) return "You are already contacts.";
  if (hasCode(e, "contact.details_too_long")) return "The alias and note are too long.";
  if (hasCode(e, "platform.unavailable") || hasCode(e, "platform.timeout")) return "Could not reach Dash Platform. Try again.";
  if (hasCode(e, "vault.locked") || hasCode(e, "platform.signer_unavailable")) return "Unlock the wallet and try again.";
  return isAppError(e) || e instanceof Error ? e.message : String(e);
}

/** D7: what a save says. */
export function publishText(state: PublishStateName): string {
  switch (state) {
    case "published":
      return "Saved";
    case "deferredUntilTwoContacts":
      return "Saved. It is published once you have two contacts.";
    case "local":
      return "Saved on this device.";
  }
}

export class ContactActions extends Model {
  /** The action under way; `null` while none is. */
  running: { kind: ActionKind; contact: string | null } | null = null;
  /** The last action's outcome; a new action clears it. */
  outcome: ActionOutcome | null = null;

  constructor(
    private readonly api: ActionApi,
    readonly grants: GrantModel,
    readonly walletId: string,
    readonly identity: string,
    private readonly refresh: (what: Refresh[]) => void = () => undefined,
  ) {
    super();
  }

  /** Whether an action runs now (the screens disable every action meanwhile). */
  get busy(): boolean {
    return this.running !== null;
  }

  /** Whether `kind` runs for `contact` now. */
  isRunning(kind: ActionKind, contact: string | null): boolean {
    return this.running?.kind === kind && this.running.contact === contact;
  }

  /** Accepts `contact`'s request (D3): it sends the reverse request under a grant. Already
   * contacts (another surface or device accepted first) is what was asked for. */
  accept(contact: string, name: string): Promise<ActionOutcome | null> {
    return this.#run("accept", contact, ["contacts"], async () => {
      const text = `You and ${name} are now contacts.`;
      try {
        const done = await this.#withPlatformGrant({ kind: "acceptRequest" }, (grant) => this.api.dashpayAcceptRequest(this.walletId, this.identity, contact, grant.id));
        return done ? text : null;
      } catch (e) {
        if (hasCode(e, "contact.already_contact")) return text;
        throw e;
      }
    });
  }

  /** Ignores `contact`'s request (D5): free, reversible from Hidden. */
  ignore(contact: string): Promise<ActionOutcome | null> {
    return this.#run("ignore", contact, ["contacts"], async () => {
      await this.api.dashpayIgnoreContact(this.walletId, this.identity, contact);
      return "Request ignored. You can find it under Hidden.";
    });
  }

  /** Unignores `contact` (D5); the cached network search is read again (D6). */
  unignore(contact: string): Promise<ActionOutcome | null> {
    return this.#run("unignore", contact, ["contacts", "search"], async () => {
      await this.api.dashpayUnignoreContact(this.walletId, this.identity, contact);
      return "The request is back in Contact Requests.";
    });
  }

  /** Saves `contact`'s private details, publishing them under a grant (D7). */
  saveDetails(contact: string, details: PrivateContactDetails): Promise<ActionOutcome | null> {
    return this.#run("details", contact, ["contacts"], async () => {
      const state = await this.#withPlatformGrant({ kind: "publishPrivateDetails" }, async (grant) => {
        try {
          return await this.api.dashpaySetPrivateDetails(this.walletId, this.identity, contact, details, grant.id);
        } finally {
          // Unused when nothing was sent (D7); a spent grant's revoke is ignored.
          this.grants.revoke(grant);
        }
      });
      return state ? publishText(state) : null;
    });
  }

  /** Adds the identity's DashPay keys (D10). */
  enable(): Promise<ActionOutcome | null> {
    return this.#run("enable", null, ["identities"], async () => {
      const done = await this.#withPlatformGrant({ kind: "enableDashPayKeys" }, async (grant) => {
        try {
          await this.api.dashpayEnableKeys(this.walletId, this.identity, grant.id);
        } finally {
          // Unused when the keys were there already (a stale identities read).
          this.grants.revoke(grant);
        }
        return true;
      });
      return done ? "DashPay is enabled." : null;
    });
  }

  /** Cancels a grant request still waiting for its passphrase. */
  cancel() {
    this.grants.cancel();
  }

  /** `run` under a `platformOp` grant sized by `action`; `null` when the user cancelled. */
  async #withPlatformGrant<T>(action: GrantAction, run: (grant: AuthGrant) => Promise<T>): Promise<T | null> {
    const since = this.grants.epoch;
    const { maxDuffs, maxCredits } = await this.api.grantRequest(this.walletId, this.identity, action);
    const got = await this.grants.withGrant({ kind: "platformOp", maxDuffs, maxCredits }, this.walletId, run, since);
    return got?.value ?? null;
  }

  /** One action at a time: its outcome, or `null` when it was cancelled or another one runs. */
  async #run(kind: ActionKind, contact: string | null, refresh: Refresh[], act: () => Promise<string | null>): Promise<ActionOutcome | null> {
    if (this.running) return null;
    this.running = { kind, contact };
    this.outcome = null;
    this.changed();
    let outcome: ActionOutcome | null = null;
    try {
      const text = await act();
      if (text) outcome = { kind, contact, ok: true, text };
    } catch (e) {
      outcome = { kind, contact, ok: false, text: actionError(e) };
    } finally {
      this.running = null;
      this.outcome = outcome;
      this.changed();
    }
    // A refusal re-reads too: it may say another surface or device acted first.
    if (outcome) this.refresh(refresh);
    return outcome;
  }
}
