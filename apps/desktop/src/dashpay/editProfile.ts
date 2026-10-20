// Edit Profile (DASHPAY §4 `EditProfileViewModel`, `AvatarPickerViewModel`; F17; DP4-03): the
// form against the profile it was opened on, the picture's sources (a cropped file, a public URL,
// a Gravatar), the save under a `platformOp` grant, and the unsaved-changes guard. One model per
// wallet, kept while the app runs (D1): a save goes on when the page is left. Plain TypeScript;
// the sheet is screens/dashpay/EditProfileSheet.tsx.

import { errorMessage, hasCode, type Backend } from "../api/backend";
import type { AvatarCandidateInfo } from "../api/generated/AvatarCandidateInfo";
import type { AvatarSourceArgs } from "../api/generated/AvatarSourceArgs";
import type { DashPayProfile } from "../api/generated/DashPayProfile";
import type { DashPayProfileLimits } from "../api/generated/DashPayProfileLimits";
import type { ProfileEditArgs } from "../api/generated/ProfileEditArgs";
import { base64ToBytes, bytesToBase64 } from "../app/files";
import { Model } from "../onboarding/model";
import { GrantModel } from "../tools/grant";
import { cropRect, initialView, type CropView } from "./crop";

export type EditProfileApi = Pick<
  Backend,
  | "grantRequest"
  | "dashpayUpdateProfile"
  | "dashpayPrepareAvatar"
  | "dashpayUploadAvatar"
  | "openFile"
  | "authorize"
  | "revokeGrant"
  | "lockStatus"
  | "paymentAuthentication"
  | "unlock"
  | "unlockMixingOnly"
>;

/** D7: the contract's lengths until `profile_limits` answers (DP4-01 D3's own fallback). */
export const DEFAULT_LIMITS: DashPayProfileLimits = { displayNameMax: 25, publicMessageMax: 140 };
/** DP4-02: the largest file `prepare_avatar` takes (D16). */
export const MAX_FILE_BYTES = 5 << 20;
/** DP4-02's decoder refuses a picture wider or higher than this (`avatar_image.rs`). */
export const MAX_PICTURE_SIDE = 4096;

export const EDIT_COPY = {
  title: "Edit Profile",
  displayName: "Display Name",
  aboutMe: "About me",
  count: (n: number, max: number) => `${n}/${max} characters`,
  tooLong: (max: number) => `At most ${max} characters.`,
  save: "Save Changes",
  saving: "Saving…",
  uploading: "Uploading your picture to the network",
  cancel: "Cancel",
  saved: "Your profile was saved.",
  empty: "A profile needs a display name, an about text or a picture.",
  picture: {
    change: "Change picture",
    chooseFile: "Choose File…",
    url: "Public URL",
    gravatar: "Gravatar",
    remove: "Remove picture",
    undo: "Undo",
    isNew: "New picture",
    isRemoved: "Picture removed",
    drop: "Drop or paste a picture here",
    noUpload: "Pictures from this computer need an upload service this version does not have yet. Use a public URL or Gravatar.",
    off: "Profile pictures are off. Turn on “Load contact pictures” in Settings to choose a new picture.",
    openSettings: "Open Settings",
  },
  url: {
    title: "Public URL",
    text: "You can specify any URL which is publicly available on the internet so users can see it on the Dash network.",
    field: "Paste your image URL",
    fetch: "Fetch Image",
    fetching: "Fetching Image",
  },
  gravatar: {
    title: "Gravatar",
    field: "Enter your Gravatar Email ID",
    placeholder: "sample@email.com",
    disclaimer:
      "Your Email is not stored in the DashPay wallet nor on any servers. It is used once to get your Gravatar account details and then discarded.",
    fetch: "Fetch Gravatar",
    fetching: "Fetching your Gravatar",
  },
  crop: {
    title: "Move and zoom your photo to find the perfect fit",
    zoom: "Zoom",
    use: "Use Photo",
    preparing: "Preparing your picture…",
    frame: "Picture crop. Drag, or use the arrow keys to move it and + or − to zoom.",
  },
  back: "Back",
  discard: {
    title: "Discard your changes?",
    text: "You made changes to your profile that are not saved.",
    keep: "Keep Editing",
    confirm: "Discard",
  },
  errors: {
    tooLarge: "This picture is too large. Choose one under 5 MB.",
    tooManyPixels: `This picture is too large. Choose one at most ${MAX_PICTURE_SIDE} pixels wide and high.`,
    unsupported: "Use a PNG, JPEG or GIF picture.",
    unreadable: "This picture could not be read.",
    urlFailed: "Unable to fetch image. Please enter a valid image URL.",
    urlInvalid: "Enter the picture's https:// address.",
    gravatarFailed: "Unable to fetch your Gravatar. Please enter a valid Gravatar email ID.",
    emailInvalid: "Enter a valid email address.",
    uploadFailed: "Unable to upload your picture. Please try again.",
    uploadUnconfigured: "Uploading pictures is not available in this version.",
    candidateGone: "The picture you chose is no longer ready. Choose it again.",
    conflict: "Your profile was changed on another device since you opened it. Save again to replace it with your changes, or cancel to keep the new one.",
    conflictMatches: "Your profile was changed on another device since you opened it, and it now matches your changes.",
    unsupportedContract: "This version cannot edit profiles on the network's DashPay contract. Update Dash Wallet.",
    credits: "Not enough credits. Top up your credits and try again.",
    unavailable: "Could not reach Dash Platform. Try again.",
    unlock: "Unlock the wallet and try again.",
    context: "Waiting for the network to sync and verify this identity. Try again shortly.",
    trust: "This identity's data could not be verified against the network. Nothing was changed.",
    grant: "The cost went above what you approved.",
    dialog: "The file dialog could not be shown.",
    fileRead: "The file could not be read.",
  },
} as const;

/** What the save does to the picture. */
export type PictureSource = "file" | "url" | "gravatar";

export type PictureChoice = { kind: "keep" } | { kind: "remove" } | { kind: "new"; source: PictureSource; candidate: AvatarCandidateInfo };

export type EditStep = "form" | "url" | "gravatar" | "crop";

/** A file being cropped (D4, D16): its bytes for the engine, a `data:` URL for the frame. */
export interface CropFile {
  bytes: string;
  src: string;
}

/** The text as the engine keeps it: trimmed, empty is none (DP4-01). */
const norm = (s: string | null | undefined): string => (s ?? "").trim();

/** D7: Unicode scalar values, as the contract's `maxLength` counts. */
export function charCount(s: string): number {
  return Array.from(norm(s)).length;
}

/** D16: the picture's format by its first bytes; `null` for anything but PNG, JPEG or GIF. */
export function sniffImage(bytes: Uint8Array): "image/png" | "image/jpeg" | "image/gif" | null {
  const starts = (sig: number[]) => sig.every((b, i) => bytes[i] === b);
  if (starts([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])) return "image/png";
  if (starts([0xff, 0xd8, 0xff])) return "image/jpeg";
  if (starts([0x47, 0x49, 0x46, 0x38]) && (bytes[4] === 0x37 || bytes[4] === 0x39) && bytes[5] === 0x61) return "image/gif";
  return null;
}

/** D9: a fetch's and an argument's refusal, by where the picture came from. */
const BY_SOURCE = {
  file: { fetch: EDIT_COPY.errors.unreadable, invalid: EDIT_COPY.errors.unreadable },
  url: { fetch: EDIT_COPY.errors.urlFailed, invalid: EDIT_COPY.errors.urlInvalid },
  gravatar: { fetch: EDIT_COPY.errors.gravatarFailed, invalid: EDIT_COPY.errors.emailInvalid },
} as const;

/** D9: why a picture could not be prepared, by where it came from. */
export function prepareErrorText(e: unknown, source: PictureSource): string {
  const E = EDIT_COPY.errors;
  if (hasCode(e, "avatar.too_large") || hasCode(e, "app.file_too_large")) return E.tooLarge;
  if (hasCode(e, "avatar.unsupported")) return E.unsupported;
  if (hasCode(e, "app.file_dialog")) return E.dialog;
  if (hasCode(e, "app.file_read")) return E.fileRead;
  if (hasCode(e, "avatar.fetch_failed")) return BY_SOURCE[source].fetch;
  if (hasCode(e, "invalid_argument")) return BY_SOURCE[source].invalid;
  return errorMessage(e);
}

/** The engine's refusals of a picture it no longer holds ready (DP4-02 `avatar.rs`: a candidate
 * kept 30 minutes, or one that never got its upload); any other `invalid_argument` is the
 * engine's own text, the choice kept (D9). */
export const CANDIDATE_GONE = ["unknown avatar candidate", "the avatar candidate has no URL; upload it first"] as const;

const candidateGone = (e: unknown): boolean => hasCode(e, "invalid_argument") && CANDIDATE_GONE.some((detail) => e.message.endsWith(detail));

/** D9: why a save was refused; the edits stay. */
export function saveErrorText(e: unknown): string {
  const E = EDIT_COPY.errors;
  if (hasCode(e, "profile.too_long") && e.data?.kind === "profileTooLong")
    return e.data.field === "displayName" ? `The display name can be at most ${e.data.max} characters.` : `About me can be at most ${e.data.max} characters.`;
  if (hasCode(e, "profile.empty")) return EDIT_COPY.empty;
  if (hasCode(e, "profile.conflict")) return E.conflict;
  if (hasCode(e, "profile.unsupported_contract")) return E.unsupportedContract;
  if (hasCode(e, "platform.insufficient_credits")) return E.credits;
  if (hasCode(e, "platform.unavailable") || hasCode(e, "platform.timeout")) return E.unavailable;
  if (hasCode(e, "vault.locked") || hasCode(e, "platform.signer_unavailable")) return E.unlock;
  if (hasCode(e, "platform.context_unavailable")) return E.context;
  if (hasCode(e, "platform.trust_mismatch")) return E.trust;
  if (hasCode(e, "platform.grant_exceeded")) return E.grant;
  return errorMessage(e);
}

/** The discard question (D11): from the sheet's own close, or a navigation it holds. */
export type Asking = { kind: "close" } | { kind: "leave"; go: () => void };

export class EditProfileModel extends Model {
  open = false;
  step: EditStep = "form";
  identity: string | null = null;
  /** D6: what the edit is made against; `profile.conflict` moves it (D10). */
  baseline: DashPayProfile | null = null;
  displayName = "";
  publicMessage = "";
  picture: PictureChoice = { kind: "keep" };
  limits: DashPayProfileLimits = DEFAULT_LIMITS;
  /** Set by the sheet from the engine (D5, D13). */
  uploadAvailable = false;
  picturesOn = false;
  /** The URL step's field. */
  url = "";
  /** The Gravatar step's field (D15): cleared when its fetch starts. */
  email = "";
  /** The crop step's file and frame (D4); the view is set once the picture has loaded. */
  file: CropFile | null = null;
  crop: CropView | null = null;
  /** A picture is being prepared (U3, G1's fetch, K2). */
  preparing = false;
  /** Why the last picture was refused, said in the form (file) or its step (URL, Gravatar). */
  pictureError: string | null = null;
  /** The save under way: the upload, then the publish (E5). */
  saving: "upload" | "publish" | null = null;
  /** The last save's refusal (E7); a new save or edit of the form keeps it until the next save. */
  error: string | null = null;
  asking: Asking | null = null;
  readonly grants: GrantModel;
  /** Moves on every reset and step change: a prepare that answers after it is dropped. */
  #seq = 0;

  constructor(
    private readonly api: EditProfileApi,
    readonly walletId: string,
    private readonly notify: (text: string) => void = () => undefined,
  ) {
    super();
    this.grants = new GrantModel(api);
    this.grants.subscribe(() => this.changed());
  }

  /** Opens the sheet for `identity` against `profile` (D6); a sheet still open for it (a save
   * that ran while the page was away, D1) is shown as it is. */
  begin(identity: string, profile: DashPayProfile | null) {
    if (this.open && this.identity === identity) return;
    this.#reset();
    this.open = true;
    this.identity = identity;
    this.baseline = profile;
    this.displayName = profile?.displayName ?? "";
    this.publicMessage = profile?.publicMessage ?? "";
    this.changed();
  }

  /** A published picture, even one whose URL the library dropped (its hash stays). */
  get hasPicture(): boolean {
    return !!(this.baseline?.avatarUrl || this.baseline?.avatarHash);
  }

  /** D11: something differs from the baseline. */
  get dirty(): boolean {
    return (
      norm(this.displayName) !== norm(this.baseline?.displayName) ||
      norm(this.publicMessage) !== norm(this.baseline?.publicMessage) ||
      (this.picture.kind === "new" || (this.picture.kind === "remove" && this.hasPicture))
    );
  }

  get nameOver(): boolean {
    return charCount(this.displayName) > this.limits.displayNameMax;
  }

  get messageOver(): boolean {
    return charCount(this.publicMessage) > this.limits.publicMessageMax;
  }

  /** D8: nothing would be left (the engine's `profile.empty`). */
  get empty(): boolean {
    const picture = this.picture.kind === "new" || (this.picture.kind === "keep" && this.hasPicture);
    return !norm(this.displayName) && !norm(this.publicMessage) && !picture;
  }

  /** D8 */
  get canSave(): boolean {
    return this.open && this.saving === null && !this.preparing && this.step === "form" && this.dirty && !this.nameOver && !this.messageOver && !this.empty;
  }

  /** The picture sources may be offered (D5, D13): files only with an upload host. */
  canUse(source: PictureSource): boolean {
    if (!this.picturesOn || this.saving !== null || this.preparing) return false;
    return source !== "file" || this.uploadAvailable;
  }

  setLimits(limits: DashPayProfileLimits | undefined) {
    const next = limits ?? DEFAULT_LIMITS;
    if (next.displayNameMax === this.limits.displayNameMax && next.publicMessageMax === this.limits.publicMessageMax) return;
    this.limits = next;
    this.changed();
  }

  /** The engine's answers the sources depend on (D5, D13). */
  setAvailability(uploadAvailable: boolean, picturesOn: boolean) {
    if (uploadAvailable === this.uploadAvailable && picturesOn === this.picturesOn) return;
    this.uploadAvailable = uploadAvailable;
    this.picturesOn = picturesOn;
    this.changed();
  }

  setDisplayName(text: string) {
    if (this.saving) return;
    this.displayName = text;
    this.changed();
  }

  setPublicMessage(text: string) {
    if (this.saving) return;
    this.publicMessage = text;
    this.changed();
  }

  setUrl(text: string) {
    this.url = text;
    this.changed();
  }

  setEmail(text: string) {
    this.email = text;
    this.changed();
  }

  removePicture() {
    if (this.saving || !this.hasPicture) return;
    this.picture = { kind: "remove" };
    this.pictureError = null;
    this.changed();
  }

  undoPicture() {
    if (this.saving) return;
    this.picture = { kind: "keep" };
    this.pictureError = null;
    this.changed();
  }

  openStep(step: Exclude<PictureSource, "file">) {
    if (!this.canUse(step)) return;
    this.#toStep(step);
  }

  /** Back to the form from a picture step; what it was preparing is dropped. */
  back() {
    this.#toStep("form");
  }

  /** Choose File… (D3): the native dialog, then the crop; a cancelled dialog changes nothing. */
  async chooseFile(): Promise<void> {
    if (!this.canUse("file")) return;
    const seq = this.#seq;
    this.#refuse(null);
    try {
      const file = await this.api.openFile("avatarImage");
      if (file && seq === this.#seq) this.#crop(base64ToBytes(file.data));
    } catch (e) {
      if (seq === this.#seq) this.#refuse(prepareErrorText(e, "file"));
    }
  }

  /** A dropped or pasted picture (D2): one too large is refused unread (D16). */
  async takeFile(file: Pick<Blob, "size" | "arrayBuffer">): Promise<void> {
    if (!this.canUse("file")) return;
    const seq = this.#seq;
    if (file.size > MAX_FILE_BYTES) return this.#refuse(EDIT_COPY.errors.tooLarge);
    this.#refuse(null);
    try {
      const buffer = await file.arrayBuffer();
      if (seq === this.#seq) this.#crop(new Uint8Array(buffer));
    } catch {
      if (seq === this.#seq) this.#refuse(EDIT_COPY.errors.fileRead);
    }
  }

  /** The crop step's picture has loaded: its frame starts at the whole shorter side. */
  pictureLoaded(width: number, height: number) {
    if (this.step !== "crop" || this.crop || width < 1 || height < 1) return;
    // The engine would refuse it at Use Photo, after the user framed it.
    if (width > MAX_PICTURE_SIDE || height > MAX_PICTURE_SIDE) {
      this.#toStep("form");
      this.#refuse(EDIT_COPY.errors.tooManyPixels);
      return;
    }
    this.crop = initialView(width, height);
    this.changed();
  }

  /** The webview could not draw the file (D16). */
  pictureFailed() {
    if (this.step !== "crop") return;
    this.#toStep("form");
    this.#refuse(EDIT_COPY.errors.unreadable);
  }

  setCrop(view: CropView) {
    if (this.step !== "crop" || this.preparing) return;
    this.crop = view;
    this.changed();
  }

  /** Use Photo (K1): the cropped square goes to the engine, which re-encodes it (D4). */
  async useCrop(): Promise<void> {
    const { file, crop } = this;
    if (this.step !== "crop" || !file || !crop) return;
    await this.#prepare("file", { kind: "file", bytes: file.bytes, crop: cropRect(crop) });
  }

  async fetchUrl(): Promise<void> {
    const url = this.url.trim();
    if (this.step !== "url" || !url) return;
    await this.#prepare("url", { kind: "url", url });
  }

  /** D15: the address leaves the model as the call starts. */
  async fetchGravatar(): Promise<void> {
    const email = this.email.trim();
    if (this.step !== "gravatar" || !email || this.preparing || !this.canUse("gravatar")) return;
    this.email = "";
    await this.#prepare("gravatar", { kind: "gravatar", email });
  }

  /** Save Changes (D8): the upload a file's picture needs (D5), then the whole profile under a
   * grant. One at a time; a refusal keeps every edit (D9). */
  async save(): Promise<void> {
    if (!this.canSave || this.identity === null) return;
    const identity = this.identity;
    // A leave while uploading or sizing the grant cancels the passphrase prompt to come (D11).
    const since = this.grants.epoch;
    this.saving = this.#uploadNeeded() ? "upload" : "publish";
    this.error = null;
    this.changed();
    try {
      if (!(await this.#upload())) return;
      const edit = this.#edit();
      const { maxDuffs, maxCredits } = await this.api.grantRequest(this.walletId, identity, { kind: "updateProfile" });
      const got = await this.grants.withGrant(
        { kind: "platformOp", maxDuffs, maxCredits },
        this.walletId,
        async (grant) => {
          try {
            return await this.api.dashpayUpdateProfile(this.walletId, identity, edit, grant.id);
          } finally {
            // Unused when nothing differed from Platform's; a spent grant's revoke is ignored.
            this.grants.revoke(grant);
          }
        },
        since,
      );
      if (!got) return;
      this.#reset();
      this.notify(EDIT_COPY.saved);
    } catch (e) {
      // D10: the other device's profile is what the next save is made against.
      if (hasCode(e, "profile.conflict") && e.data?.kind === "profileConflict") {
        this.baseline = e.data.current;
        // Nothing is left to save when the other device's profile is already the user's.
        if (!this.dirty) this.error = EDIT_COPY.errors.conflictMatches;
      }
      if (!this.error && !this.#candidateGone(e)) this.error = saveErrorText(e);
    } finally {
      this.saving = null;
      this.changed();
    }
  }

  /** ×, Esc, the scrim, Cancel (D11): a question when there are changes; nothing while saving. */
  requestClose() {
    if (this.saving) return;
    if (this.dirty) {
      this.asking = { kind: "close" };
      this.changed();
    } else this.close();
  }

  /** The leave guard (D11): `true` when it holds the navigation for the question. Without
   * changes the sheet closes and the navigation goes on; a save in flight goes on too (D1). */
  requestLeave(go: () => void): boolean {
    if (!this.open) return false;
    if (this.saving) {
      // A passphrase asked for now or later would wait unseen: the save ends there, its edits
      // kept; a publish already sent goes on.
      this.grants.cancel();
      return false;
    }
    if (!this.dirty) {
      this.close();
      return false;
    }
    this.asking = { kind: "leave", go };
    this.changed();
    return true;
  }

  keepEditing() {
    this.asking = null;
    this.changed();
  }

  /** Discard: the edits go, and so does the navigation the question held. */
  discard() {
    const asking = this.asking;
    this.close();
    if (asking?.kind === "leave") asking.go();
  }

  close() {
    if (this.saving) return;
    this.#reset();
    this.changed();
  }

  #reset() {
    this.#seq++;
    this.open = false;
    this.step = "form";
    this.identity = null;
    this.baseline = null;
    this.displayName = "";
    this.publicMessage = "";
    this.picture = { kind: "keep" };
    this.url = "";
    this.email = "";
    this.file = null;
    this.crop = null;
    this.preparing = false;
    this.pictureError = null;
    this.error = null;
    this.asking = null;
    this.grants.cancel();
  }

  #toStep(step: EditStep) {
    this.#seq++;
    this.step = step;
    this.preparing = false;
    this.pictureError = null;
    this.file = null;
    this.crop = null;
    this.url = "";
    this.email = "";
    this.changed();
  }

  /** Says why a picture was refused (`null`: clears it). */
  #refuse(text: string | null) {
    this.pictureError = text;
    this.changed();
  }

  #crop(bytes: Uint8Array) {
    if (bytes.byteLength > MAX_FILE_BYTES) return this.#refuse(EDIT_COPY.errors.tooLarge);
    const type = sniffImage(bytes);
    if (!type) return this.#refuse(EDIT_COPY.errors.unsupported);
    const b64 = bytesToBase64(bytes);
    this.#toStep("crop");
    this.file = { bytes: b64, src: `data:${type};base64,${b64}` };
    this.changed();
  }

  async #prepare(source: PictureSource, args: AvatarSourceArgs): Promise<void> {
    if (this.preparing || !this.canUse(source)) return;
    const seq = this.#seq;
    this.preparing = true;
    this.pictureError = null;
    this.changed();
    try {
      const candidate = await this.api.dashpayPrepareAvatar(this.walletId, args);
      if (seq !== this.#seq) return;
      this.#toStep("form");
      this.picture = { kind: "new", source, candidate };
    } catch (e) {
      if (seq !== this.#seq) return;
      this.preparing = false;
      // A file's refusal is said in the form, where the file was chosen (K3).
      if (source === "file") this.#toStep("form");
      this.pictureError = prepareErrorText(e, source);
    }
    this.changed();
  }

  #uploadNeeded(): boolean {
    const p = this.picture;
    return p.kind === "new" && p.candidate.needsUpload && p.candidate.url === null;
  }

  /** D5: a file's candidate is uploaded once; the URL it gets is kept for a retry. `false`
   * when the upload was refused (said in `error`). */
  async #upload(): Promise<boolean> {
    const p = this.picture;
    if (p.kind !== "new" || !this.#uploadNeeded()) return true;
    let url: string;
    try {
      url = await this.api.dashpayUploadAvatar(this.walletId, p.candidate.id);
    } catch (e) {
      if (!this.#candidateGone(e))
        this.error = hasCode(e, "avatar.upload_unconfigured")
          ? EDIT_COPY.errors.uploadUnconfigured
          : hasCode(e, "invalid_argument")
            ? errorMessage(e)
            : EDIT_COPY.errors.uploadFailed;
      return false;
    }
    this.picture = { ...p, candidate: { ...p.candidate, url, needsUpload: false } };
    this.saving = "publish";
    this.changed();
    return true;
  }

  /** A new picture's candidate the engine no longer knows (it lives 30 minutes): the choice
   * goes back to the published picture and the form says to choose it again; no retry could
   * use it. */
  #candidateGone(e: unknown): boolean {
    if (this.picture.kind !== "new" || !candidateGone(e)) return false;
    this.picture = { kind: "keep" };
    this.error = EDIT_COPY.errors.candidateGone;
    return true;
  }

  #edit(): ProfileEditArgs {
    const p = this.picture;
    return {
      displayName: norm(this.displayName) || null,
      publicMessage: norm(this.publicMessage) || null,
      avatar: p.kind === "new" ? { kind: "set", candidate: p.candidate.id } : { kind: p.kind },
      baseline: { profile: this.baseline },
    };
  }
}

const models = new Map<string, EditProfileModel>();

/** The wallet's Edit Profile, created on first use and kept while the app runs (D1). */
export function editProfileFor(api: EditProfileApi, walletId: string, notify?: (text: string) => void): EditProfileModel {
  let m = models.get(walletId);
  if (!m) {
    m = new EditProfileModel(api, walletId, notify);
    models.set(walletId, m);
  }
  return m;
}
