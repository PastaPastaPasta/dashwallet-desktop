// Add Contact's view model (DP2-08 D4, D6–D10, D12–D14, D16).

import { afterEach, describe, expect, it, vi } from "vitest";
import type { ScannedContactInfo } from "../api/generated/ScannedContactInfo";
import { appError, WALLET_A } from "../onboarding/testWorld";
import {
  AddContactModel,
  answersAtOnce,
  candidateName,
  candidateOfHit,
  cardLine,
  CLAIM_TICK_MS,
  claimLine,
  claimNote,
  currentRelation,
  listedNow,
  eligibilityLine,
  MAX_IMAGE_BYTES,
  imageError,
  myQr,
  NO_CODE_IN_IMAGE,
  NOT_A_CODE,
  ownerLine,
  SCAN_EXPIRED,
  SCAN_UNVERIFIED,
  scanStatus,
  SCREEN_UNAVAILABLE,
  userLinkText,
  verifyError,
  watchClaim,
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
    const c = candidateOfHit({ identity: GRACE, username: "grace", displayName: null, relation: "stranger", unverified: false, readSeq: 100 });
    expect(currentRelation(c, undefined)).toBe("stranger");
    expect(currentRelation(c, { rows: [{ contact: GRACE, relation: "pendingOutgoing" }], seq: 50 })).toBe("pendingOutgoing");
    expect(currentRelation(c, { rows: [{ contact: GRACE, relation: "ignored" }], seq: 50 })).toBe("ignored");
    expect(currentRelation({ ...c, relation: "isSelf" }, { rows: [], seq: 200 })).toBe("isSelf");
  });

  it("D34 D38 an ignore sticks until a list read begun after it says otherwise", () => {
    const ignored = candidateOfHit({ identity: GRACE, username: "grace", displayName: null, relation: "ignored", unverified: false, readSeq: 100 });
    const stale = (rows: { contact: string; relation: "pendingIncoming" }[], seq: number) => ({ rows, seq });
    const incoming = [{ contact: GRACE, relation: "pendingIncoming" as const }];
    // A list read begun before the hit arrived: still ignored, so the card answers at once, no Send.
    for (const seq of [0, 99, 100]) {
      for (const rows of [[], incoming]) {
        const relation = currentRelation(ignored, stale(rows, seq));
        expect(relation).toBe("ignored");
        expect(answersAtOnce(relation)).toBe(true);
        expect(cardLine(relation, { data: "ok", failed: false })?.next).toBe("showHidden");
      }
    }
    // A list read begun after it: unignored (their request shows again) or gone.
    expect(currentRelation(ignored, stale(incoming, 101))).toBe("pendingIncoming");
    expect(currentRelation(ignored, stale([], 101))).toBe("stranger");
  });

  it("D14 an empty list a pass still holds says nothing: the candidate's relation stands", () => {
    const rows = [{ contact: GRACE, relation: "ignored" as const }];
    const list = (contacts: typeof rows, updating: boolean) => ({ sections: [{ contacts }], updating, seq: 5 });
    expect(listedNow(undefined)).toBeUndefined();
    expect(listedNow(list([], true))).toBeUndefined();
    expect(listedNow(list([], false))).toEqual({ rows: [], seq: 5 });
    expect(listedNow(list(rows, true))).toEqual({ rows, seq: 5 });
    const c = candidateOfHit({ identity: GRACE, username: "grace", displayName: null, relation: "stranger", unverified: false, readSeq: 1 });
    expect(currentRelation(c, listedNow(list([], true)))).toBe("stranger");
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

  it("D7 D36 notes an auto-accept claim and its expiry; an expired one replaces Send", () => {
    const now = 1_000_000_000_000;
    const at = (secs: number) => ({ scan: "scan-1", expiresAt: now / 1000 + secs });
    const send = { text: null, next: "send" as const };
    expect(claimNote(null, now)).toBeNull();
    expect(claimNote(at(1800), now)).toMatch(/accept you automatically.*expires in 30 min\.$/);
    expect(claimNote(at(7200), now)).toMatch(/expires in 2 h\.$/);
    expect(claimNote(at(61), now)).toMatch(/expires in 1 min\.$/);
    expect(claimLine(send, at(61), now)).toEqual(send);
    // The engine refuses a claim with a minute or less left.
    for (const secs of [60, -1]) {
      expect(claimNote(at(secs), now)).toBeNull();
      expect(claimLine(send, at(secs), now)).toEqual({ text: SCAN_EXPIRED, next: null });
    }
    // Only Send needs the claim; a plain card and other next steps stand.
    expect(claimLine(send, null, now)).toEqual(send);
    const contact = { text: "Already in your contacts.", next: "showContact" as const };
    expect(claimLine(contact, at(-1), now)).toBe(contact);
    expect(claimLine(undefined, at(-1), now)).toBeUndefined();
  });

  it("D9 D10 the Scan tab's progress line; a link's check shows on the card", () => {
    expect(scanStatus({ kind: "idle" })).toBeNull();
    expect(scanStatus({ kind: "reading" })).toBe("Reading the image…");
    expect(scanStatus({ kind: "capturing" })).toBe("Select a region of the screen…");
    expect(scanStatus({ kind: "verifying", source: "scan" })).toBe("Checking the code on the Dash network…");
    expect(scanStatus({ kind: "verifying", source: "link" })).toBeNull();
  });

  it("D4 only oneself and an ignored sender answer without eligibility", () => {
    expect(["isSelf", "ignored"].every((r) => answersAtOnce(r as "isSelf"))).toBe(true);
    expect(answersAtOnce("stranger")).toBe(false);
    expect(answersAtOnce("pendingOutgoing")).toBe(false);
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

describe("an open claim (D36)", () => {
  afterEach(() => vi.useRealTimers());

  it("ages its note on a timer and replaces Send the moment it expires", () => {
    vi.useFakeTimers();
    const start = 1_000_000_000_000;
    vi.setSystemTime(start);
    // 30 min 10 s: the margin (29 min 10 s in) falls between two ticks.
    const claim = { scan: "scan-1", expiresAt: start / 1000 + 30 * 60 + 10 };
    const send = { text: null, next: "send" as const };
    // What the card renders on each tick.
    const shown: (string | null)[] = [];
    const render = () => shown.push(claimLine(send, claim, Date.now())?.next === "send" ? claimNote(claim, Date.now()) : SCAN_EXPIRED);
    const stop = watchClaim(claim, Date.now, render);
    vi.advanceTimersByTime(CLAIM_TICK_MS);
    expect(shown).toEqual(["This code asks them to accept you automatically. They confirm it when your request arrives. It expires in 30 min."]);
    vi.advanceTimersByTime(15 * 60_000 - CLAIM_TICK_MS);
    expect(shown.at(-1)).toMatch(/expires in 15 min\.$/);
    // The 29 min tick still offers Send; the card hears the expiry the moment it comes, not at the
    // next tick (29 min 30 s).
    vi.advanceTimersByTime(14 * 60_000 + 10_000 - 1);
    expect(shown.at(-1)).toMatch(/expires in 1 min\.$/);
    vi.advanceTimersByTime(1);
    expect(shown.at(-1)).toBe(SCAN_EXPIRED);
    // Nothing changes after that: the timer is done.
    const ticks = shown.length;
    expect(vi.getTimerCount()).toBe(0);
    vi.advanceTimersByTime(10 * 60_000);
    expect(shown.length).toBe(ticks);
    stop();
  });

  it("stops when the card goes, and never starts for an expired claim", () => {
    vi.useFakeTimers();
    const tick = vi.fn();
    const stop = watchClaim({ scan: "s", expiresAt: Date.now() / 1000 + 600 }, Date.now, tick);
    stop();
    vi.advanceTimersByTime(600_000);
    expect(tick).not.toHaveBeenCalled();
    watchClaim({ scan: "s", expiresAt: Date.now() / 1000 + 60 }, Date.now, tick);
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe("AddContactModel", () => {
  it("IOS-073 a search hit is a candidate without verification", () => {
    const { api, model } = setup();
    model.pick({ identity: GRACE, username: "grace", displayName: null, relation: "stranger", unverified: false, readSeq: 1 });
    expect(model.candidate).toMatchObject({ identity: GRACE, source: "search", claim: null });
    expect(api.dashpayVerifyScanned).not.toHaveBeenCalled();
  });

  it("IOS-073 a pasted text is verified; Platform's answer is the candidate", async () => {
    const { api, model } = setup({ dashpayVerifyScanned: vi.fn(async () => info({ code: { kind: "autoAcceptClaim", scan: "scan-1", expiresAt: 99 } })) });
    const done = model.verifyText(`  ${LINK}\n`);
    expect(model.scan).toEqual({ kind: "verifying", source: "scan" });
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
    expect(model.scan).toEqual({ kind: "verifying", source: "link" });
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
    await model.imageFile(new File(["img"], "qr.png"));
    expect(api.scanQrImage).toHaveBeenCalledWith("aW1n");
    expect(verify.mock.calls.map((c) => c[1])).toEqual(["dash:XpayRequest", LINK]);
    expect(model.candidate?.source).toBe("scan");
  });

  it("D35 refuses a file too large before reading it, and a read that turns out too large", async () => {
    const { api, model } = setup();
    const tooLarge = { kind: "failed", source: "scan", text: "The image is too large.", retry: false };
    const arrayBuffer = vi.fn(() => Promise.reject(new Error("read a huge file")));
    await model.imageFile({ size: MAX_IMAGE_BYTES + 1, arrayBuffer });
    expect(arrayBuffer).not.toHaveBeenCalled();
    expect(model.scan).toEqual(tooLarge);
    // Its size said less than it holds: the bytes are checked too, nothing crosses IPC.
    await model.imageFile({ size: 3, arrayBuffer: async () => new ArrayBuffer(MAX_IMAGE_BYTES + 1) });
    expect(model.scan).toEqual(tooLarge);
    expect(api.scanQrImage).not.toHaveBeenCalled();
    // A read the webview refuses (a folder, a file gone) is the file's refusal.
    await model.imageFile({ size: 3, arrayBuffer: () => Promise.reject(new DOMException("gone", "NotReadableError")) });
    expect(model.scan).toEqual({ kind: "failed", source: "scan", text: "The file could not be read.", retry: false });
    // At the limit the file is read and decoded.
    await model.imageFile({ size: MAX_IMAGE_BYTES, arrayBuffer: async () => new TextEncoder().encode("img").buffer as ArrayBuffer });
    expect(api.scanQrImage).toHaveBeenCalledWith("aW1n");
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
    model.pick({ identity: GRACE, username: "grace", displayName: null, relation: "stranger", unverified: false, readSeq: 1 });
    await model.openImage();
    expect(model.scan).toEqual({ kind: "idle" });
    expect(model.candidate?.identity).toBe(GRACE);
    expect(api.dashpayVerifyScanned).not.toHaveBeenCalled();
    await model.screen();
    expect(model.scan).toMatchObject({ kind: "failed", text: "No QR code was found in this image." });
    // A capture that ended has nothing to cancel.
    expect(api.cancelScreenScan).not.toHaveBeenCalled();
  });

  it("IOS-043 a finished capture's codes are verified without cancelling it", async () => {
    const { api, model } = setup({ scanQrScreen: vi.fn(async () => [LINK]) });
    await model.screen();
    expect(model.candidate?.source).toBe("scan");
    expect(api.cancelScreenScan).not.toHaveBeenCalled();
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
