// The fixture's Identities page calls (DP6-01) over the real IPC path: each identity's keys and
// revision, Set as main (kept across a relaunch's world), Refresh and Refresh all with a failure,
// the read-only look-up by id and by username, Set as main name and Find my identities.

import { listen } from "@tauri-apps/api/event";
import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import type { AppEvent } from "../api/generated/commands";
import { IDENTITY } from "./dashpay";
import { identitiesOf, SPARE_IDENTITY, WORK_IDENTITY } from "./identities";
import { installFixture } from "./index";
import { restarted } from "./registration";
import { FIXTURE_WALLET_ID, type World } from "./scenarios";

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

const code = (p: Promise<unknown>) => p.then(() => null, (e: { code: string }) => e.code);

describe("the Identities page's calls (DP6-01)", () => {
  it("two identities, the second main; each with its keys and revision", async () => {
    installFixture({ scenario: "dashpay-identities" });
    const list = await cmd.identities(W);
    expect(list.map((i) => [i.identity, i.index, i.isMain])).toEqual([
      [A, 0, false],
      [WORK_IDENTITY, 1, true],
    ]);
    const alice = await cmd.identityDetail(W, A);
    expect([alice.revision, alice.publicKeys.length, alice.publicKeys.filter((k) => k.disabledAt !== null).length]).toEqual([3, 7, 1]);
    expect(alice.publicKeys.find((k) => k.id === 4)?.contractBoundDocumentType).toBe("contactRequest");
    expect((await cmd.identityDetail(W, WORK_IDENTITY)).publicKeys).toHaveLength(4);
    expect(await code(cmd.identityDetail(W, IDENTITY.bob))).toBe("identity.not_found");
  });

  it("Set as main moves the badge and the status, signals both as the engine does (D15), and a relaunch keeps it", async () => {
    const f = installFixture({ scenario: "dashpay-identities" });
    const seen: AppEvent[] = [];
    await listen<AppEvent>(cmd.ENGINE_EVENT, (e) => void seen.push(e.payload));
    await cmd.setMainIdentity(W, A);
    expect(seen.flatMap((e) => (e.kind === "platform" ? [e.change.domain] : []))).toEqual(["identities", "startup"]);
    expect((await cmd.identities(W)).map((i) => i.isMain)).toEqual([true, false]);
    expect(await cmd.dashpayStatus(W)).toEqual({ kind: "ready", main: A });
    const again = restarted(JSON.parse(JSON.stringify(f.world)) as World);
    expect(again.dashpay.identities.find((i) => i.isMain)?.identity).toBe(A);
    expect(await code(cmd.setMainIdentity(W, IDENTITY.bob))).toBe("identity.not_found");
  });

  it("Refresh: the balance current again; Refresh all reports a failure by identity", async () => {
    const f = installFixture({ scenario: "dashpay-identities" });
    f.world.dashpay.identities[0].balanceStale = true;
    expect((await cmd.identityRefresh(W, A)).summary.balanceStale).toBe(false);
    identitiesOf(f.world.dashpay).refreshFailures[WORK_IDENTITY] = "platform.unavailable";
    expect(await code(cmd.identityRefresh(W, WORK_IDENTITY))).toBe("platform.unavailable");
    expect(await cmd.identityRefreshAll(W)).toEqual([
      { identity: A, failureCode: null },
      { identity: WORK_IDENTITY, failureCode: "platform.unavailable" },
    ]);
  });

  it("the look-up: by id or username, own or not, none, refused", async () => {
    installFixture({ scenario: "dashpay-identities" });
    expect(await cmd.identityFind(W, "Bob.dash")).toMatchObject({ identity: IDENTITY.bob, foundBy: { kind: "name", label: "bob" }, names: ["bob"], revision: 2, own: false });
    expect(await cmd.identityFind(W, WORK_IDENTITY)).toMatchObject({ foundBy: { kind: "id" }, names: ["alicework", "aliceatwork"], own: true });
    expect(await cmd.identityFind(W, "nobody")).toBeNull();
    expect(await code(cmd.identityFind(W, "a.b"))).toBe("invalid_argument");
  });

  it("Set as main name picks one of its names only", async () => {
    installFixture({ scenario: "dashpay-identities" });
    await cmd.setMainName(W, WORK_IDENTITY, "aliceatwork");
    expect((await cmd.identities(W))[1].mainName).toBe("aliceatwork");
    expect(await code(cmd.setMainName(W, WORK_IDENTITY, "alice"))).toBe("invalid_argument");
  });

  it("Find my identities finds the seed's third identity once, under an identityScan grant", async () => {
    installFixture({ scenario: "dashpay-identities" });
    const grant = async () => (await cmd.authorize({ kind: "identityScan" }, W, { kind: "passphrase", passphrase: "demo" })).id;
    expect(await cmd.discoverIdentities(W, await grant())).toBe(1);
    expect((await cmd.identities(W)).map((i) => i.identity)).toEqual([A, WORK_IDENTITY, SPARE_IDENTITY]);
    expect(await cmd.discoverIdentities(W, await grant())).toBe(0);
    expect(await code(cmd.discoverIdentities(W, "nope"))).toBe("platform.grant_invalid");
  });
});
