// "Include my username in payment requests" in the renderer (DP3-03b D22): no `user=` shows
// unless the setting is known to be on when the URI is shown. A change under way, a failed
// read and a setting not read yet all count as off.

import type { ReceiveRequest } from "./generated/ReceiveRequest";

/** Changes under way per wallet: its setting reads off until all are done. A count, as a card
 * remounted mid-change can start another before the first is done. */
const changing = new Map<string, number>();

/** Marks the wallet's setting as being changed, until the returned function is called. */
export function changingUsernameInRequests(walletId: string): () => void {
  changing.set(walletId, (changing.get(walletId) ?? 0) + 1);
  let done = false;
  return () => {
    if (done) return;
    done = true;
    const left = changing.get(walletId)! - 1;
    if (left === 0) changing.delete(walletId);
    else changing.set(walletId, left);
  };
}

export function isChangingUsernameInRequests(walletId: string): boolean {
  return changing.has(walletId);
}

/**
 * The request as shown while the username may not be: dash-qt's plain URI and no username. The
 * engine appends `user=` as the URI's last parameter, percent-encoded (dw-uri
 * `format_request_uri`), so the URI without it is `format_bitcoin_uri`'s.
 */
export function withoutUsername(request: ReceiveRequest): ReceiveRequest {
  if (request.username === null) return request;
  return { ...request, username: null, uri: request.uri.replace(/[?&]user=[^&]*$/, "") };
}
