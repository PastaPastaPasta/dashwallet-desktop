// Fakes for the Tools view-model tests. Imported by tests only.

import { vi } from "vitest";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { CredentialSpec } from "../api/generated/CredentialSpec";
import type { GrantPurposeSpec } from "../api/generated/GrantPurposeSpec";
import type { LockStatus } from "../api/generated/LockStatus";
import { appError, status } from "../onboarding/testWorld";
import type { GrantAsk } from "./grant";

/** A fake vault: grants for the passphrase `pw` or its own credential, and what was asked. */
export function fakeVault(lock: Partial<LockStatus> = {}) {
  const calls = { authorize: [] as { purpose: GrantAsk; walletId: string | null; credential: CredentialSpec }[], revoked: [] as string[] };
  let issued = 0;
  const api = {
    lockStatus: vi.fn(async () => status(lock)),
    paymentAuthentication: vi.fn(async () => true),
    unlock: vi.fn(async () => status(lock)),
    unlockMixingOnly: vi.fn(async () => status(lock)),
    authorize: vi.fn(async (purpose: GrantPurposeSpec, walletId: string | null, credential: CredentialSpec): Promise<AuthGrant> => {
      calls.authorize.push({ purpose, walletId, credential });
      if (credential.kind === "passphrase" && credential.passphrase !== "pw")
        throw appError("vault.wrong_passphrase", { kind: "wrongPassphrase", failedAttempts: 1, retryAfterSecs: null });
      return { id: `grant-${++issued}`, purpose, walletId, expiresAt: 2_000_000_000, singleUse: true };
    }),
    // A send's grant, issued for its operation: recorded as asked, a `platformOp` grant.
    authorizePlatformSend: vi.fn(async (walletId: string, operation: string, credential: CredentialSpec): Promise<AuthGrant> => {
      calls.authorize.push({ purpose: { kind: "platformSend", operation }, walletId, credential });
      if (credential.kind === "passphrase" && credential.passphrase !== "pw")
        throw appError("vault.wrong_passphrase", { kind: "wrongPassphrase", failedAttempts: 1, retryAfterSecs: null });
      return { id: `grant-${++issued}`, purpose: { kind: "platformOp", maxDuffs: 0, maxCredits: "0" }, walletId, expiresAt: 2_000_000_000, singleUse: true };
    }),
    revokeGrant: vi.fn(async (id: string) => {
      calls.revoked.push(id);
      return null;
    }),
  };
  return { api, calls };
}
