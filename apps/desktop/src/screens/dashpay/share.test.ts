import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { Backend } from "../../api/backend";
import type { IdentitySummary } from "../../api/generated/IdentitySummary";
import { BackendContext } from "../../api/queries";
import { ProfileHeader } from "./MyProfile";
import { SHARE_QR_SIZE, ShareSheet } from "./ShareSheet";

const LINK = "dashpay://user?username=alice";
const linkQuery = vi.hoisted(() => ({ current: null as null | { data: string | undefined; error: unknown; isError: boolean; isFetching: boolean } }));
vi.mock("../../contacts/queries", async (original) => ({
  ...(await original<typeof import("../../contacts/queries")>()),
  useMyUserLink: () => ({ ...(linkQuery.current ?? { data: LINK, error: null, isError: false, isFetching: false }), refetch: () => undefined }),
}));
vi.mock("../../api/queries", async (original) => ({
  ...(await original<typeof import("../../api/queries")>()),
  useQrMatrix: () => ({ data: { size: 21, modules: [] } }),
}));
vi.mock("../../dashpay/profileQueries", async (original) => ({
  ...(await original<typeof import("../../dashpay/profileQueries")>()),
  useAvatar: () => null,
}));

const identity = (over: Partial<IdentitySummary>) =>
  ({ identity: "ID", index: 0, names: [], mainName: null, namesUpdating: false, namesCheck: null, isMain: true, balance: null, profile: null, ...over }) as IdentitySummary;

const header = (main: IdentitySummary, contest: string | null = null) =>
  renderToStaticMarkup(createElement(BackendContext.Provider, { value: {} as Backend }, createElement(ProfileHeader, { main, contest, walletId: "w", onEdit: () => undefined })));

describe("Share username on My Profile (PRODUCT #12)", () => {
  it("shows the button beside Edit Profile when the identity has a username", () => {
    const html = header(identity({ names: ["alice"], mainName: "alice" }));
    expect(html).toMatch(/data-testid="profile-share"[^>]*>Share Username</);
    expect(html).toContain('data-testid="profile-edit"');
    // The sheet waits for the button.
    expect(html).not.toContain("profile-share-sheet");
  });

  it("has no Share button without a username, while names update, or while a requested name is voted on", () => {
    expect(header(identity({}))).not.toContain("profile-share");
    expect(header(identity({}))).toContain('data-testid="profile-choose-name"');
    expect(header(identity({ namesUpdating: true }))).not.toContain("profile-share");
    expect(header(identity({ namesCheck: "failed" }))).not.toContain("profile-share");
    expect(header(identity({}), "alice")).not.toContain("profile-share");
  });

  it("hides the button while a held username updates or is rechecked: the engine gives no link then", () => {
    expect(header(identity({ names: ["alice"], mainName: "alice", namesUpdating: true }))).not.toContain('data-testid="profile-share"');
    expect(header(identity({ names: ["alice"], mainName: "alice", namesCheck: "pending" }))).not.toContain('data-testid="profile-share"');
  });

  it("hides the button until the engine gives a link for the username shown", () => {
    const named = identity({ names: ["alice"], mainName: "alice" });
    try {
      linkQuery.current = { data: undefined, error: null, isError: false, isFetching: true };
      expect(header(named)).not.toContain('data-testid="profile-share"');
      linkQuery.current = { data: undefined, error: { code: "platform.unavailable" }, isError: true, isFetching: false };
      expect(header(named)).not.toContain('data-testid="profile-share"');
      // A cached link refused on its re-read, or one for another name, is not shown.
      linkQuery.current = { data: LINK, error: { code: "platform.unavailable" }, isError: true, isFetching: false };
      expect(header(named)).not.toContain('data-testid="profile-share"');
      linkQuery.current = { data: "dashpay://user?username=bob", error: null, isError: false, isFetching: true };
      expect(header(named)).not.toContain('data-testid="profile-share"');
      // A cached link for the same name stays usable while it is re-read.
      linkQuery.current = { data: LINK, error: null, isError: false, isFetching: true };
      expect(header(named)).toContain('data-testid="profile-share"');
    } finally {
      linkQuery.current = null;
    }
  });

  it("the sheet shows My QR, large, with the dashpay://user link and Copy Link", () => {
    const html = renderToStaticMarkup(createElement(ShareSheet, { walletId: "w", identity: "ID", who: { name: "Alice Liddell", username: "alice" }, testId: "profile-share-sheet", onClose: () => undefined }));
    expect(html).toContain('data-testid="profile-share-sheet"');
    expect(html).toContain("Share my username");
    // Whose code it is: the display name, with the username under it.
    expect(html).toMatch(/class="share-who-name">Alice Liddell</);
    expect(html).toMatch(/class="share-who-username">@alice</);
    expect(html).toContain('data-testid="add-contact-my-qr"');
    expect(html).toMatch(new RegExp(`<svg[^>]*width="${SHARE_QR_SIZE}"[^>]*data-testid="add-contact-qr"`));
    expect(SHARE_QR_SIZE).toBeGreaterThanOrEqual(240);
    expect(html).toContain(LINK);
    expect(html).toContain("Copy Link");
  });

  it("shows no link read for another name than the one the sheet names", () => {
    const html = renderToStaticMarkup(createElement(ShareSheet, { walletId: "w", identity: "ID", who: { name: null, username: "alice2" }, testId: "profile-share-sheet", onClose: () => undefined }));
    expect(html).not.toContain(LINK);
    expect(html).not.toContain('data-testid="add-contact-qr"');
  });
});
