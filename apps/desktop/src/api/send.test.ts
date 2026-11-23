import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { closeSendDraft, SIGNED_BEFORE_LOCK } from "./send";

describe("closeSendDraft", () => {
  beforeEach(() => vi.stubGlobal("window", globalThis));
  afterEach(() => {
    clearMocks();
    vi.unstubAllGlobals();
  });

  it("warns that a running broadcast may still send", async () => {
    const closed: unknown[] = [];
    mockIPC((cmd, args) => {
      expect(cmd).toBe("close_draft");
      closed.push(args);
      return true;
    });
    expect(await closeSendDraft(7, true)).toEqual({ closed: true, warning: SIGNED_BEFORE_LOCK });
    expect(await closeSendDraft(7, false)).toEqual({ closed: true, warning: null });
    expect(closed).toEqual([{ draft: 7 }, { draft: 7 }]);
  });
});
