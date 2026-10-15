// Add Contact's view model (DP2-08 D4, D6–D10, D12–D14, D16).

import { describe, expect, it, vi } from "vitest";
import type { ScannedContactInfo } from "../api/generated/ScannedContactInfo";
import { appError, WALLET_A } from "../onboarding/testWorld";
import {
  AddContactModel,
  candidateName,
  candidateOfHit,
  cardLine,
  claimNote,
  currentRelation,
  eligibilityLine,
  imageError,
  myQr,
  NO_CODE_IN_IMAGE,
  NOT_A_CODE,
  ownerLine,
  SCAN_UNVERIFIED,
  SCREEN_UNAVAILABLE,
  userLinkText,
  verifyError,
  type ScanApi,
} from "./add";

const GRACE = "GrAC3eQhZmKGwKu4R7PDuBkaxWDcXuspNwGM8RHm9ZsZ";
const LINK = userLinkText(GRACE, "grace");

const info = (over: Partial<ScannedContactInfo> = {}): ScannedContactInfo => ({
  identity: GRACE,
  username: "Grace",
  displayName: "Grace Hopper",
  code: { kind: "userLink" },
  relation: "stranger",
  unverified: false,
  ...over,
});

function setup(over: Partial<ScanApi> = {}) {
  const api: ScanApi = {
    dashpayVerifyScanned: vi.fn(async () => info()),
    scanQrFile: vi.fn(async () => [LINK]),
    scanQrImage: vi.fn(async () => [LINK]),
    scanQrScreen: vi.fn(async () => [LINK]),
    cancelScreenScan: vi.fn(async () => null),
    ...over,
  };
  return { api, model: new AddContactModel(api, WALLET_A) };
}

describe("eligibility (D4, D13)", () => {
  it("IOS-073 maps every eligibility answer to its line and next step", () => {
    expect(eligibilityLine("ok")).toEqual({ text: null, next: "send" });
    expect(eligibilityLine("isSelf")).toEqual({ text: "This is you.", next: null });
    expect(eligibilityLine("alreadyContact")).toEqual({ text: "Already in your contacts.", next: "showContact" });
    expect(eligibilityLine("pendingOutgoing")).toEqual({ text: "Contact request sent.", next: "showRequest" });
    expect(eligibilityLine("pendingIncoming")).toEqual({ text: "They sent you a contact request.", next: "showRequest" });
    expect(eligibilityLine("noDashPayKeys")).toEqual({ text: "Enable DashPay to send contact requests.", next: "enable" });
    expect(eligibilityLine("contactCannotReceive")).toEqual({ text: "This user cannot receive contact requests yet.", next: null });
  });

  it("IOS-073 answers no identity, self and ignored before any read", () => {
    expect(ownerLine({ kind: "join", reason: null })).toEqual({ text: "Join DashPay to send contact requests.", next: "join" });
    expect(ownerLine({ kind: "join", reason: "watchOnly" })?.next).toBeNull();
    expect(ownerLine({ kind: "registering" })).toEqual({ text: "You can send contact requests once your username is registered.", next: null });
    expect(ownerLine({ kind: "starting" })).toBeUndefined();
    const idle = { data: undefined, failed: false };
    expect(cardLine("isSelf", idle)).toEqual({ text: "This is you.", next: null });
    // The engine's eligibility treats an ignored sender as a stranger.
    expect(cardLine("ignored", { data: "ok", failed: false })).toEqual({ text: "You ignored their contact request.", next: "showHidden" });
    expect(cardLine("stranger", idle)).toBeUndefined();
    expect(cardLine("stranger", { data: undefined, failed: true })).toEqual({ text: "Could not check this user.", next: "retry" });
    expect(cardLine("stranger", { data: "ok", failed: false })).toEqual({ text: null, next: "send" });
  });

  it("D14 goes by the contact list once it is read", () => {
    const c = candidateOfHit({ identity: GRACE, username: "grace", displayName: null, relation: "stranger", unverified: false });
    expect(currentRelation(c, undefined)).toBe("stranger");
    expect(currentRelation(c, [{ contact: GRACE, relation: "pendingOutgoing" }])).toBe("pendingOutgoing");
    expect(currentRelation({ ...c, relation: "ignored" }, [])).toBe("stranger");
    expect(currentRelation({ ...c, relation: "isSelf" }, [])).toBe("isSelf");
  });

  it("names the candidate by display name, username, then id", () => {
    expect(candidateName({ displayName: "Grace", username: "g", identity: GRACE })).toBe("Grace");
    expect(candidateName({ displayName: null, username: "g", identity: GRACE })).toBe("@g");
    expect(candidateName({ displayName: null, username: null, identity: GRACE })).toBe(GRACE);
  });
});

describe("copy (D6, D7, D9, D12)", () => {
  it("IOS-073 says verification refusals, with Try again only for the network", () => {
    expect(verifyError(appError("contact.scan_unverified"))).toEqual({ text: SCAN_UNVERIFIED, retry: false });
    expect(verifyError(appError("contact.scan_expired"))).toEqual({ text: "This QR code has expired. Ask for a new one.", retry: false });
    expect(verifyError(appError("invalid_argument"))).toEqual({ text: NOT_A_CODE, retry: false });
    expect(verifyError(appError("platform.unavailable"))).toEqual({ text: "Could not reach Dash Platform.", retry: true });
    expect(verifyError(appError("platform.timeout")).retry).toBe(true);
  });

  it("IOS-043 says image and capture refusals", () => {
    expect(imageError(appError("app.image_unreadable"))).toBe("This image could not be read. Use a PNG, JPEG or BMP file.");
    expect(imageError(appError("app.no_qr_code"))).toBe("No QR code was found in this image.");
    expect(imageError(appError("app.screen_capture_unavailable"))).toBe(SCREEN_UNAVAILABLE);
    expect(imageError(appError("app.file_too_large"))).toBe("The file is too large.");
  });

  it("D7 notes an auto-accept claim and its expiry", () => {
    const now = 1_000_000_000_000;
    const at = (secs: number) => ({ scan: "scan-1", expiresAt: now / 1000 + secs });
    expect(claimNote(null, now)).toBeNull();
    expect(claimNote(at(1800), now)).toMatch(/accept you automatically.*expires in 30 min\.$/);
    expect(claimNote(at(7200), now)).toMatch(/expires in 2 h\.$/);
    expect(claimNote(at(-1), now)).toMatch(/This code has expired\.$/);
  });

  it("D13 rebuilds a link in the engine's form", () => {
    expect(userLinkText(GRACE, "gr ace&x")).toBe(`dashpay://user?id=${GRACE}&username=gr%20ace%26x`);
  });

  it("D12 My QR's states", () => {
    expect(myQr({ data: LINK, error: null, isError: false })).toEqual({ kind: "link", link: LINK });
    expect(myQr({ data: undefined, error: null, isError: false })).toEqual({ kind: "loading" });
    expect(myQr({ data: undefined, error: appError("contact.no_username"), isError: true })).toEqual({ kind: "noUsername" });
    expect(myQr({ data: undefined, error: appError("platform.unavailable"), isError: true })).toEqual({ kind: "updating" });
    expect(myQr({ data: undefined, error: appError("internal", null, "boom"), isError: true })).toEqual({ kind: "failed", text: "boom" });
  });
});

describe("AddContactModel", () => {
  it("IOS-073 a search hit is a candidate without verification", () => {
    const { api, model } = setup();
    model.pick({ identity: GRACE, username: "grace", displayName: null, avatarUrl: null, relation: "stranger", unverified: false });
    expect(model.candidate).toMatchObject({ identity: GRACE, source: "search", claim: null });
    expect(api.dashpayVerifyScanned).not.toHaveBeenCalled();
  });

  it("IOS-073 a pasted text is verified; Platform's answer is the candidate", async () => {
    const { api, model } = setup({ dashpayVerifyScanned: vi.fn(async () => info({ code: { kind: "autoAcceptClaim", scan: "scan-1", expiresAt: 99 } })) });
    const done = model.verifyText(`  ${LINK}\n`);
    expect(model.scan).toEqual({ kind: "verifying", source: "scan", username: null });
    await done;
    expect(api.dashpayVerifyScanned).toHaveBeenCalledWith(WALLET_A, LINK);
    expect(model.candidate).toMatchObject({ username: "Grace", source: "scan", claim: { scan: "scan-1", expiresAt: 99 } });
    expect(model.scan).toEqual({ kind: "idle" });
  });

  it("IOS-073 an unverifiable code is an error state with no candidate", async () => {
    const { model } = setup({
      dashpayVerifyScanned: vi.fn(async () => {
        throw appError("contact.scan_unverified");
      }),
    });
    await model.verifyText(LINK);
    expect(model.candidate).toBeNull();
    expect(model.scan).toEqual({ kind: "failed", source: "scan", text: SCAN_UNVERIFIED, retry: false });
  });

  it("D13 a link says its username while it is checked, and errors on the card", async () => {
    let fail!: (e: unknown) => void;
    const { api, model } = setup({ dashpayVerifyScanned: vi.fn(() => new Promise<ScannedContactInfo>((_, reject) => (fail = reject))) });
    const done = model.verifyLink(GRACE, "grace");
    expect(model.scan).toEqual({ kind: "verifying", source: "link", username: "grace" });
    expect(api.dashpayVerifyScanned).toHaveBeenCalledWith(WALLET_A, LINK);
    fail(appError("contact.scan_unverified"));
    await done;
    expect(model.scan).toMatchObject({ kind: "failed", source: "link" });
  });

  it("IOS-043 D8 tries an image's codes in order: others skipped, the first contact code wins", async () => {
    const verify = vi.fn(async (_w: string, text: string) => {
      if (text !== LINK) throw appError("invalid_argument");
      return info();
    });
    const { api, model } = setup({ scanQrImage: vi.fn(async () => ["dash:XpayRequest", LINK, "dashpay://user?id=other"]), dashpayVerifyScanned: verify });
    await model.image("aW1n");
    expect(api.scanQrImage).toHaveBeenCalledWith("aW1n");
    expect(verify.mock.calls.map((c) => c[1])).toEqual(["dash:XpayRequest", LINK]);
    expect(model.candidate?.source).toBe("scan");
  });

  it("IOS-043 D8 an image with no contact code says so", async () => {
    const { model } = setup({
      scanQrFile: vi.fn(async () => ["dash:Xpay"]),
      dashpayVerifyScanned: vi.fn(async () => {
        throw appError("invalid_argument");
      }),
    });
    await model.openImage();
    expect(model.scan).toMatchObject({ kind: "failed", text: NO_CODE_IN_IMAGE });
  });

  it("IOS-043 a cancelled dialog or picker changes nothing; a decoder refusal is said", async () => {
    const { api, model } = setup({
      scanQrFile: vi.fn(async () => null),
      scanQrScreen: vi.fn(async () => {
        throw appError("app.no_qr_code");
      }),
    });
    model.pick({ identity: GRACE, username: "grace", displayName: null, avatarUrl: null, relation: "stranger", unverified: false });
    await model.openImage();
    expect(model.scan).toEqual({ kind: "idle" });
    expect(model.candidate?.identity).toBe(GRACE);
    expect(api.dashpayVerifyScanned).not.toHaveBeenCalled();
    await model.screen();
    expect(model.scan).toMatchObject({ kind: "failed", text: "No QR code was found in this image." });
  });

  it("IOS-043 D10 Cancel stops the picker and drops its late answer", async () => {
    let answer!: (texts: string[] | null) => void;
    const { api, model } = setup({ scanQrScreen: vi.fn(() => new Promise<string[] | null>((r) => (answer = r))) });
    const done = model.screen();
    expect(model.scan).toEqual({ kind: "capturing" });
    model.cancelScreen();
    expect(api.cancelScreenScan).toHaveBeenCalledTimes(1);
    expect(model.scan).toEqual({ kind: "idle" });
    answer([LINK]);
    await done;
    expect(api.dashpayVerifyScanned).not.toHaveBeenCalled();
    expect(model.candidate).toBeNull();
  });

  it("D16 keeps a text for Try again only after a network failure", async () => {
    const SECRET = "dash:?du=grace&dapk=c2VjcmV0";
    const verify = vi
      .fn<ScanApi["dashpayVerifyScanned"]>()
      .mockRejectedValueOnce(appError("platform.unavailable"))
      .mockResolvedValueOnce(info());
    const { model } = setup({ dashpayVerifyScanned: verify });
    await model.verifyText(SECRET);
    expect(model.scan).toMatchObject({ kind: "failed", retry: true });
    await model.retry();
    expect(verify.mock.calls.map((c) => c[1])).toEqual([SECRET, SECRET]);
    expect(model.candidate).not.toBeNull();
    // Nothing is held now: another Try again does nothing.
    await model.retry();
    expect(verify).toHaveBeenCalledTimes(2);
  });

  it("the latest request wins; Close drops the candidate", async () => {
    let first!: (v: ScannedContactInfo) => void;
    const verify = vi
      .fn<ScanApi["dashpayVerifyScanned"]>()
      .mockImplementationOnce(() => new Promise((r) => (first = r)))
      .mockResolvedValueOnce(info({ username: "second" }));
    const { model } = setup({ dashpayVerifyScanned: verify });
    const a = model.verifyText("one");
    await model.verifyText("two");
    first(info({ username: "first" }));
    await a;
    expect(model.candidate?.username).toBe("second");
    model.clear();
    expect(model.candidate).toBeNull();
  });
});
