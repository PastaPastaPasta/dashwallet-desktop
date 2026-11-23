// The fixture's Edit Profile calls (DP4-03 D19) over the real IPC path: the save's refusals in
// `profiles::update`'s order, a no-op, a conflict, the picture kept, set and removed, the
// candidates `dashpay_prepare_avatar` makes from each source, the upload, and the pictures toggle.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import type { DashPayProfile } from "../api/generated/DashPayProfile";
import type { ProfileEditArgs } from "../api/generated/ProfileEditArgs";
import { bytesToBase64 } from "../app/files";
import { IDENTITY } from "./dashpay";
import { installFixture } from "./index";
import { AVATAR_PNG, FIXTURE_WALLET_ID, type ScenarioName } from "./scenarios";

const W = FIXTURE_WALLET_ID;
const A = IDENTITY.alice;

beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  vi.stubGlobal("sessionStorage", { getItem: () => null, setItem: () => undefined, removeItem: () => undefined });
  vi.stubGlobal("location", { reload: () => undefined });
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

const install = (scenario: ScenarioName) => installFixture({ scenario });
const grant = async () => (await cmd.authorize({ kind: "platformOp", maxDuffs: 0, maxCredits: "300000000" }, W, { kind: "passphrase", passphrase: "demo" })).id;
const code = (p: Promise<unknown>) => p.then(() => null, (e: { code: string }) => e.code);
const profile = async () => (await cmd.dashpayProfile(W, A))!;
const edit = (patch: Partial<ProfileEditArgs>, baseline?: DashPayProfile | null): ProfileEditArgs => ({
  displayName: "Alice Liddell",
  publicMessage: "Down the rabbit hole",
  avatar: { kind: "keep" },
  ...(baseline !== undefined ? { baseline: { profile: baseline } } : {}),
  ...patch,
});
const file = (bytes: number[]) => ({ kind: "file" as const, bytes: bytesToBase64(new Uint8Array(bytes)), crop: { x: 1, y: 2, width: 3, height: 3 } });
const PNG = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

describe("dashpay_update_profile (DP4-01's checks, DP4-03 D19)", () => {
  it("trims, publishes under the grant, and the identity's profile follows", async () => {
    const f = install("dashpay-registered");
    const before = await profile();
    await cmd.dashpayUpdateProfile(W, A, edit({ displayName: "  Alice L.  ", publicMessage: "" }, before), await grant());
    const after = await profile();
    expect(after).toMatchObject({ displayName: "Alice L.", publicMessage: null, avatarUrl: null });
    expect(after.updatedAt).not.toBe(before.updatedAt);
    expect((await cmd.identities(W))[0].profile).toEqual(after);
    expect(f.calls.filter((c) => c === "dashpay_update_profile")).toHaveLength(1);
  });

  it("refuses too long (in characters), empty, and changed-since in the engine's order", async () => {
    install("dashpay-registered");
    const g = await grant();
    const long = await cmd.dashpayUpdateProfile(W, A, edit({ displayName: "x".repeat(26) }), g).catch((e) => e);
    expect(long).toMatchObject({ code: "profile.too_long", data: { kind: "profileTooLong", field: "displayName", length: 26, max: 25 } });
    // 25 emoji are 50 UTF-16 units but 25 characters.
    expect(await code(cmd.dashpayUpdateProfile(W, A, edit({ displayName: "😀".repeat(25) }), g))).toBeNull();
    expect(await code(cmd.dashpayUpdateProfile(W, A, edit({ publicMessage: "m".repeat(141) }), g))).toBe("profile.too_long");
    expect(await code(cmd.dashpayUpdateProfile(W, A, edit({ displayName: " ", publicMessage: null }), g))).toBe("profile.empty");
    const stale = { ...(await profile()), displayName: "Someone else" };
    const conflict = await cmd.dashpayUpdateProfile(W, A, edit({ displayName: "New" }, stale), g).catch((e) => e);
    expect(conflict).toMatchObject({ code: "profile.conflict", data: { kind: "profileConflict", current: { displayName: "😀".repeat(25), updatedAt: null } } });
  });

  it("an edit that changes nothing sends nothing, whatever the baseline", async () => {
    install("dashpay-registered");
    const before = await profile();
    await cmd.dashpayUpdateProfile(W, A, edit({}, { ...before, displayName: "stale" }), "no-such-grant");
    expect(await profile()).toEqual(before);
  });

  it("a baseline differing only in updatedAt or hash case is not a conflict", async () => {
    install("dashpay-avatar");
    const before = await profile();
    const baseline = { ...before, updatedAt: 1, avatarHash: before.avatarHash!.toUpperCase() };
    await cmd.dashpayUpdateProfile(W, A, edit({ displayName: "Alice" }, baseline), await grant());
    expect((await profile()).displayName).toBe("Alice");
  });

  it("keeps, removes and sets the picture; the thumbnail follows", async () => {
    install("dashpay-avatar");
    // A grant covers one write.
    await cmd.dashpayUpdateProfile(W, A, edit({ displayName: "Alice" }), await grant());
    expect((await profile()).avatarUrl).toBe("https://example.org/alice.png");
    await cmd.dashpayUpdateProfile(W, A, edit({ avatar: { kind: "remove" } }), await grant());
    expect(await profile()).toMatchObject({ avatarUrl: null, avatarHash: null, avatarFingerprint: null });
    expect(await cmd.dashpayAvatar(W, A, "small")).toBeNull();
    const c = await cmd.dashpayPrepareAvatar(W, { kind: "url", url: "https://example.org/new.png" });
    await cmd.dashpayUpdateProfile(W, A, edit({ avatar: { kind: "set", candidate: c.id } }), await grant());
    expect((await profile()).avatarUrl).toBe("https://example.org/new.png");
    expect(await cmd.dashpayAvatar(W, A, "large")).toEqual({ png: AVATAR_PNG, size: "large" });
  });

  it("refuses an unknown candidate and one still needing its upload", async () => {
    install("dashpay-avatar-upload");
    const g = await grant();
    expect(await code(cmd.dashpayUpdateProfile(W, A, edit({ avatar: { kind: "set", candidate: "nope" } }), g))).toBe("invalid_argument");
    const c = await cmd.dashpayPrepareAvatar(W, file(PNG));
    expect(c).toMatchObject({ url: null, needsUpload: true });
    expect(await code(cmd.dashpayUpdateProfile(W, A, edit({ avatar: { kind: "set", candidate: c.id } }), g))).toBe("invalid_argument");
    const url = await cmd.dashpayUploadAvatar(W, c.id);
    expect(await code(cmd.dashpayUploadAvatar(W, c.id))).toBe("invalid_argument");
    await cmd.dashpayUpdateProfile(W, A, edit({ avatar: { kind: "set", candidate: c.id } }), g);
    expect((await profile()).avatarUrl).toBe(url);
  });

  it("dashpay-profile-save-error fails the first save only, sending nothing", async () => {
    install("dashpay-profile-save-error");
    const g = await grant();
    expect(await code(cmd.dashpayUpdateProfile(W, A, edit({ displayName: "Alice" }), g))).toBe("platform.unavailable");
    expect((await profile()).displayName).toBe("Alice Liddell");
    await cmd.dashpayUpdateProfile(W, A, edit({ displayName: "Alice" }), await grant());
    expect((await profile()).displayName).toBe("Alice");
  });
});

describe("dashpay_prepare_avatar and the upload (DP4-02's calls, DP4-03 D19)", () => {
  it("answers each source by D19's rules and records it with its crop", async () => {
    const f = install("dashpay-registered");
    const prep = (s: Parameters<typeof cmd.dashpayPrepareAvatar>[1]) => code(cmd.dashpayPrepareAvatar(W, s));
    expect(await prep({ kind: "url", url: "not a url" })).toBe("invalid_argument");
    expect(await prep({ kind: "url", url: "http://example.org/a.png" })).toBe("invalid_argument");
    expect(await prep({ kind: "url", url: "https://example.org/missing.png" })).toBe("avatar.fetch_failed");
    expect(await prep({ kind: "url", url: "https://example.org/a.webp" })).toBe("avatar.unsupported");
    expect(await prep({ kind: "url", url: "https://example.org/huge.png" })).toBe("avatar.too_large");
    expect(await prep({ kind: "gravatar", email: "nobody@example.org" })).toBe("avatar.fetch_failed");
    expect(await prep({ kind: "gravatar", email: "no-at-sign" })).toBe("invalid_argument");
    expect(await prep(file([0x52, 0x49, 0x46, 0x46]))).toBe("avatar.unsupported");
    const g = await cmd.dashpayPrepareAvatar(W, { kind: "gravatar", email: "alice@example.org" });
    expect(g.url).not.toContain("alice");
    expect(g).toMatchObject({ preview: { png: AVATAR_PNG, size: "large" }, needsUpload: false });
    const j = await cmd.dashpayPrepareAvatar(W, file([0xff, 0xd8, 0xff, 0xe0]));
    expect(j.id).not.toBe(g.id);
    const prepares = f.world.dashpay.pictures!.prepares;
    expect(prepares).toHaveLength(10);
    expect(prepares.at(-1)).toMatchObject({ kind: "file", crop: { x: 1, y: 2, width: 3, height: 3 } });
  });

  it("refuses a PNG's crop outside the picture", async () => {
    install("dashpay-registered");
    // A 4 x 4 PNG.
    const bytes = Array.from(atob(AVATAR_PNG), (c) => c.charCodeAt(0));
    const crop = (x: number, width: number) => ({ kind: "file" as const, bytes: bytesToBase64(new Uint8Array(bytes)), crop: { x, y: 0, width, height: width } });
    expect(await code(cmd.dashpayPrepareAvatar(W, crop(0, 4)))).toBeNull();
    expect(await code(cmd.dashpayPrepareAvatar(W, crop(1, 4)))).toBe("invalid_argument");
    expect(await code(cmd.dashpayPrepareAvatar(W, crop(0, 0)))).toBe("invalid_argument");
  });

  it("no upload host: unavailable and upload_unconfigured", async () => {
    install("dashpay-registered");
    expect(await cmd.dashpayAvatarUploadAvailable(W)).toBe(false);
    const c = await cmd.dashpayPrepareAvatar(W, file(PNG));
    expect(await code(cmd.dashpayUploadAvatar(W, c.id))).toBe("avatar.upload_unconfigured");
    install("dashpay-avatar-upload");
    expect(await cmd.dashpayAvatarUploadAvailable(W)).toBe(true);
  });
});

describe("the avatar scenarios (DP4-03 D13)", () => {
  it("dashpay-avatars shows others' pictures; dashpay-pictures-off only the own", async () => {
    install("dashpay-avatars");
    expect(await cmd.loadContactPictures()).toBe(true);
    expect(await cmd.dashpayAvatar(W, IDENTITY.bob, "small")).not.toBeNull();
    expect(await cmd.dashpayAvatar(W, IDENTITY.carol, "small")).toBeNull();
    install("dashpay-pictures-off");
    expect(await cmd.loadContactPictures()).toBe(false);
    expect(await cmd.dashpayAvatar(W, IDENTITY.bob, "small")).toBeNull();
    expect(await cmd.dashpayAvatar(W, A, "small")).toEqual({ png: AVATAR_PNG, size: "small" });
    await cmd.setLoadContactPictures(true);
    expect(await cmd.dashpayAvatar(W, IDENTITY.bob, "small")).not.toBeNull();
  });
});
