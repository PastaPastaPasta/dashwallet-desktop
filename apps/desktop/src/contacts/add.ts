// Add Contact's view model (DP2-08): the candidate a search hit, a scan or a `dashpay://user` link
// shows, the eligibility line that says whether a request can go to it (D4), the scan sources'
// verification (D6, D8, D9, D10) and My QR's states (D12). The send itself is DP2-09's
// `ContactActions.send` (D5). A scanned or pasted text may carry a bearer key (`dapk`, D16): the
// model holds it only while `dashpay_verify_scanned` runs and, after a network failure, for Try
// again; it never enters a query key or the shell's store.

import { hasCode, isAppError, type Backend } from "../api/backend";
import type { EligibilityName } from "../api/generated/EligibilityName";
import type { ScannedContactInfo } from "../api/generated/ScannedContactInfo";
import type { UserHit } from "../api/generated/UserHit";
import type { UserRelation } from "../api/generated/UserRelation";
import { Model } from "../onboarding/model";
import { bytesToBase64, fileErrorText } from "../app/files";
import type { ContactsOwner } from "./model";

export type AddTab = "search" | "scan" | "myQr";

export const ADD_TABS: { id: AddTab; title: string }[] = [
  { id: "search", title: "Search" },
  { id: "scan", title: "Scan" },
  { id: "myQr", title: "My QR" },
];

/** Where a candidate came from (the card says it). */
export type CandidateSource = "search" | "scan" | "link";

/** The user the card shows: a search hit, or what `dashpay_verify_scanned` answered. */
export interface Candidate {
  identity: string;
  username: string | null;
  displayName: string | null;
  relation: UserRelation;
  /** Verified only through the trusted fallback. */
  unverified: boolean;
  /** The auto-accept claim the request answers (D7); its id is no secret. */
  claim: { scan: string; expiresAt: number } | null;
  source: CandidateSource;
}

export function candidateOfHit(hit: Pick<UserHit, "identity" | "username" | "displayName" | "relation" | "unverified">): Candidate {
  return { ...hitFields(hit), claim: null, source: "search" };
}

export function candidateOfScan(info: ScannedContactInfo, source: CandidateSource): Candidate {
  const claim = info.code.kind === "autoAcceptClaim" ? { scan: info.code.scan, expiresAt: info.code.expiresAt } : null;
  return { ...hitFields(info), claim, source };
}

const hitFields = (h: Pick<UserHit, "identity" | "displayName" | "relation" | "unverified"> & { username: string | null }) => ({
  identity: h.identity,
  username: h.username,
  displayName: h.displayName,
  relation: h.relation,
  unverified: h.unverified,
});

/** The card's name: the display name, else `@username`, else the identity. */
export function candidateName(c: Pick<Candidate, "displayName" | "username" | "identity">): string {
  return c.displayName || (c.username ? `@${c.username}` : c.identity);
}

/** The relation the card goes by (D14): the one the identity's unfiltered contact list holds once
 * it is read (a request may have come or gone since the hit or the scan), else the candidate's. */
export function currentRelation(c: Pick<Candidate, "identity" | "relation">, listed: { contact: string; relation: UserRelation }[] | undefined): UserRelation {
  if (listed === undefined || c.relation === "isSelf") return c.relation;
  return listed.find((l) => l.contact === c.identity)?.relation ?? "stranger";
}

/** What the card offers next (D4, D13). */
export type CardNext = "send" | "showContact" | "showRequest" | "showHidden" | "enable" | "join" | "retry" | null;

export interface CardLine {
  /** The eligibility line; `null` with Send (nothing stands in the way). */
  text: string | null;
  next: CardNext;
}

const ELIGIBILITY: Record<Exclude<EligibilityName, "ok">, CardLine> = {
  isSelf: { text: "This is you.", next: null },
  alreadyContact: { text: "Already in your contacts.", next: "showContact" },
  pendingOutgoing: { text: "Contact request sent.", next: "showRequest" },
  pendingIncoming: { text: "They sent you a contact request.", next: "showRequest" },
  noDashPayKeys: { text: "Enable DashPay to send contact requests.", next: "enable" },
  contactCannotReceive: { text: "This user cannot receive contact requests yet.", next: null },
};

/** The line for an eligibility answer (D4). */
export function eligibilityLine(e: EligibilityName): CardLine {
  return e === "ok" ? { text: null, next: "send" } : ELIGIBILITY[e];
}

/** The card's line without a main identity (D13, BL-153's status): Join where the wallet can,
 * else why it cannot yet; `undefined` while DashPay starts. */
export function ownerLine(owner: Exclude<ContactsOwner, { kind: "identity" }>): CardLine | undefined {
  switch (owner.kind) {
    case "join":
      if (owner.reason === "watchOnly") return { text: "A watch-only wallet cannot send contact requests.", next: null };
      if (owner.reason === "waitingForSync") return { text: "You can send contact requests once the wallet has synced.", next: null };
      return { text: "Join DashPay to send contact requests.", next: "join" };
    case "registering":
      return { text: "You can send contact requests once your username is registered.", next: null };
    case "error":
      return { text: "Could not read DashPay's state.", next: null };
    case "starting":
      return undefined;
  }
}

/** The card's line (D4): an ignored sender answers at once (the engine's eligibility treats them
 * as a stranger); else the eligibility read, `undefined` while it runs (the card says
 * "Checking…"). */
export function cardLine(relation: UserRelation, eligibility: { data: EligibilityName | undefined; failed: boolean }): CardLine | undefined {
  if (relation === "isSelf") return ELIGIBILITY.isSelf;
  if (relation === "ignored") return { text: "You ignored their contact request.", next: "showHidden" };
  if (eligibility.data !== undefined) return eligibilityLine(eligibility.data);
  if (eligibility.failed) return { text: "Could not check this user.", next: "retry" };
  return undefined;
}

/** The auto-accept note (D7). */
export function claimNote(claim: Candidate["claim"], now: number): string | null {
  if (!claim) return null;
  const left = claim.expiresAt - Math.floor(now / 1000);
  const when = left <= 0 ? "This code has expired." : left < 3600 ? `It expires in ${Math.max(1, Math.round(left / 60))} min.` : `It expires in ${Math.round(left / 3600)} h.`;
  return `This code asks them to accept you automatically. They confirm it when your request arrives. ${when}`;
}

/** The text of a `dashpay://user` link (D13), in `DashPayUserLink::to_uri`'s form. */
export function userLinkText(identity: string, username: string): string {
  return `dashpay://user?id=${encodeURIComponent(identity)}&username=${encodeURIComponent(username)}`;
}

export const SCAN_UNVERIFIED = "This QR code couldn't be verified on the Dash network.";
export const LINK_UNVERIFIED = "This link couldn't be verified on the Dash network.";
export const SCAN_EXPIRED = "This QR code has expired. Ask for a new one.";
export const NOT_A_CODE = "This is not a DashPay contact code.";
export const NO_CODE_IN_IMAGE = "There is no DashPay contact code in this image.";
export const PLATFORM_DOWN = "Could not reach Dash Platform.";

const isNetwork = (e: unknown) => hasCode(e, "platform.unavailable") || hasCode(e, "platform.timeout");

/** A verification refusal (D6); `retry` after a network failure. */
export function verifyError(e: unknown): { text: string; retry: boolean } {
  if (hasCode(e, "contact.scan_unverified")) return { text: SCAN_UNVERIFIED, retry: false };
  if (hasCode(e, "contact.scan_expired")) return { text: SCAN_EXPIRED, retry: false };
  if (hasCode(e, "invalid_argument")) return { text: NOT_A_CODE, retry: false };
  if (isNetwork(e)) return { text: PLATFORM_DOWN, retry: true };
  return { text: message(e), retry: false };
}

/** An image refusal (D9): the decoder's, the screen capture's, the file's (U-05). */
export function imageError(e: unknown): string {
  if (hasCode(e, "app.image_unreadable")) return "This image could not be read. Use a PNG, JPEG or BMP file.";
  if (hasCode(e, "app.no_qr_code")) return "No QR code was found in this image.";
  if (hasCode(e, "app.screen_capture_unavailable")) return SCREEN_UNAVAILABLE;
  if (hasCode(e, "app.screen_capture")) return "The screen could not be captured. Try again.";
  return fileErrorText(e);
}

export const SCREEN_UNAVAILABLE = "This desktop has no screen capture. Take a screenshot, then open or paste it here.";
/** macOS's way to a region (D10, F1). */
export const SCREEN_SHORTCUT = "Press Control-Shift-Command-4 to copy a region of the screen, then paste it here.";

const message = (e: unknown) => (isAppError(e) || e instanceof Error ? e.message : String(e));

/** My QR (D12): the link, or why there is none. */
export type MyQr = { kind: "loading" } | { kind: "link"; link: string } | { kind: "noUsername" } | { kind: "updating" } | { kind: "failed"; text: string };

export function myQr(q: { data: string | undefined; error: unknown; isError: boolean }): MyQr {
  if (q.data !== undefined) return { kind: "link", link: q.data };
  if (!q.isError) return { kind: "loading" };
  if (hasCode(q.error, "contact.no_username")) return { kind: "noUsername" };
  if (isNetwork(q.error)) return { kind: "updating" };
  return { kind: "failed", text: message(q.error) };
}

/** dw-desktop's `qr::MAX_IMAGE_BYTES`: a larger drop or paste is refused before it crosses IPC. */
export const MAX_IMAGE_BYTES = 32 * 1024 * 1024;

export type ScanApi = Pick<Backend, "dashpayVerifyScanned" | "scanQrFile" | "scanQrImage" | "scanQrScreen" | "cancelScreenScan">;

/** A scan's progress: reading an image or the screen, verifying (with the link's unverified
 * username while a link's runs, D13), or its refusal; `source` says where the screen shows it (a
 * link's on the card, a scan's on the Scan tab). */
export type ScanState =
  | { kind: "idle" }
  | { kind: "reading" }
  | { kind: "capturing" }
  | { kind: "verifying"; source: CandidateSource; username: string | null }
  | { kind: "failed"; source: CandidateSource; text: string; retry: boolean };

/** The candidate and the scans of one Add Contact (one wallet). The latest request wins: an older
 * answer that lands later is dropped, and a new one drops the candidate it would replace. */
export class AddContactModel extends Model {
  candidate: Candidate | null = null;
  scan: ScanState = { kind: "idle" };
  /** Bumped by every new request and by `clear`. */
  #seq = 0;
  /** Try again's texts after a network failure (D16: held for nothing else). */
  #retry: { texts: string[]; source: CandidateSource; image: boolean } | null = null;

  constructor(
    private readonly api: ScanApi,
    readonly walletId: string,
  ) {
    super();
  }

  /** A search hit: Platform's answer already, nothing to verify. */
  pick(hit: UserHit) {
    this.#begin();
    this.candidate = candidateOfHit(hit);
    this.changed();
  }

  /** Drops the candidate and any scan under way. */
  clear() {
    this.#begin();
    this.changed();
  }

  /** A pasted or dropped text (D9). */
  verifyText(text: string): Promise<void> {
    const t = text.trim();
    if (!t) return Promise.resolve();
    return this.#verify([t], "scan", false, null);
  }

  /** A `dashpay://user` link's identity and (unverified) username (D13). */
  verifyLink(identity: string, username: string): Promise<void> {
    return this.#verify([userLinkText(identity, username)], "link", false, username);
  }

  /** Open Image… (D9); a cancelled dialog changes nothing. */
  openImage(): Promise<void> {
    return this.#image("reading", () => this.api.scanQrFile());
  }

  /** A dropped or pasted image, base64 (D9). */
  image(data: string): Promise<void> {
    return this.#image("reading", () => this.api.scanQrImage(data));
  }

  /** A dropped or pasted image's bytes (D9). */
  imageBytes(bytes: Uint8Array): Promise<void> {
    if (bytes.length <= MAX_IMAGE_BYTES) return this.image(bytesToBase64(bytes));
    this.#begin();
    this.scan = { kind: "failed", source: "scan", text: "The image is too large.", retry: false };
    this.changed();
    return Promise.resolve();
  }

  /** Scan a Screen Region (D10); the user's cancel changes nothing. */
  screen(): Promise<void> {
    return this.#image("capturing", () => this.api.scanQrScreen());
  }

  /** Stops waiting for the region picker. */
  cancelScreen() {
    if (this.scan.kind !== "capturing") return;
    this.#stop();
    this.changed();
  }

  /** Try again after a network failure. */
  retry(): Promise<void> {
    const r = this.#retry;
    if (!r) return Promise.resolve();
    return this.#verify(r.texts, r.source, r.image, null);
  }

  /** Forgets the last scan's refusal. */
  dismiss() {
    if (this.scan.kind !== "failed") return;
    this.#begin();
    this.changed();
  }

  /** A new request (or none): the last one's answer, refusal and candidate go. */
  #begin(): number {
    const seq = this.#stop();
    this.candidate = null;
    return seq;
  }

  /** Stops the last request (a capture waiting too) and forgets its refusal. */
  #stop(): number {
    if (this.scan.kind === "capturing") void this.api.cancelScreenScan().catch(() => undefined);
    this.#retry = null;
    this.scan = { kind: "idle" };
    return ++this.#seq;
  }

  /** An image's codes, verified; the candidate stays until there are some (a cancelled dialog or
   * picker changes nothing). */
  async #image(kind: "reading" | "capturing", read: () => Promise<string[] | null>) {
    const seq = this.#stop();
    this.scan = { kind };
    this.changed();
    let texts: string[] | null;
    try {
      texts = await read();
    } catch (e) {
      if (seq !== this.#seq) return;
      this.#begin();
      this.scan = { kind: "failed", source: "scan", text: imageError(e), retry: false };
      this.changed();
      return;
    }
    if (seq !== this.#seq) return;
    if (texts === null) {
      this.scan = { kind: "idle" };
      this.changed();
      return;
    }
    await this.#verify(texts, "scan", true, null);
  }

  /** D8: `texts` in order, the first that verifies wins; a text that is no contact code is skipped,
   * any other refusal stops there. */
  async #verify(texts: string[], source: CandidateSource, image: boolean, username: string | null) {
    const seq = this.#begin();
    this.scan = { kind: "verifying", source, username };
    this.changed();
    for (let i = 0; i < texts.length; i++) {
      try {
        const info = await this.api.dashpayVerifyScanned(this.walletId, texts[i]);
        if (seq !== this.#seq) return;
        this.candidate = candidateOfScan(info, source);
        this.scan = { kind: "idle" };
        this.changed();
        return;
      } catch (e) {
        if (seq !== this.#seq) return;
        if (hasCode(e, "invalid_argument")) continue;
        const { text: said, retry } = verifyError(e);
        const text = source === "link" && said === SCAN_UNVERIFIED ? LINK_UNVERIFIED : said;
        this.#retry = retry ? { texts: texts.slice(i), source, image } : null;
        this.scan = { kind: "failed", source, text, retry };
        this.changed();
        return;
      }
    }
    this.scan = { kind: "failed", source, text: image ? NO_CODE_IN_IMAGE : NOT_A_CODE, retry: false };
    this.changed();
  }
}
