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
  /** The `nextRead` number taken when `relation` arrived, for `currentRelation` (D38). */
  readSeq: number;
}

/** The renderer's read order (D38): a contact-list read takes the next number when it begins, a
 * search hit or a verified code when its relation arrives. Unlike the wall clock and TanStack's
 * `dataUpdatedAt` (when an answer lands), a later number is a later read. */
let reads = 0;
export const nextRead = () => ++reads;

/** A search hit's fields the card reads, with its `nextRead` number (`networkSearchQuery`). */
export type HitFields = Pick<UserHit, "identity" | "username" | "displayName" | "relation" | "unverified"> & { readSeq: number };

export function candidateOfHit(hit: HitFields): Candidate {
  return { ...hitFields(hit), claim: null, source: "search", readSeq: hit.readSeq };
}

export function candidateOfScan(info: ScannedContactInfo, source: CandidateSource, readSeq: number): Candidate {
  const claim = info.code.kind === "autoAcceptClaim" ? { scan: info.code.scan, expiresAt: info.code.expiresAt } : null;
  return { ...hitFields(info), claim, source, readSeq };
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

/** The identity's unfiltered contact list (D14): its rows and the `nextRead` number its read took
 * when it began. */
export interface Listed<R = { contact: string; relation: UserRelation }> {
  rows: R[];
  seq: number;
}

/** The relation the card goes by (D14): the one the identity's unfiltered contact list holds once
 * it is read (a request may have come or gone since the hit or the scan), else the candidate's.
 * An ignore sticks (D34, D38): the engine's eligibility takes an ignored sender for a stranger, so
 * only a list read begun after the candidate's relation arrived may say they are no longer
 * ignored. `listed` is `undefined` until read (`listedNow`). */
export function currentRelation(c: Pick<Candidate, "identity" | "relation" | "readSeq">, listed: Listed | undefined): UserRelation {
  if (listed === undefined || c.relation === "isSelf") return c.relation;
  if (c.relation === "ignored" && listed.seq <= c.readSeq) return "ignored";
  return listed.rows.find((l) => l.contact === c.identity)?.relation ?? "stranger";
}

/** The contact list for `currentRelation` (`contactListQuery`'s answer); `undefined` while a pass
 * holds a list with no rows yet (an empty snapshot says nothing about who relates how). */
export function listedNow<R>(list: { sections: { contacts: R[] }[]; updating: boolean; seq: number } | undefined): Listed<R> | undefined {
  if (!list) return undefined;
  const rows = list.sections.flatMap((s) => s.contacts);
  return list.updating && !rows.length ? undefined : { rows, seq: list.seq };
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

/** Whether the card answers without asking eligibility: oneself, or an ignored sender (the
 * engine's eligibility treats them as a stranger). */
export const answersAtOnce = (relation: UserRelation) => relation === "isSelf" || relation === "ignored";

/** The card's line (D4): oneself and an ignored sender answer at once (`answersAtOnce`); else the
 * eligibility read, `undefined` while it runs (the card says "Checking…"). */
export function cardLine(relation: UserRelation, eligibility: { data: EligibilityName | undefined; failed: boolean }): CardLine | undefined {
  if (relation === "isSelf") return ELIGIBILITY.isSelf;
  if (relation === "ignored") return { text: "You ignored their contact request.", next: "showHidden" };
  if (eligibility.data !== undefined) return eligibilityLine(eligibility.data);
  if (eligibility.failed) return { text: "Could not check this user.", next: "retry" };
  return undefined;
}

/** The engine's `SCAN_MARGIN_SECS`: a claim with this long or less left is refused as expired. */
const SCAN_MARGIN_SECS = 60;

/** When (ms) the engine starts refusing `claim` as expired. */
const claimEnds = (claim: NonNullable<Candidate["claim"]>) => (claim.expiresAt - SCAN_MARGIN_SECS) * 1000;

/** Whether a send can no longer carry `claim` at `now` (ms). */
const claimExpired = (claim: Candidate["claim"], now: number) => claim !== null && now >= claimEnds(claim);

/** The auto-accept note (D7); none once the claim has expired (the card's line says so, D36). */
export function claimNote(claim: Candidate["claim"], now: number): string | null {
  if (!claim || claimExpired(claim, now)) return null;
  const left = claim.expiresAt - Math.floor(now / 1000);
  const when = left < 3600 ? `It expires in ${Math.max(1, Math.round(left / 60))} min.` : `It expires in ${Math.round(left / 3600)} h.`;
  return `This code asks them to accept you automatically. They confirm it when your request arrives. ${when}`;
}

/** The note's re-read interval (ms) while a claim is open (D36). */
export const CLAIM_TICK_MS = 30_000;

/** Calls `tick` while `claim` is open so its note ages (D36): every `CLAIM_TICK_MS`, and at the
 * moment it expires, then no more. Returns the stop. */
export function watchClaim(claim: NonNullable<Candidate["claim"]>, now: () => number, tick: () => void): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const arm = () => {
    const due = claimEnds(claim) - now();
    if (due <= 0) return;
    timer = setTimeout(() => {
      tick();
      arm();
    }, Math.min(due, CLAIM_TICK_MS));
  };
  arm();
  return () => clearTimeout(timer);
}

/** The card's line with a claim (D36): an expired one replaces Send, which could no longer carry it. */
export function claimLine(line: CardLine | undefined, claim: Candidate["claim"], now: number): CardLine | undefined {
  return line?.next === "send" && claimExpired(claim, now) ? { text: SCAN_EXPIRED, next: null } : line;
}

/** The text of a `dashpay://user` link (D13), in `DashPayUserLink::to_uri`'s form. */
export function userLinkText(identity: string, username: string): string {
  return `dashpay://user?id=${encodeURIComponent(identity)}&username=${encodeURIComponent(username)}`;
}

export const SCAN_UNVERIFIED = "This QR code couldn't be verified on the Dash network.";
const LINK_UNVERIFIED = "This link couldn't be verified on the Dash network.";
export const SCAN_EXPIRED = "This QR code has expired. Ask for a new one.";
export const NOT_A_CODE = "This is not a DashPay contact code.";
export const NO_CODE_IN_IMAGE = "There is no DashPay contact code in this image.";
const PLATFORM_DOWN = "Could not reach Dash Platform.";

const isNetwork = (e: unknown) => hasCode(e, "platform.unavailable") || hasCode(e, "platform.timeout");

/** A verification refusal (D6); `retry` after a network failure. */
export function verifyError(e: unknown): { text: string; retry: boolean } {
  if (hasCode(e, "contact.scan_unverified")) return { text: SCAN_UNVERIFIED, retry: false };
  if (hasCode(e, "contact.scan_expired")) return { text: SCAN_EXPIRED, retry: false };
  if (hasCode(e, "invalid_argument")) return { text: NOT_A_CODE, retry: false };
  if (isNetwork(e)) return { text: PLATFORM_DOWN, retry: true };
  return { text: message(e), retry: false };
}

const IMAGE_TOO_LARGE = "The image is too large.";
/** `fileErrorText`'s `app.file_read`: a dropped file the webview could not read (D35). */
const FILE_UNREADABLE = "The file could not be read.";

/** A dropped or pasted file's refusal, said as its message. */
class Refused extends Error {}

/** An image refusal (D9): the decoder's, the screen capture's, the file's (U-05, D35). */
export function imageError(e: unknown): string {
  if (e instanceof Refused) return e.message;
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

/** A scan's progress: reading an image or the screen, verifying, or its refusal; `source` says
 * where the screen shows it (a link's on the card, a scan's on the Scan tab). */
export type ScanState =
  | { kind: "idle" }
  | { kind: "reading" }
  | { kind: "capturing" }
  | { kind: "verifying"; source: CandidateSource }
  | { kind: "failed"; source: CandidateSource; text: string; retry: boolean };

/** The Scan tab's progress line, if any. */
export function scanStatus(scan: ScanState): string | null {
  if (scan.kind === "reading") return "Reading the image…";
  if (scan.kind === "capturing") return "Select a region of the screen…";
  if (scan.kind === "verifying" && scan.source === "scan") return "Checking the code on the Dash network…";
  return null;
}

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
  pick(hit: HitFields) {
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
    return this.#verify([t], "scan", false);
  }

  /** A `dashpay://user` link's identity and (unverified) username (D13). */
  verifyLink(identity: string, username: string): Promise<void> {
    return this.#verify([userLinkText(identity, username)], "link", false);
  }

  /** Open Image… (D9); a cancelled dialog changes nothing. */
  openImage(): Promise<void> {
    return this.#image("reading", () => this.api.scanQrFile());
  }

  /** A dropped or pasted image file (D9): one too large is refused unread, and its bytes are
   * checked again once read (D35). */
  imageFile(file: Pick<Blob, "size" | "arrayBuffer">): Promise<void> {
    if (file.size > MAX_IMAGE_BYTES) {
      this.#begin();
      this.#refuse("scan", IMAGE_TOO_LARGE);
      return Promise.resolve();
    }
    return this.#image("reading", async () => {
      const buffer = await file.arrayBuffer().catch(() => {
        throw new Refused(FILE_UNREADABLE);
      });
      if (buffer.byteLength > MAX_IMAGE_BYTES) throw new Refused(IMAGE_TOO_LARGE);
      return this.api.scanQrImage(bytesToBase64(new Uint8Array(buffer)));
    });
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
    return this.#verify(r.texts, r.source, r.image);
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

  #refuse(source: CandidateSource, text: string, retry = false) {
    this.scan = { kind: "failed", source, text, retry };
    this.changed();
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
      // The read is over: nothing is left to cancel.
      this.scan = { kind: "idle" };
      this.#begin();
      this.#refuse("scan", imageError(e));
      return;
    }
    if (seq !== this.#seq) return;
    this.scan = { kind: "idle" };
    if (texts === null) {
      this.changed();
      return;
    }
    await this.#verify(texts, "scan", true);
  }

  /** D8: `texts` in order, the first that verifies wins; a text that is no contact code is skipped,
   * any other refusal stops there. */
  async #verify(texts: string[], source: CandidateSource, image: boolean) {
    const seq = this.#begin();
    this.scan = { kind: "verifying", source };
    this.changed();
    for (let i = 0; i < texts.length; i++) {
      try {
        const info = await this.api.dashpayVerifyScanned(this.walletId, texts[i]);
        if (seq !== this.#seq) return;
        this.candidate = candidateOfScan(info, source, nextRead());
        this.scan = { kind: "idle" };
        this.changed();
        return;
      } catch (e) {
        if (seq !== this.#seq) return;
        if (hasCode(e, "invalid_argument")) continue;
        const { text: said, retry } = verifyError(e);
        const text = source === "link" && said === SCAN_UNVERIFIED ? LINK_UNVERIFIED : said;
        this.#retry = retry ? { texts: texts.slice(i), source, image } : null;
        this.#refuse(source, text, retry);
        return;
      }
    }
    this.#refuse(source, image ? NO_CODE_IN_IMAGE : NOT_A_CODE);
  }
}
