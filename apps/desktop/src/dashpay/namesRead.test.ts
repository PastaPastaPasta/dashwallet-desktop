// `useNamesRead` (D4 reversed, the failure line): its query (`namesReadQuery`) driven through
// TanStack's QueryObserver, the core `useQuery` wraps, with the hook's effect (`rereadAfterSecond`
// on each new `startedAt`) and its answer (`answeredRead`), under fake timers and a mocked backend.

import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Backend } from "../api/backend";
import type { IdentitySummary } from "../api/generated/IdentitySummary";
import type { NameAvailability } from "../api/generated/NameAvailability";
import type { RegistrationStatus } from "../api/generated/RegistrationStatus";
import { rereadAfterSecond, rowSecond, temporaryOutcome, type NamesRead, type TemporaryOutcome } from "./instantOffer";
import { answeredRead, namesReadQuery } from "./queries";

const ROW_AT = 1_800_000_000;
const W = "w1";

const row: RegistrationStatus = {
  draft: "reg-1",
  phase: "contested",
  label: "alice",
  temporaryLabel: "alice2",
  identity: "A",
  txid: null,
  waiting: null,
  holdsKey: false,
  running: false,
  fundsCommitted: true,
  contestEndsAt: null,
  failure: null,
  nameFailure: null,
  profileFailure: null,
  invitation: null,
  fromPlatform: false,
  fromShielded: false,
  createdAt: ROW_AT - 60,
  updatedAt: ROW_AT,
};

const nameless = { identity: "A", names: [], mainName: null, namesUpdating: false, namesCheck: null } as unknown as IdentitySummary;
const FREE: NameAvailability = { kind: "available", contested: false };

/** The hook as it runs: the query's observer, the reread effect on each new `startedAt`, and the
 * outcome every render would show. */
function mount(b: Pick<Backend, "identities" | "nameAvailability">) {
  const client = new QueryClient();
  const observer = new QueryObserver<NamesRead>(client, namesReadQuery(b as Backend, W, row));
  const shown: TemporaryOutcome[] = [];
  let started: number | undefined;
  let cancel: (() => void) | undefined;
  const unsubscribe = observer.subscribe((result) => {
    if (result.data?.startedAt !== started) {
      cancel?.();
      started = result.data?.startedAt;
      cancel = rereadAfterSecond(rowSecond(row), started, () => void observer.refetch());
    }
    shown.push(temporaryOutcome(row, answeredRead(result)));
  });
  const now = () => temporaryOutcome(row, answeredRead(observer.getCurrentResult()));
  return { shown, now, observer, stop: () => (unsubscribe(), cancel?.(), client.clear()) };
}

describe("useNamesRead: the read in the row's own second, then one more", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(ROW_AT * 1000 + 200);
  });
  afterEach(() => vi.useRealTimers());

  it("a read in the row's own second schedules a second identities read at (updatedAt+1)*1000+50; the failure shows only after it", async () => {
    const b = { identities: vi.fn(async () => [nameless]), nameAvailability: vi.fn(async () => FREE) };
    const m = mount(b);
    await vi.advanceTimersByTimeAsync(0);
    expect(b.identities).toHaveBeenCalledTimes(1);
    expect(m.observer.getCurrentResult().data?.startedAt).toBe(ROW_AT * 1000 + 200);
    expect(m.now()).toBeNull();
    // The first read says nothing, so DPNS is not asked yet.
    expect(b.nameAvailability).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(849);
    expect(b.identities).toHaveBeenCalledTimes(1);
    expect(m.shown).not.toContain("failed");

    await vi.advanceTimersByTimeAsync(1);
    expect(b.identities).toHaveBeenCalledTimes(2);
    expect(m.observer.getCurrentResult().data?.startedAt).toBe((ROW_AT + 1) * 1000 + 50);
    expect(b.nameAvailability).toHaveBeenCalledWith(W, "alice2");
    expect(m.now()).toBe("failed");

    // A read after the row's second schedules nothing more.
    await vi.advanceTimersByTimeAsync(60_000);
    expect(b.identities).toHaveBeenCalledTimes(2);
    m.stop();
  });

  it("a read started after the row's second needs no second read", async () => {
    vi.setSystemTime((ROW_AT + 3) * 1000);
    const b = { identities: vi.fn(async () => [nameless]), nameAvailability: vi.fn(async () => FREE) };
    const m = mount(b);
    await vi.advanceTimersByTimeAsync(0);
    expect(m.now()).toBe("failed");
    await vi.advanceTimersByTimeAsync(60_000);
    expect(b.identities).toHaveBeenCalledTimes(1);
    m.stop();
  });

  it("a read that failed (isError) says nothing, not even what the read before it said", async () => {
    vi.setSystemTime((ROW_AT + 3) * 1000);
    let fail = false;
    const b = {
      identities: vi.fn(async () => {
        if (fail) throw new Error("offline");
        return [nameless];
      }),
      nameAvailability: vi.fn(async () => FREE),
    };
    const m = mount(b);
    await vi.advanceTimersByTimeAsync(0);
    expect(m.now()).toBe("failed");
    fail = true;
    await m.observer.refetch();
    expect(m.observer.getCurrentResult().isError).toBe(true);
    expect(answeredRead(m.observer.getCurrentResult())).toBeUndefined();
    expect(m.now()).toBeNull();
    m.stop();
  });

  it("a first read that failed shows neither line", async () => {
    vi.setSystemTime((ROW_AT + 3) * 1000);
    const b = { identities: vi.fn(async (): Promise<IdentitySummary[]> => Promise.reject(new Error("offline"))), nameAvailability: vi.fn(async () => FREE) };
    const m = mount(b);
    await vi.advanceTimersByTimeAsync(0);
    expect(m.observer.getCurrentResult().isError).toBe(true);
    expect(m.shown.every((o) => o === null)).toBe(true);
    expect(b.nameAvailability).not.toHaveBeenCalled();
    m.stop();
  });

  it("the row reaching its contest reads afresh: a read made while it ran never asked DPNS", async () => {
    vi.setSystemTime((ROW_AT + 3) * 1000);
    const b = { identities: vi.fn(async () => [nameless]), nameAvailability: vi.fn(async () => FREE) };
    const client = new QueryClient();
    const running: RegistrationStatus = { ...row, phase: "nameRequested", updatedAt: ROW_AT - 30 };
    const observer = new QueryObserver<NamesRead>(client, namesReadQuery(b as unknown as Backend, W, running));
    const unsubscribe = observer.subscribe(() => {});
    await vi.advanceTimersByTimeAsync(0);
    expect(b.identities).toHaveBeenCalledTimes(1);
    expect(b.nameAvailability).not.toHaveBeenCalled();
    expect(temporaryOutcome(row, answeredRead(observer.getCurrentResult()))).toBeNull();
    observer.setOptions(namesReadQuery(b as unknown as Backend, W, row));
    await vi.advanceTimersByTimeAsync(0);
    expect(b.identities).toHaveBeenCalledTimes(2);
    expect(temporaryOutcome(row, answeredRead(observer.getCurrentResult()))).toBe("failed");
    unsubscribe();
    client.clear();
  });

  it("DPNS says the label is the identity's (a write the engine still holds as pending): no failure", async () => {
    vi.setSystemTime((ROW_AT + 3) * 1000);
    const b = { identities: vi.fn(async () => [nameless]), nameAvailability: vi.fn(async (): Promise<NameAvailability> => ({ kind: "taken", owner: "A" })) };
    const m = mount(b);
    await vi.advanceTimersByTimeAsync(0);
    expect(b.nameAvailability).toHaveBeenCalledTimes(1);
    expect(m.now()).toBeNull();
    m.stop();
  });
});
