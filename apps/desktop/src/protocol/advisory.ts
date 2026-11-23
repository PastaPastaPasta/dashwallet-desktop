import { useSyncExternalStore } from "react";
import type { ProtocolCompatibility } from "../api/generated/ProtocolCompatibility";
import { messageTable } from "../i18n";

export const PROTOCOL_ADVISORY_COPY = messageTable("home.advisory", ["label", "text", "link", "dismiss", "openFailed"] as const);

const dismissedNetworks = new Set<string>();
const listeners = new Set<() => void>();

function notify() {
  for (const listener of listeners) listener();
}

export function subscribeProtocolAdvisory(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function isProtocolAdvisoryDismissed(networkDirName: string): boolean {
  return dismissedNetworks.has(networkDirName);
}

export function dismissProtocolAdvisory(networkDirName: string): void {
  if (dismissedNetworks.has(networkDirName)) return;
  dismissedNetworks.add(networkDirName);
  notify();
}

export function resetProtocolAdvisoryDismissals(): void {
  if (dismissedNetworks.size === 0) return;
  dismissedNetworks.clear();
  notify();
}

export function useProtocolAdvisoryDismissed(networkDirName: string): boolean {
  return useSyncExternalStore(
    subscribeProtocolAdvisory,
    () => isProtocolAdvisoryDismissed(networkDirName),
    () => false,
  );
}

export function shouldShowProtocolAdvisory(
  compat: ProtocolCompatibility | null | undefined,
  dismissed: boolean,
): boolean {
  return (
    !dismissed &&
    compat != null &&
    compat.networkVersion !== null &&
    compat.networkVersion > compat.supportedVersion
  );
}
