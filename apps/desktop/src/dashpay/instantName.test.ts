// The instant username's form (DP1-09 D4 reversed): iOS's suggestion, its checks in iOS's order
// (`CreateUsernameViewModel.swift` 1915-2000) and the debounced availability lookup.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { NameAvailability } from "../api/generated/NameAvailability";
import type { UsernameCheck } from "../api/generated/UsernameCheck";
import { checkUsername } from "../fixture/registration";
import { AvailabilityChecker, instantCheck, suggest } from "./instantName";
import { INSTANT_COPY } from "./instantOffer";
import { MAX_LABEL, RULE_TEXT } from "./registration";

describe("suggest (iOS 1915-1933)", () => {
  it("adds a 2 to the request", () => {
    expect(suggest("alice")).toBe("alice2");
    expect(suggest(" bob ")).toBe("bob2");
  });

  it("trims a request at the cap (23) first, not one a character under it", () => {
    expect(MAX_LABEL).toBe(23);
    const at = "a".repeat(23);
    const under = "b".repeat(22);
    expect(suggest(at)).toBe(`${"a".repeat(22)}2`);
    expect(suggest(at)).toHaveLength(23);
    expect(suggest(under)).toBe(`${under}2`);
    expect(suggest(under)).toHaveLength(23);
  });

  it("takes a contested request out of the contested set", () => {
    expect(checkUsername("alice").contested).toBe(true);
    expect(checkUsername(suggest("alice")).contested).toBe(false);
  });
});

describe("instantCheck: iOS's order, then availability", () => {
  const available: NameAvailability = { kind: "available", contested: false };
  const check = (patch: Partial<UsernameCheck>): UsernameCheck => ({ ...checkUsername("alice2"), ...patch });

  it("an empty field and an unread checklist say nothing and cannot go on", () => {
    expect(instantCheck("alice", "", undefined, undefined, false)).toEqual({ kind: "empty", line: null, ok: false });
    expect(instantCheck("alice", "al", undefined, undefined, false)).toEqual({ kind: "rules", line: null, ok: false });
  });

  it("1. the rules first: the first broken rule's line", () => {
    const c = instantCheck("alice", "a", checkUsername("a"), available, false);
    expect(c).toEqual({ kind: "rules", line: { text: RULE_TEXT.minLength, tone: "problem" }, ok: false });
  });

  it("2. then not contested, even when it would also be the request", () => {
    const c = instantCheck("alice", "alice", checkUsername("alice"), available, false);
    expect(c).toEqual({ kind: "contested", line: { text: INSTANT_COPY.contested, tone: "problem" }, ok: false });
    expect(INSTANT_COPY.contested).toBe("This username would also need a vote. Add a digit from 2 to 9, or use 20 characters or more.");
  });

  it("3. then not the request (Android), under DPNS's homograph folding", () => {
    const c = instantCheck("al1ce", "alice2", check({ contested: false, normalized: "a11ce" }), available, false);
    expect(c).toEqual({ kind: "same", line: { text: INSTANT_COPY.same, tone: "problem" }, ok: false });
    expect(INSTANT_COPY.same).toBe("Your instant username must be different from the contested one");
  });

  it("4. then the network's availability, in step 1's words", () => {
    expect(instantCheck("alice", "alice2", check({}), undefined, true)).toMatchObject({ kind: "availability", line: { text: "Checking…", tone: "pending" }, ok: false });
    expect(instantCheck("alice", "alice2", check({}), available, false)).toMatchObject({ line: { text: "Username available", tone: "ok" }, ok: true });
    expect(instantCheck("alice", "alice2", check({}), { kind: "taken", owner: null }, false)).toMatchObject({ line: { text: "Username taken", tone: "problem" }, ok: false });
    // A lookup that failed lets the user go on; the start decides.
    expect(instantCheck("alice", "alice2", check({}), { kind: "unknown" }, false).ok).toBe(true);
  });
});

describe("AvailabilityChecker: debounced 400 ms, stale answers dropped", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("asks once the label has stayed the same for 400 ms", async () => {
    const lookup = vi.fn(async (_: string): Promise<NameAvailability> => ({ kind: "available", contested: false }));
    const c = new AvailabilityChecker(lookup);
    c.set("al");
    c.set("ali");
    c.set("alice2");
    expect(c.pending).toBe(true);
    await vi.advanceTimersByTimeAsync(399);
    expect(lookup).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(lookup.mock.calls.map(([l]) => l)).toEqual(["alice2"]);
    expect(c.availability).toEqual({ kind: "available", contested: false });
    expect(c.pending).toBe(false);
  });

  it("drops the answer for a label no longer set", async () => {
    const answers = new Map<string, (a: NameAvailability) => void>();
    const c = new AvailabilityChecker((label) => new Promise((resolve) => answers.set(label, resolve)));
    c.set("alice2");
    await vi.advanceTimersByTimeAsync(400);
    c.set("alice3");
    await vi.advanceTimersByTimeAsync(400);
    answers.get("alice3")!({ kind: "taken", owner: null });
    answers.get("alice2")!({ kind: "available", contested: false });
    await vi.advanceTimersByTimeAsync(0);
    expect(c.label).toBe("alice3");
    expect(c.availability).toEqual({ kind: "taken", owner: null });
  });

  it("A, B, then A again waits for a new answer for A instead of reusing the first", async () => {
    const answers: ((a: NameAvailability) => void)[] = [];
    const lookup = vi.fn((_: string) => new Promise<NameAvailability>((resolve) => answers.push(resolve)));
    const c = new AvailabilityChecker(lookup);
    c.set("alice2");
    await vi.advanceTimersByTimeAsync(400);
    answers[0]({ kind: "available", contested: false });
    await vi.advanceTimersByTimeAsync(0);
    expect(c.pending).toBe(false);
    c.set("alice3");
    c.set("alice2");
    expect(c.availability).toBeUndefined();
    expect(c.pending).toBe(true);
    await vi.advanceTimersByTimeAsync(400);
    expect(lookup).toHaveBeenCalledTimes(2);
    expect(c.pending).toBe(true);
    answers[1]({ kind: "taken", owner: null });
    await vi.advanceTimersByTimeAsync(0);
    expect(c.availability).toEqual({ kind: "taken", owner: null });
  });

  it("set again after dispose (StrictMode's effect replay) asks again instead of staying pending", async () => {
    const lookup = vi.fn(async (_: string): Promise<NameAvailability> => ({ kind: "available", contested: false }));
    const c = new AvailabilityChecker(lookup);
    c.set("alice2");
    c.dispose();
    c.set("alice2");
    await vi.advanceTimersByTimeAsync(400);
    expect(lookup).toHaveBeenCalledTimes(1);
    expect(c.pending).toBe(false);
  });

  it("a label cleared (it failed an earlier check) asks nothing; a failed lookup reads unknown", async () => {
    const lookup = vi.fn(async (): Promise<NameAvailability> => Promise.reject(new Error("offline")));
    const c = new AvailabilityChecker(lookup);
    c.set("alice2");
    c.set(null);
    await vi.advanceTimersByTimeAsync(400);
    expect(lookup).not.toHaveBeenCalled();
    expect(c.pending).toBe(false);
    c.set("alice2");
    await vi.advanceTimersByTimeAsync(400);
    expect(c.availability).toEqual({ kind: "unknown" });
  });
});
