import { beforeEach, describe, expect, it } from "vitest";
import {
  dismissProtocolAdvisory,
  isProtocolAdvisoryDismissed,
  PROTOCOL_ADVISORY_COPY,
  resetProtocolAdvisoryDismissals,
  shouldShowProtocolAdvisory,
  subscribeProtocolAdvisory,
} from "./advisory";

beforeEach(() => {
  resetProtocolAdvisoryDismissals();
});

describe("protocol advisory model", () => {
  it("shows only when the network version is known and exceeds the supported version", () => {
    expect(shouldShowProtocolAdvisory(undefined, false)).toBe(false);
    expect(shouldShowProtocolAdvisory(null, false)).toBe(false);
    expect(shouldShowProtocolAdvisory({ networkVersion: null, supportedVersion: 15 }, false)).toBe(false);
    expect(shouldShowProtocolAdvisory({ networkVersion: 14, supportedVersion: 15 }, false)).toBe(false);
    expect(shouldShowProtocolAdvisory({ networkVersion: 15, supportedVersion: 15 }, false)).toBe(false);
    expect(shouldShowProtocolAdvisory({ networkVersion: 16, supportedVersion: 15 }, false)).toBe(true);
    expect(shouldShowProtocolAdvisory({ networkVersion: 16, supportedVersion: 15 }, true)).toBe(false);
  });

  it("keeps session dismissal per network and notifies subscribers", () => {
    let notifications = 0;
    const stop = subscribeProtocolAdvisory(() => {
      notifications += 1;
    });
    expect(isProtocolAdvisoryDismissed("testnet")).toBe(false);
    dismissProtocolAdvisory("testnet");
    expect(isProtocolAdvisoryDismissed("testnet")).toBe(true);
    expect(isProtocolAdvisoryDismissed("mainnet")).toBe(false);
    expect(notifications).toBe(1);
    dismissProtocolAdvisory("testnet");
    expect(notifications).toBe(1);
    resetProtocolAdvisoryDismissals();
    expect(isProtocolAdvisoryDismissed("testnet")).toBe(false);
    expect(notifications).toBe(2);
    stop();
  });

  it("carries the required advisory copy", () => {
    expect(PROTOCOL_ADVISORY_COPY.text).toBe("The network has upgraded. Check for an app update.");
  });
});
