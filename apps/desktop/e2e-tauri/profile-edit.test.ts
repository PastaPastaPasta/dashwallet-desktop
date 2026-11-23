// profile-edit.ts against a stand-in app (BL-162): the edit is recorded pending before it is
// submitted, done only once the profile reads it back, and a rerun settles a pending edit by
// reading the profile, never by paying for another.

import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { writeProfile } from "./profile-edit.ts";

describe("the BL-162 profile edit and its ledger", () => {
  let dir: string;
  let ledger: string;
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), "dw-profile-edit-"));
    ledger = join(dir, "BL-162.json");
  });
  afterEach(() => rmSync(dir, { recursive: true, force: true }));

  /** A stand-in app: `answers[cmd]` is returned (or thrown, if an Error); every call is recorded. */
  const edit = (answers: Record<string, unknown>, label: string, opts: { again?: boolean; ledger?: string } = {}) => {
    const calls: string[] = [];
    const shots: string[] = [];
    const run = writeProfile(
      async (cmd) => {
        calls.push(cmd);
        const a = answers[cmd];
        if (a instanceof Error) throw a;
        return a as never;
      },
      async (name) => void shots.push(name),
      { walletId: "w", identity: "idA", passphrase: "p", phase: "Synced", label, writeAt: "synced", ledger: opts.ledger ?? ledger, again: opts.again ?? false },
      () => undefined,
      (s) => s,
    );
    return { run, calls, shots };
  };
  const state = () => JSON.parse(readFileSync(ledger, "utf8")) as { state: string; label: string };
  const granted = { authorize: { ok: { id: "g1" } } };

  it("leaves a timed-out submit pending, and the rerun reads the profile back instead of submitting", async () => {
    const first = edit({ ...granted, dashpay_update_profile: new Error("POST /execute/async: script timeout") }, "BL-162 11:00");
    await expect(first.run).rejects.toThrow(/script timeout/);
    expect(first.calls).toEqual(["authorize", "dashpay_update_profile"]);
    expect(state()).toMatchObject({ state: "pending", label: "BL-162 11:00" });

    const rerun = edit({ ...granted, dashpay_update_profile: { ok: null }, dashpay_profile: { ok: { displayName: "BL-162 11:00" } } }, "BL-162 11:30");
    await expect(rerun.run).resolves.toMatchObject({ ok: true, resumed: true, label: "BL-162 11:00" });
    expect(rerun.calls).toEqual(["dashpay_profile"]);
    expect(state()).toMatchObject({ state: "done", label: "BL-162 11:00" });
  });

  it("stops a rerun whose pending edit the profile does not show, until DW_PROFILE_WRITE_AGAIN=1", async () => {
    await expect(edit({ ...granted, dashpay_update_profile: new Error("timeout") }, "BL-162 11:00").run).rejects.toThrow();
    const rerun = edit({ ...granted, dashpay_update_profile: { ok: null }, dashpay_profile: { ok: { displayName: "QA A" } } }, "BL-162 11:30");
    await expect(rerun.run).rejects.toThrow(/outcome is unknown, and the profile reads "QA A": not submitting; wait and rerun.*DW_PROFILE_WRITE_AGAIN=1/);
    expect(rerun.calls).toEqual(["dashpay_profile"]);
    expect(state()).toMatchObject({ state: "pending", label: "BL-162 11:00" });

    const again = edit({ ...granted, dashpay_update_profile: { ok: null }, dashpay_profile: { ok: { displayName: "BL-162 11:30" } } }, "BL-162 11:30", { again: true });
    await expect(again.run).resolves.toMatchObject({ ok: true, label: "BL-162 11:30" });
    expect(again.calls).toEqual(["authorize", "dashpay_update_profile", "dashpay_profile"]);
    expect(state()).toMatchObject({ state: "done", label: "BL-162 11:30" });
  });

  it("settles nothing and suggests no new edit when the pending edit's profile cannot be read", async () => {
    await expect(edit({ ...granted, dashpay_update_profile: new Error("timeout") }, "BL-162 11:00").run).rejects.toThrow();
    const rerun = edit({ ...granted, dashpay_update_profile: { ok: null }, dashpay_profile: { err: { kind: "platform" } } }, "BL-162 11:30");
    await expect(rerun.run).rejects.toThrow(/the profile could not be read: not submitting; rerun later$/);
    expect(rerun.calls).toEqual(["dashpay_profile"]);
    expect(state()).toMatchObject({ state: "pending", label: "BL-162 11:00" });
  });

  it("submits nothing over a done edit, or a ledger it does not understand, unless DW_PROFILE_WRITE_AGAIN=1", async () => {
    const answers = { ...granted, dashpay_update_profile: { ok: null }, dashpay_profile: { ok: { displayName: "BL-162 11:00" } } };
    await expect(edit(answers, "BL-162 11:00").run).resolves.toMatchObject({ ok: true });
    const rerun = edit(answers, "BL-162 11:30");
    await expect(rerun.run).rejects.toThrow(/holds an edit to "BL-162 11:00", done: not submitting another/);
    expect(rerun.calls).toEqual([]);
    writeFileSync(ledger, JSON.stringify({ state: "sent", identity: "idA", label: "x" }));
    const odd = edit(answers, "BL-162 11:30");
    await expect(odd.run).rejects.toThrow(/in state "sent": not submitting/);
    expect(odd.calls).toEqual([]);
  });

  it("submits nothing when the pending record cannot be written", async () => {
    const e = edit({ ...granted, dashpay_update_profile: { ok: null } }, "BL-162 11:00", { ledger: join(dir, "missing", "BL-162.json") });
    await expect(e.run).rejects.toThrow(/could not be written; nothing submitted/);
    expect(e.calls).not.toContain("dashpay_update_profile");
    expect(e.shots).toEqual([]);
  });

  it("fails, leaving the ledger pending, when the read back shows the old name or an error", async () => {
    for (const back of [{ ok: { displayName: "QA A" } }, { err: { kind: "platform", message: "unavailable" } }]) {
      const e = edit({ ...granted, dashpay_update_profile: { ok: null }, dashpay_profile: back }, "BL-162 11:00");
      await expect(e.run).rejects.toThrow(/the edit reported success but the profile reads .*; .* stays pending/);
      expect(state()).toMatchObject({ state: "pending", label: "BL-162 11:00" });
      rmSync(ledger);
    }
    expect(existsSync(ledger)).toBe(false);
  });
});
