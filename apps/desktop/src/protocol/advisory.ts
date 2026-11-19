import { useSyncExternalStore } from "react";
import type { ProtocolCompatibility } from "../api/generated/ProtocolCompatibility";

export const PROTOCOL_ADVISORY_COPY = {
  label: "App update available",
  text: "The network has upgraded. Check for an app update.",
  link: "GitHub releases",
  dismiss: "Dismiss",
  openFailed: "Could not open the releases page.",
} as const;

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
