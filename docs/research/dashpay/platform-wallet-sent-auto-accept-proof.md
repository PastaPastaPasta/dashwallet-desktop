# Upstream issue draft: platform-wallet drops `autoAcceptProof` from the sender's own contact request

Status: draft for dashpay/platform (E0-10b L1, accepted as fail-safe by the manager 2026-10-11). Not filed. The
desktop carries no patch (DEC-18); the effect here fails closed (below).

Revisions read: the desktop pin `ebe37f8a67` (PastaPastaPasta/platform) and upstream `v5.0-dev` at `24460b8b4e`
(the file last changed in `7f513f58be`, 2026-09-29). Line numbers are the same within two lines on both; the ones
below are upstream `v5.0-dev`. `PW` = `packages/rs-platform-wallet/src/wallet/identity`.

## Draft

**Title:** `platform-wallet: a sent contact request is stored without its autoAcceptProof`

**Body:**

`send_contact_request_with_external_signer` (`PW/network/contact_requests.rs:409`) resolves the DIP-15
auto-accept proof (`:594`, including the QR-scanner `SignWithKey` variant used by `send_contact_request_from_qr`,
`:768`) and broadcasts it in the `contactRequest` document. The local outgoing row it then builds (`:704`) copies
`encryptedAccountLabel` from the broadcast document (`:718`) but not `autoAcceptProof`, and stores it with
`add_sent_contact_request` (`:734`). So the wallet's own `ContactRequest` has `auto_accept_proof: None` while the
document on Platform has the proof.

A later sweep does not repair it. The sweep's parse reads `autoAcceptProof` (`:3832`), but
`add_sent_contact_request` (`PW/state/managed_identity/contact_requests.rs:74`) is a no-op when the recipient is
already tracked with the same `accountReference`, whether established (`:87`) or pending (`:127`). The row stays
without the proof for its lifetime.

Impact:
- The local row does not match the document that was broadcast. Any consumer comparing the stored request with the
  fetched document (for example, re-verifying it against a proof) sees a difference on every request sent with a
  proof.
- The sender cannot show or re-share the proof it sent.

Fix: set `contact_request.auto_accept_proof` from the broadcast document next to `encrypted_account_label` (`:718`),
read with `to_binary_bytes()` as the other optional byte fields are. A test: send with `SignWithKey`, then check that
the stored sent request's `auto_accept_proof` equals the document's `autoAcceptProof`. Rows already stored without the
proof would need the sent-side guard to fill a missing `auto_accept_proof` from a re-ingested document with the same
`accountReference` (leaving metadata alone), or a one-off reconcile.

Related: #4020 (DashPay backlog, DIP-15 QR auto-accept) does not list this.

## Effect on the desktop (not part of the issue)

E0-10b re-fetches each held contact request and records it verified only when the held `ContactRequest` equals one
the library parses from the fetched documents (`reverify.rs`, DEC-183 digest over every field including
`auto_accept_proof`). A request this wallet sent with a proof is held with `None` and fetched with `Some`, so it never
verifies: once the trusted-quorum fallback has been used it shows as unverified. That is fail-safe and accepted
(manager ruling); it clears with the upstream fix and a reconcile of old rows. Do not relax the comparison here.
