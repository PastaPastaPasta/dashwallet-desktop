// The fixture's answers for Add Contact (DP2-08 D17): eligibility, the send, My QR's link, the
// scanned codes' verification and the scan commands refuse in the engine's order (requests.rs,
// users.rs, api/scan.rs), so the screen tests exercise the engine's rules.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import { textToBase64 } from "../app/files";
import { IDENTITY } from "./dashpay";
import { installFixture, type FixtureHandle } from "./index";
import { fixtureQrImage } from "./scan";
import { FIXTURE_WALLET_ID as W, type ScenarioName } from "./scenarios";
import { FIXTURE_NOW } from "./wallet";

beforeEach(() => vi.stubGlobal("window", globalThis));
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

const ME = IDENTITY.alice;
/** A well-formed id no one holds. */
const UNKNOWN = "11111111111111111111111111111111";
const install = (scenario: ScenarioName = "dashpay-registered"): FixtureHandle => installFixture({ scenario });
const grant = async (maxCredits = "150000000") => (await cmd.authorize({ kind: "platformOp", maxDuffs: 0, maxCredits }, W, { kind: "passphrase", passphrase: "demo" })).id;
const now = Math.floor(FIXTURE_NOW.getTime() / 1000);
const link = (id: string, username: string) => `dashpay://user?id=${id}&username=${username}`;
const b64 = (bytes: Uint8Array) => btoa(String.fromCharCode(...bytes));

describe("dashpay_eligibility (fixture, as requests.rs)", () => {
  it("IOS-073 answers from the wallet's state, then the contact", async () => {
    install();
    expect(await cmd.dashpayEligibility(W, ME, ME)).toBe("isSelf");
    expect(await cmd.dashpayEligibility(W, ME, IDENTITY.bob)).toBe("alreadyContact");
    expect(await cmd.dashpayEligibility(W, ME, IDENTITY.frank)).toBe("pendingOutgoing");
    expect(await cmd.dashpayEligibility(W, ME, IDENTITY.grace)).toBe("pendingIncoming");
    // An ignored sender is a stranger to the engine.
    expect(await cmd.dashpayEligibility(W, ME, IDENTITY.ivan)).toBe("ok");
    expect(await cmd.dashpayEligibility(W, ME, IDENTITY.mallory)).toBe("ok");
    expect(await cmd.dashpayEligibility(W, ME, IDENTITY.mayumi)).toBe("contactCannotReceive");
    await expect(cmd.dashpayEligibility(W, ME, UNKNOWN)).rejects.toMatchObject({ code: "identity.not_found" });
    await expect(cmd.dashpayEligibility(W, ME, "nope")).rejects.toMatchObject({ code: "invalid_argument" });
  });

  it("asks for the identity's own keys before the network", async () => {
    install("dashpay-no-keys");
    expect(await cmd.dashpayEligibility(W, ME, IDENTITY.mallory)).toBe("noDashPayKeys");
    install("dashpay-search-offline");
    expect(await cmd.dashpayEligibility(W, ME, IDENTITY.bob)).toBe("alreadyContact");
    await expect(cmd.dashpayEligibility(W, ME, IDENTITY.mallory)).rejects.toMatchObject({ code: "platform.unavailable" });
  });
});

describe("dashpay_send_request (fixture, as requests.rs)", () => {
  it("IOS-073 sends under a grant: the contact shows pending, search and eligibility say so", async () => {
    const f = install();
    const out = await cmd.dashpaySendRequest(W, ME, IDENTITY.mallory, null, await grant());
    expect(out.kind).toBe("pending");
    const row = f.world.dashpay.contacts.find((c) => c.summary.contact === IDENTITY.mallory)!;
    expect(row.summary).toMatchObject({ relation: "pendingOutgoing", username: "mallory", displayName: "Mallory" });
    expect(await cmd.dashpayEligibility(W, ME, IDENTITY.mallory)).toBe("pendingOutgoing");
    expect((await cmd.searchUsers(W, "mal", 12))[0].relation).toBe("pendingOutgoing");
    // A second send is refused before the grant.
    await expect(cmd.dashpaySendRequest(W, ME, IDENTITY.mallory, null, await grant())).rejects.toMatchObject({
      code: "contact.ineligible",
      data: { kind: "ineligible", reason: "pendingOutgoing" },
    });
  });

  it("refuses in the engine's order: ids, scan, eligibility, credits, grant", async () => {
    const f = install();
    await expect(cmd.dashpaySendRequest(W, ME, "nope", null, "g")).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.dashpaySendRequest(W, ME, IDENTITY.mallory, "scan-9", "g")).rejects.toMatchObject({ code: "contact.scan_expired" });
    await expect(cmd.dashpaySendRequest(W, ME, IDENTITY.mayumi, null, "g")).rejects.toMatchObject({
      code: "contact.ineligible",
      data: { reason: "contactCannotReceive" },
    });
    f.world.dashpay.identities[0].balance = "1000";
    await expect(cmd.dashpaySendRequest(W, ME, IDENTITY.mallory, null, "g")).rejects.toMatchObject({ code: "platform.insufficient_credits" });
    f.world.dashpay.identities[0].balance = "184500000000";
    await expect(cmd.dashpaySendRequest(W, ME, IDENTITY.mallory, null, await grant("1000"))).rejects.toMatchObject({ code: "platform.grant_exceeded" });
    expect(f.world.dashpay.contacts.some((c) => c.summary.contact === IDENTITY.mallory)).toBe(false);
  });

  it("IOS-073 a scan's claim is spent by its send and belongs to its recipient", async () => {
    install();
    const scanned = await cmd.dashpayVerifyScanned(W, `dash:?du=mallory&dapk=${now + 3600}`);
    if (scanned.code.kind !== "autoAcceptClaim") throw new Error("expected a claim");
    const { scan } = scanned.code;
    await expect(cmd.dashpaySendRequest(W, ME, IDENTITY.marta, scan, await grant())).rejects.toMatchObject({ code: "invalid_argument" });
    await cmd.dashpaySendRequest(W, ME, IDENTITY.mallory, scan, await grant());
    await expect(cmd.dashpaySendRequest(W, ME, IDENTITY.mallory, scan, await grant())).rejects.toMatchObject({ code: "contact.scan_expired" });
  });
});

describe("dashpay_my_user_link and dashpay_verify_scanned (fixture, as users.rs)", () => {
  it("IOS-073 My QR is the plain link; no name or names updating say so", async () => {
    install();
    expect(await cmd.dashpayMyUserLink(W, ME)).toBe(link(ME, "alice"));
    install("dashpay-no-username");
    await expect(cmd.dashpayMyUserLink(W, ME)).rejects.toMatchObject({ code: "contact.no_username" });
  });

  it("IOS-073 verifies a link against its name's owner, in Platform's spelling", async () => {
    install();
    expect(await cmd.dashpayVerifyScanned(W, link(IDENTITY.mallory, "Mallory.dash"))).toMatchObject({
      identity: IDENTITY.mallory,
      username: "mallory",
      displayName: "Mallory",
      code: { kind: "userLink" },
      relation: "stranger",
    });
    expect((await cmd.dashpayVerifyScanned(W, link(IDENTITY.ivan, "ivan"))).relation).toBe("ignored");
    // The name is someone else's: unverified, not a send.
    await expect(cmd.dashpayVerifyScanned(W, link(IDENTITY.marta, "mallory"))).rejects.toMatchObject({ code: "contact.scan_unverified" });
    await expect(cmd.dashpayVerifyScanned(W, link(IDENTITY.mallory, "nobody"))).rejects.toMatchObject({ code: "contact.scan_unverified" });
    await expect(cmd.dashpayVerifyScanned(W, `${link(IDENTITY.mallory, "mallory")}&id=x`)).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(cmd.dashpayVerifyScanned(W, "dash:XpayAddress")).rejects.toMatchObject({ code: "invalid_argument" });
  });

  it("IOS-073 an expired contact code is refused before any query; DAPI down is unavailable", async () => {
    install("dashpay-search-offline");
    await expect(cmd.dashpayVerifyScanned(W, `dash:?du=mallory&dapk=${now - 1}`)).rejects.toMatchObject({ code: "contact.scan_expired" });
    await expect(cmd.dashpayVerifyScanned(W, link(IDENTITY.mallory, "mallory"))).rejects.toMatchObject({ code: "platform.unavailable" });
  });
});

describe("scan commands (fixture, as api/scan.rs)", () => {
  it("IOS-043 decodes fixture images; others are unreadable, empty ones have no code", async () => {
    install();
    const image = fixtureQrImage("dash:Xpay", link(IDENTITY.mallory, "mallory"));
    expect(await cmd.scanQrImage(b64(image))).toEqual(["dash:Xpay", link(IDENTITY.mallory, "mallory")]);
    await expect(cmd.scanQrImage(textToBase64("GIF89a"))).rejects.toMatchObject({ code: "app.image_unreadable" });
    await expect(cmd.scanQrImage(b64(fixtureQrImage()))).rejects.toMatchObject({ code: "app.no_qr_code" });
    await expect(cmd.scanQrImage("%%%")).rejects.toMatchObject({ code: "invalid_argument" });
  });

  it("IOS-043 Open Image… shows the image dialog; a cancel is null", async () => {
    const f = install();
    expect(await cmd.scanQrFile()).toBeNull();
    f.world.tools.files.open = { name: "qr.png", data: b64(fixtureQrImage("x")) };
    expect(await cmd.scanQrFile()).toEqual(["x"]);
    expect(f.world.tools.files.dialogs.map((d) => d.purpose)).toEqual(["qrImage", "qrImage"]);
  });

  it("IOS-043 the screen picker answers, cancels, waits for Cancel, or is missing", async () => {
    const f = install();
    expect(await cmd.screenScanMode()).toBe("picker");
    expect(await cmd.scanQrScreen()).toBeNull();
    f.world.scan.screen = b64(fixtureQrImage("y"));
    expect(await cmd.scanQrScreen()).toEqual(["y"]);
    f.world.scan.holdScreen = true;
    const waiting = cmd.scanQrScreen();
    await cmd.cancelScreenScan();
    expect(await waiting).toBeNull();
    f.world.scan.screen = "unavailable";
    await expect(cmd.scanQrScreen()).rejects.toMatchObject({ code: "app.screen_capture_unavailable" });
    install("dashpay-screen-shortcut");
    expect(await cmd.screenScanMode()).toBe("copyShortcut");
    await expect(cmd.scanQrScreen()).rejects.toMatchObject({ code: "app.screen_capture_unavailable" });
  });
});
