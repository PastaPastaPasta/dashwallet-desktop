// Edit Profile's view model (DP4-03): what is dirty and savable (D7, D8), one save at a time
// under an updateProfile grant, a refusal keeping the edits (D9) and a conflict moving the
// baseline (D10), the discard question (D11), and the picture sources (D2–D5, D13, D15, D16).

import { describe, expect, it, vi } from "vitest";
import type { AvatarCandidateInfo } from "../api/generated/AvatarCandidateInfo";
import type { AvatarSourceArgs } from "../api/generated/AvatarSourceArgs";
import type { DashPayProfile } from "../api/generated/DashPayProfile";
import { bytesToBase64 } from "../app/files";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { fakeVault } from "../tools/testApi";
import { charCount, EDIT_COPY, EditProfileModel, MAX_FILE_BYTES, prepareErrorText, saveErrorText, sniffImage, type EditProfileApi } from "./editProfile";

const ME = "4EfA9Jrvv3nnCFdSf7fad59851iiTRZ6Wcu6YVJ4iSeF";
const PROFILE: DashPayProfile = {
  displayName: "Alice Liddell",
  publicMessage: "Down the rabbit hole",
  avatarUrl: "https://example.org/alice.png",
  avatarHash: "ab".repeat(32),
  avatarFingerprint: "cd".repeat(8),
  updatedAt: 1_700_000_000_000,
};
const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0]);

const until = async (cond: () => boolean) => {
  for (let i = 0; i < 100 && !cond(); i++) await new Promise((r) => setTimeout(r, 0));
  expect(cond()).toBe(true);
};

const candidate = (id: string, url: string | null): AvatarCandidateInfo => ({ id, preview: { png: "AAAA", size: "large" }, url, needsUpload: url === null });

function setup(overrides: Partial<EditProfileApi> = {}) {
  const { api: vault, calls } = fakeVault();
  let n = 0;
  const api: EditProfileApi = {
    ...vault,
    grantRequest: vi.fn(async () => ({ maxDuffs: 0, maxCredits: "300000000" })),
    dashpayUpdateProfile: vi.fn(async () => null),
    dashpayPrepareAvatar: vi.fn(async (_w: string, s: AvatarSourceArgs) => candidate(`c-${++n}`, s.kind === "file" ? null : `https://example.org/${n}.png`)),
    dashpayUploadAvatar: vi.fn(async () => "https://i.imgur.com/up.png"),
    openFile: vi.fn(async () => ({ name: "me.png", data: bytesToBase64(PNG) })),
    ...overrides,
  };
  const notes: string[] = [];
  const m = new EditProfileModel(api, WALLET_A, (t) => notes.push(t));
  m.setAvailability(false, true);
  m.begin(ME, PROFILE);
  /** Answers the passphrase prompt the save raised. */
  const authorize = async () => {
    await until(() => m.grants.pending !== null);
    expect(await m.grants.submit("pw")).toBe(true);
  };
  return { api, calls, m, notes, authorize };
}

describe("EditProfileModel: the form (D7, D8)", () => {
  it("IOS-076 starts from the profile; only a real change is dirty", () => {
    const { m } = setup();
    expect(m).toMatchObject({ open: true, displayName: "Alice Liddell", publicMessage: "Down the rabbit hole", dirty: false, canSave: false });
    m.setDisplayName("  Alice Liddell ");
    expect(m.dirty).toBe(false);
    m.setDisplayName("Alice");
    expect(m).toMatchObject({ dirty: true, canSave: true });
  });

  it("AND-095 counts characters, not UTF-16 units, against the contract's limits", () => {
    const { m } = setup();
    expect(charCount(" 😀😀 ")).toBe(2);
    m.setDisplayName("😀".repeat(25));
    expect(m).toMatchObject({ nameOver: false, canSave: true });
    m.setDisplayName("😀".repeat(26));
    expect(m).toMatchObject({ nameOver: true, canSave: false });
    m.setDisplayName("Alice");
    m.setLimits({ displayNameMax: 25, publicMessageMax: 10 });
    expect(m).toMatchObject({ messageOver: true, canSave: false });
    m.setLimits(undefined);
    expect(m.messageOver).toBe(false);
  });

  it("an edit leaving nothing is empty and cannot be saved", () => {
    const { m } = setup();
    m.setDisplayName("");
    m.setPublicMessage(" ");
    expect(m.empty).toBe(false); // the picture is kept
    m.removePicture();
    expect(m).toMatchObject({ empty: true, canSave: false });
    m.undoPicture();
    expect(m.picture).toEqual({ kind: "keep" });
  });

  it("removing needs a picture to remove; one whose URL the library dropped still counts", () => {
    const { m } = setup();
    m.close();
    m.begin(ME, { ...PROFILE, avatarUrl: null, avatarHash: null, avatarFingerprint: null });
    m.removePicture();
    expect(m.picture).toEqual({ kind: "keep" });
    m.close();
    m.begin(ME, { ...PROFILE, avatarUrl: null });
    m.removePicture();
    expect(m.picture).toEqual({ kind: "remove" });
  });
});

describe("EditProfileModel: saving (D8–D10)", () => {
  it("IOS-076 saves the whole profile against its baseline under an updateProfile grant", async () => {
    const { api, calls, m, notes, authorize } = setup();
    m.setDisplayName(" Alice ");
    m.setPublicMessage("");
    const done = m.save();
    expect(m.saving).toBe("publish");
    await authorize();
    await done;
    expect(api.grantRequest).toHaveBeenCalledWith(WALLET_A, ME, { kind: "updateProfile" });
    expect(calls.authorize[0].purpose).toEqual({ kind: "platformOp", maxDuffs: 0, maxCredits: "300000000" });
    expect(api.dashpayUpdateProfile).toHaveBeenCalledWith(
      WALLET_A,
      ME,
      { displayName: "Alice", publicMessage: null, avatar: { kind: "keep" }, baseline: { profile: PROFILE } },
      "grant-1",
    );
    expect(calls.revoked).toEqual(["grant-1"]);
    expect(m).toMatchObject({ open: false, saving: null });
    expect(notes).toEqual([EDIT_COPY.saved]);
  });

  it("a save in flight cannot be submitted again, nor its fields edited or the sheet closed", async () => {
    const { api, m, authorize } = setup();
    m.setDisplayName("Alice");
    const first = m.save();
    void m.save();
    expect(m.canSave).toBe(false);
    m.setDisplayName("Changed");
    m.requestClose();
    expect(m).toMatchObject({ displayName: "Alice", asking: null, open: true });
    await authorize();
    await first;
    expect(api.dashpayUpdateProfile).toHaveBeenCalledTimes(1);
  });

  it("AND-096 a refused save keeps every edit and says the engine's reason", async () => {
    const { m, authorize } = setup({ dashpayUpdateProfile: vi.fn(async () => Promise.reject(appError("platform.unavailable"))) });
    m.setDisplayName("Alice");
    m.removePicture();
    const done = m.save();
    await authorize();
    await done;
    expect(m).toMatchObject({ open: true, displayName: "Alice", picture: { kind: "remove" }, saving: null, error: EDIT_COPY.errors.unavailable, canSave: true });
  });

  it("a cancelled passphrase prompt ends the save with the edits and no error", async () => {
    const { api, m } = setup();
    m.setDisplayName("Alice");
    const done = m.save();
    await until(() => m.grants.pending !== null);
    m.grants.cancel();
    await done;
    expect(api.dashpayUpdateProfile).not.toHaveBeenCalled();
    expect(m).toMatchObject({ open: true, displayName: "Alice", saving: null, error: null });
  });

  it("a conflict moves the baseline to the other device's profile; the next save replaces it", async () => {
    const current = { ...PROFILE, publicMessage: "Edited elsewhere", updatedAt: null };
    const update = vi.fn().mockRejectedValueOnce(appError("profile.conflict", { kind: "profileConflict", current })).mockResolvedValueOnce(null);
    const { m, authorize } = setup({ dashpayUpdateProfile: update });
    m.setDisplayName("Alice");
    let done = m.save();
    await authorize();
    await done;
    expect(m).toMatchObject({ baseline: current, error: EDIT_COPY.errors.conflict, displayName: "Alice", publicMessage: "Down the rabbit hole" });
    done = m.save();
    await authorize();
    await done;
    expect(update.mock.calls[1][2].baseline).toEqual({ profile: current });
    expect(m.open).toBe(false);
  });

  it("maps the engine's refusals to their copy (D9)", () => {
    expect(saveErrorText(appError("profile.too_long", { kind: "profileTooLong", field: "publicMessage", length: 141, max: 140 }))).toBe(
      "About me can be at most 140 characters.",
    );
    expect(saveErrorText(appError("profile.empty"))).toBe(EDIT_COPY.empty);
    expect(saveErrorText(appError("platform.insufficient_credits"))).toBe(EDIT_COPY.errors.credits);
    expect(saveErrorText(appError("internal", null, "boom"))).toBe("boom");
    expect(prepareErrorText(appError("avatar.fetch_failed"), "gravatar")).toBe(EDIT_COPY.errors.gravatarFailed);
    expect(prepareErrorText(appError("avatar.fetch_failed"), "url")).toBe(EDIT_COPY.errors.urlFailed);
    expect(prepareErrorText(appError("invalid_argument"), "url")).toBe(EDIT_COPY.errors.urlInvalid);
    expect(prepareErrorText(appError("app.file_too_large"), "file")).toBe(EDIT_COPY.errors.tooLarge);
  });
});

describe("EditProfileModel: leaving with changes (D11)", () => {
  it("AND-097 closing with changes asks; Keep Editing stays, Discard closes", () => {
    const { m } = setup();
    m.setDisplayName("Alice");
    m.requestClose();
    expect(m.asking).toEqual({ kind: "close" });
    m.keepEditing();
    expect(m).toMatchObject({ asking: null, open: true, displayName: "Alice" });
    m.requestClose();
    m.discard();
    expect(m.open).toBe(false);
  });

  it("closing without changes closes at once", () => {
    const { m } = setup();
    m.requestClose();
    expect(m).toMatchObject({ open: false, asking: null });
  });

  it("a navigation with changes is held for the question; Discard lets it go on", () => {
    const { m } = setup();
    const go = vi.fn();
    expect(m.requestLeave(go)).toBe(false); // no changes: closes, the navigation goes on
    expect(m.open).toBe(false);
    m.begin(ME, PROFILE);
    m.setPublicMessage("New");
    expect(m.requestLeave(go)).toBe(true);
    expect(go).not.toHaveBeenCalled();
    m.discard();
    expect(go).toHaveBeenCalledOnce();
    expect(m.requestLeave(go)).toBe(false); // closed: nothing to hold
  });

  it("a save in flight lets a navigation go on and keeps running", async () => {
    const { api, m, authorize } = setup();
    m.setDisplayName("Alice");
    const done = m.save();
    await authorize();
    expect(m.requestLeave(() => undefined)).toBe(false);
    await done;
    expect(api.dashpayUpdateProfile).toHaveBeenCalledOnce();
  });

  it("a navigation while the passphrase is asked for ends the save there, edits kept", async () => {
    const { api, m } = setup();
    m.setDisplayName("Alice");
    const done = m.save();
    await until(() => m.grants.pending !== null);
    expect(m.requestLeave(() => undefined)).toBe(false);
    await done;
    expect(api.dashpayUpdateProfile).not.toHaveBeenCalled();
    expect(m).toMatchObject({ open: true, displayName: "Alice", saving: null });
  });

  it("reopening for the same identity keeps a sheet still open", () => {
    const { m } = setup();
    m.setDisplayName("Alice");
    m.begin(ME, PROFILE);
    expect(m.displayName).toBe("Alice");
  });
});

describe("EditProfileModel: pictures (D2–D5, D13, D15)", () => {
  it("IOS-076 a public URL becomes the new picture; its refusal stays in its step", async () => {
    const prepare = vi.fn().mockRejectedValueOnce(appError("avatar.fetch_failed")).mockResolvedValueOnce(candidate("c-1", "https://example.org/a.png"));
    const { m } = setup({ dashpayPrepareAvatar: prepare });
    m.openStep("url");
    m.setUrl(" https://example.org/missing.png ");
    await m.fetchUrl();
    expect(prepare).toHaveBeenCalledWith(WALLET_A, { kind: "url", url: "https://example.org/missing.png" });
    expect(m).toMatchObject({ step: "url", pictureError: EDIT_COPY.errors.urlFailed, preparing: false });
    m.setUrl("https://example.org/a.png");
    await m.fetchUrl();
    expect(m).toMatchObject({ step: "form", picture: { kind: "new", source: "url" }, dirty: true });
    expect(m.picture.kind === "new" && m.picture.candidate.id).toBe("c-1");
  });

  it("D15 the Gravatar e-mail address leaves the model when its fetch starts", async () => {
    const { api, m } = setup();
    m.openStep("gravatar");
    m.setEmail(" alice@example.org ");
    const done = m.fetchGravatar();
    expect(m).toMatchObject({ email: "", preparing: true, canSave: false });
    await done;
    expect(api.dashpayPrepareAvatar).toHaveBeenCalledWith(WALLET_A, { kind: "gravatar", email: "alice@example.org" });
    expect(m.picture.kind).toBe("new");
  });

  it("an answer arriving after Back is dropped", async () => {
    let answer: (c: AvatarCandidateInfo) => void = () => undefined;
    const { m } = setup({ dashpayPrepareAvatar: vi.fn(() => new Promise<AvatarCandidateInfo>((r) => (answer = r))) });
    m.openStep("url");
    m.setUrl("https://example.org/a.png");
    const done = m.fetchUrl();
    m.back();
    answer(candidate("late", "https://example.org/a.png"));
    await done;
    expect(m).toMatchObject({ step: "form", picture: { kind: "keep" }, preparing: false });
  });

  it("D13 with pictures off no source is offered", async () => {
    const { api, m } = setup();
    m.setAvailability(true, false);
    expect(["file", "url", "gravatar"].map((s) => m.canUse(s as "file"))).toEqual([false, false, false]);
    m.openStep("url");
    await m.chooseFile();
    expect(m.step).toBe("form");
    expect(api.openFile).not.toHaveBeenCalled();
  });

  it("D5 without an upload host a file cannot be chosen", async () => {
    const { api, m } = setup();
    expect(m.canUse("file")).toBe(false);
    await m.chooseFile();
    expect(api.openFile).not.toHaveBeenCalled();
  });

  it("D4 a chosen file is cropped, and only the square goes to the engine", async () => {
    const { api, m } = setup();
    m.setAvailability(true, true);
    await m.chooseFile();
    expect(api.openFile).toHaveBeenCalledWith("avatarImage");
    expect(m.step).toBe("crop");
    expect(m.file?.src).toBe(`data:image/png;base64,${bytesToBase64(PNG)}`);
    m.pictureLoaded(400, 300);
    m.setCrop({ ...m.crop!, zoom: 2, cx: 300 });
    await m.useCrop();
    expect(api.dashpayPrepareAvatar).toHaveBeenCalledWith(WALLET_A, { kind: "file", bytes: bytesToBase64(PNG), crop: { x: 225, y: 75, width: 150, height: 150 } });
    expect(m).toMatchObject({ step: "form", picture: { kind: "new", source: "file" } });
  });

  it("D16 a file that is not PNG, JPEG or GIF, or too large, is refused before the crop", async () => {
    const { m } = setup({ openFile: vi.fn(async () => ({ name: "x.webp", data: bytesToBase64(new Uint8Array([0x52, 0x49, 0x46, 0x46])) })) });
    m.setAvailability(true, true);
    await m.chooseFile();
    expect(m).toMatchObject({ step: "form", pictureError: EDIT_COPY.errors.unsupported });
    const read = vi.fn();
    await m.takeFile({ size: MAX_FILE_BYTES + 1, arrayBuffer: read });
    expect(read).not.toHaveBeenCalled();
    expect(m.pictureError).toBe(EDIT_COPY.errors.tooLarge);
    expect(sniffImage(new Uint8Array([0x47, 0x49, 0x46, 0x38, 0x39, 0x61]))).toBe("image/gif");
    expect(sniffImage(new Uint8Array([0xff, 0xd8, 0xff, 0xdb]))).toBe("image/jpeg");
  });

  it("a picture the webview cannot draw goes back to the form", async () => {
    const { m } = setup();
    m.setAvailability(true, true);
    await m.takeFile({ size: PNG.length, arrayBuffer: async () => PNG.buffer });
    expect(m.step).toBe("crop");
    m.pictureFailed();
    expect(m).toMatchObject({ step: "form", pictureError: EDIT_COPY.errors.unreadable });
  });

  it("D5 a file's picture is uploaded once, before the publish; a retry keeps the URL", async () => {
    const update = vi.fn().mockRejectedValueOnce(appError("platform.unavailable")).mockResolvedValueOnce(null);
    const { api, m, authorize } = setup({ dashpayUpdateProfile: update });
    m.setAvailability(true, true);
    await m.chooseFile();
    m.pictureLoaded(10, 10);
    await m.useCrop();
    let done = m.save();
    expect(m.saving).toBe("upload");
    await authorize();
    await done;
    expect(m.error).toBe(EDIT_COPY.errors.unavailable);
    done = m.save();
    await authorize();
    await done;
    expect(api.dashpayUploadAvatar).toHaveBeenCalledOnce();
    expect(update.mock.calls[1][2].avatar).toEqual({ kind: "set", candidate: "c-1" });
  });

  it("a candidate the engine no longer knows goes back to the published picture, once", async () => {
    const update = vi.fn(async () => Promise.reject(appError("invalid_argument")));
    const { m, authorize } = setup({ dashpayUpdateProfile: update });
    m.openStep("url");
    m.setUrl("https://example.org/a.png");
    await m.fetchUrl();
    m.setDisplayName("Alice");
    const done = m.save();
    await authorize();
    await done;
    expect(m).toMatchObject({ picture: { kind: "keep" }, error: EDIT_COPY.errors.candidateGone, displayName: "Alice", canSave: true });
  });

  it("an expired file candidate's upload refusal says to choose it again, not to retry", async () => {
    const { api, m } = setup({ dashpayUploadAvatar: vi.fn(async () => Promise.reject(appError("invalid_argument"))) });
    m.setAvailability(true, true);
    await m.chooseFile();
    m.pictureLoaded(10, 10);
    await m.useCrop();
    await m.save();
    expect(api.grantRequest).not.toHaveBeenCalled();
    expect(m).toMatchObject({ picture: { kind: "keep" }, error: EDIT_COPY.errors.candidateGone, saving: null });
  });

  it("D11 a navigation during the upload cancels the passphrase prompt to come; the edits stay", async () => {
    let uploaded: (url: string) => void = () => undefined;
    const { api, m } = setup({ dashpayUploadAvatar: vi.fn(() => new Promise<string>((r) => (uploaded = r))) });
    m.setAvailability(true, true);
    await m.chooseFile();
    m.pictureLoaded(10, 10);
    await m.useCrop();
    const done = m.save();
    expect(m.saving).toBe("upload");
    expect(m.requestLeave(() => undefined)).toBe(false);
    uploaded("https://i.imgur.com/up.png");
    await done;
    expect(m.grants.pending).toBeNull();
    expect(api.dashpayUpdateProfile).not.toHaveBeenCalled();
    expect(m).toMatchObject({ open: true, saving: null, error: null, picture: { kind: "new", candidate: { url: "https://i.imgur.com/up.png" } } });
  });

  it("a failed upload publishes nothing and says so", async () => {
    const { api, m } = setup({ dashpayUploadAvatar: vi.fn(async () => Promise.reject(appError("internal"))) });
    m.setAvailability(true, true);
    await m.chooseFile();
    m.pictureLoaded(10, 10);
    await m.useCrop();
    await m.save();
    expect(api.grantRequest).not.toHaveBeenCalled();
    expect(m).toMatchObject({ error: EDIT_COPY.errors.uploadFailed, saving: null, picture: { kind: "new" } });
  });
});
