# W-02f: Windows Hello quick unlock

Status: design for the W-02f row of the 1.0 plan (ROADMAP W-02, `docs/design/ROADMAP.md:273`; parity IOS-011,
IOS-016). No code yet. The design gate is MG and pasta; rows marked **MG to confirm** are product or risk calls this
doc recommends but does not settle. Implementation waits for W-02a and for the real-PC session
(`docs/PLAN-1.0.md:64`, by 11-30); H-01 reviews it.

Ruling taken as given (lane N D-N7): the slot B wrap key is protected by a key derived from a Windows Hello
`KeyCredential` signature (a TPM-held key that only signs after Hello verifies the user), not stored readable in
Credential Manager behind a `UserConsentVerifier` prompt, which is a UI gate only.

## 1. What exists on main

| Piece | Where | What it does today |
|---|---|---|
| Key hierarchy, slot B | `docs/design/DESIGN-opus.md:250-262` | DEK wrapped by slot P (Argon2id passphrase), slot B (quick unlock: "Windows: Hello KeyCredential signature over a fixed challenge → HKDF"), slot O (OS store, unencrypted mode only). |
| Credential model | `DESIGN-opus.md:24` (D10), `:293` | The passphrase is the root credential. Quick unlock is offered only where hardware-backed. Quick-unlock grants carry the spending limit (default 0.5 DASH) and the 7-day passphrase rule, enforced in Rust. |
| Slot B in the vault | `rust/crates/dw-vault/src/vault.rs:2586-2627` (`enroll_quick_unlock`), `:2629-2638` (`remove_quick_unlock`), `:1267-1320` (`authorize_quick_unlock`), `file.rs:142` (`slot_b_aad`) | Enrol needs an encrypted vault and a `ChangeCredential` grant (passphrase). It makes a random 256-bit wrap key W, seals the DEK under W (XChaCha20-Poly1305, AAD `dw-vault/slot-b/v1` ‖ vault id ‖ network), returns W and does not keep it. `QuickUnlock{W}` issues `Spend` up to the limit, `SignMessage` and `PlatformOp`; everything else is `vault.credential_required`; a wrong W is `vault.quick_unlock_unavailable` and does not count toward the throttle; a passphrase older than 7 days is `vault.passphrase_stale`. The lock state does not change. |
| Contract | `docs/contracts/m2-engine.md:260-263`, `:371-389`; `m1-engine.md:126`, `:143-154` | As above. `m2-engine.md:283-284` lists dw-ffi's `desktop_quick_unlock_provider` (returns `Unavailable`) and `windows_hello_wrap_key(challenge)` (`NotImplemented`), `rust/crates/dw-ffi/src/api/desktop.rs:362-380`. dw-ffi goes away in U-08. |
| OS secret store | `rust/crates/dw-vault/src/os_store.rs:20-33` (`OsSecretStore`), `:39-104` (`KeyringOsStore`), `:113` (`MemoryOsStore`) | `put/get/delete` by a 32-byte service id and a label, through keyring-core: Windows Credential Manager (DPAPI per user, D-N8). Fails closed. `MemoryOsStore` is the test fake and has an `unavailable()` mode. |
| dw-app commands | `rust/crates/dw-app/src/api/vault.rs:980-995` (`enroll_quick_unlock`), `:999-1004`, `:1007-1010`, `:1044-1055`; `CredentialSpec` `:383-421` | `enroll_quick_unlock` returns W base64 to the renderer, and is refused off macOS (`cfg!(target_os = "macos")`, `:988`). The renderer can send `{kind:"quickUnlock", wrapKey}`. |
| macOS store (Swift, retiring) | `Sources/PlatformServicesMac/MacBiometricKeyStore.swift` | Keeps W in a data-protection keychain item with `.biometryCurrentSet`; `retrieve` runs `LAContext` then reads the item. Cancel → `platform.cancelled`, refusal → `platform.denied`, missing item → `vault.quick_unlock_unavailable`. The Tauri app has no macOS store; Touch ID stays off until B4 (`docs/design/DECISIONS-PENDING.md:14`). |
| Settings row | `apps/desktop/src/screens/settings/Security.tsx:62-68`, copy `apps/desktop/src/settings/security.ts:24-25`; U-04 D11 (`docs/design/U-04-settings.md:120`) | One `NotYetRow`: "Not available on this computer", "Touch ID and Windows Hello are coming soon." |
| Where a credential is asked | `apps/desktop/src/screens/pay/model.ts:276-279` (`credentialPrompt`), `ConfirmSheet.tsx:142`, `:175`, `:214` | Passphrase or none. The lock screen (`LockScreen.tsx:104`, shown while `state === "locked"`, `App.tsx:134`, `:296`) asks only for the passphrase. |
| Windows deps | `rust/crates/dw-desktop/Cargo.toml:28-29` (`windows-sys` 0.59, Win32 only); `rust/Cargo.lock:9760` (`windows` 0.62.2, already built for Tauri) | No WinRT projection in our crates yet. |

## 2. The Windows API, and whether the signature is deterministic

`Windows.Security.Credentials.KeyCredentialManager` (WinRT, Windows 10+, works from an unpackaged Win32 app):

- `IsSupportedAsync()`: Hello is set up for this user (a PIN at least).
- `RequestCreateAsync(name, ReplaceExisting | FailIfExists)`: creates an RSA-2048 key pair in the user's Hello
  (NGC) container, after a Hello prompt. TPM-backed when the PC has a usable TPM, software otherwise.
- `OpenAsync(name)`: no prompt. `KeyCredentialStatus::NotFound` when the key or the whole Hello container is gone.
- `KeyCredential.RetrievePublicKey()`: no prompt.
- `KeyCredential.RequestSignAsync(data)`: a Hello prompt (face, fingerprint or PIN), then a signature.
  Statuses: `Success`, `UserCanceled`, `UserPrefersPassword`, `NotFound`, `SecurityDeviceLocked`, `UnknownError`.
- `KeyCredential.GetAttestationAsync()`: `Success` / `TemporaryFailure` for a TPM key, `NotSupported` for a
  software key (to be confirmed on the real PC, §10 R2).
- `DeleteAsync(name)`.

**Determinism.** Hello keys sign with RSASSA-PKCS1-v1_5 over SHA-256. PKCS#1 v1.5 has no random padding, so one key
signing one message always gives the same signature; a hash of it is a stable secret that only that key, after a
Hello prompt, can reproduce. Two shipping password managers rely on this (read 2026-11-19):
KeePassXC `src/quickunlock/WindowsHello.cpp` (SHA-256 of the signature over a random per-database challenge →
AES-256-GCM key) and Bitwarden `apps/desktop/desktop_native/biometric/src/windows.rs` ("a signing API, that
deterministically signs a challenge, from which a windows hello key is derived").

We do not take it on faith (D4): at enrolment, dw-desktop verifies the signature against the key's public key as
PKCS#1 v1.5 SHA-256 (WinRT `AsymmetricKeyAlgorithmProvider` `RsaSignPkcs1Sha256` + `CryptographicEngine.VerifySignature`,
no new crate, no extra prompt). A signature that verifies under that scheme is the only one that key can produce
for that message. Any other scheme (PSS, ECDSA in a future Windows) fails the check and enrolment is refused with
"can't be used for quick unlock". If a later Windows change made signatures differ, unlock would fail its AEAD
check and fall back to the passphrase (§7), never weaken anything.

There is no Hello API that decrypts with the user-verified key, so "TPM key that decrypts a wrapped secret" means
exactly this: the signature is the decryption key's source.

## 3. Threat model

Assets: the DEK (opens every seed), reachable through slot P (passphrase) or slot B (W). Note that W opens the DEK
itself: the spending limit and the 7-day rule bind our app's use of W, not an attacker who has W and the vault file.

| Attacker | With D-N7 (this design) | With the rejected `UserConsentVerifier` design |
|---|---|---|
| **Another process of the same Windows user** (malware, a malicious extension host) | Can read the Credential Manager blob (DPAPI is per user, not per app) and can call `RequestSignAsync` on our key name. It still needs the user to pass a real Hello prompt, which Windows shows and which it cannot answer. Residual: it can trigger the prompt at a moment the user expects one and get W (R1). The same process can also keylog the passphrase or inject into our process, so same-user code execution is out of scope for full protection, as it is for the passphrase. | Reads W silently with no prompt. This is why D-N7 rejects it. |
| **Offline copy of the disk** (stolen laptop, backup, imaging) | Has `vault.dwv`, the Credential Manager blob (DPAPI master key needs the user's Windows password) and the NGC container. A TPM key cannot sign off this PC, so no K, no W. The vault stays protected by the passphrase's Argon2id cost. With a **software** Hello key the NGC key is protected by the Hello PIN without TPM anti-hammering: a short PIN can be brute-forced offline. D3 refuses enrolment without a TPM key. | DPAPI blob: falls to the Windows password, often weaker than the passphrase. |
| **A copy of the data root to another PC** (user migration, cloud sync of `%APPDATA%`) | The blob is not in the data root (D6). The copied vault still has slot B, but no key on the new PC opens it: Hello can't be used there; the passphrase works. The new PC detects it and removes slot B (§6.6). | Same. |
| **Removed or reset Hello** (user removes the PIN, a destructive PIN reset, a new Windows profile, TPM cleared) | The key is gone (`NotFound`) or replaced. Quick unlock stops; the passphrase is unaffected. §6.6 detects it, removes slot B and says so. | The readable key would keep working with no Hello at all. |
| **Someone who knows the user's Hello PIN** at the signed-in PC | Hello always offers the PIN as a fallback, and `KeyCredentialManager` cannot require biometrics. Through our UI they can spend up to the limit while the passphrase is fresh. With the ability to run their own program in that session they can get W (as in row 1) and with it the DEK. D9 states this in the enable dialog. **MG to confirm** (D9). | Same, plus no prompt at all. |
| **Remote Desktop** | Hello signing is normally not offered in an RDP session; we fall back to the passphrase without removing anything (§7, R4). | — |

Out of scope: an administrator or kernel attacker; a malicious build of the app itself.

## 4. Key hierarchy

```
DEK ── slot P: Argon2id(passphrase)                       (unchanged)
    └─ slot B: XChaCha20-Poly1305(W, DEK, AAD slot-b/v1)   (unchanged, vault.rs:2605-2612)
W  (32 random bytes, made by the vault at enrolment, never stored in clear)
    └─ hello blob: XChaCha20-Poly1305(K, W, AAD = "dw-app/hello-blob/v1" ‖ vault_id ‖ network ‖ key_name ‖ SHA-256(pubkey))
K  = HKDF-SHA256(ikm = S, salt = C, info = "dw-app/hello-kek/v1")
S  = Hello RSA PKCS#1 v1.5 signature over M
M  = "dw-app/hello-challenge/v1" ‖ vault_id ‖ network ‖ C
C  = 32 random bytes, made at enrolment, stored in the blob ("the fixed challenge")
```

- The engine contract does not change: the vault still makes W and hands it out once; Hello protects W the way the
  keychain item does on macOS (D1).
- C is per enrolment, so re-enrolling gives a new M, S, K and W; an old blob opens nothing (D5).
- The public-key hash in the AAD binds the blob to one Hello key; dw-app compares it with `RetrievePublicKey()`
  before prompting, so a reset is found without a pointless prompt (§6.6).
- K, S and W live in `Zeroizing` buffers, on the Rust side only. W never crosses IPC to the renderer (D2).

## 5. What is stored where

| Item | Store | Address | Contents |
|---|---|---|---|
| Slot B, policy | `vault.dwv` (unchanged) | — | as today |
| Hello key | the user's Hello (NGC) container | name `DashWallet.QuickUnlock.<hex16>`, hex16 = first 16 bytes of SHA-256(`"dw-app/hello-name/v1"` ‖ vault_id ‖ network) | RSA-2048, TPM-backed |
| Hello blob | Credential Manager through `OsSecretStore` (D-N8) | service = SHA-256(`"dw-app/hello-blob/v1"` ‖ len ‖ vault_id ‖ network) (the `os_service` pattern, `vault.rs:260`), label `quick-unlock` | `version(1) ‖ C(32) ‖ nonce(24) ‖ ciphertext(32+16)` |

One key and one blob per vault (data root × network): two data roots or networks never share or delete each
other's (D7). Nothing new is written to the data root.

## 6. Flows

Code shape (D8): `dw-desktop/src/hello.rs` (cfg(windows) WinRT calls behind a small `HelloKeys` trait:
`supported`, `create`, `public_key`, `sign`, `attest`, `delete`); `dw-vault/src/hello_blob.rs` (pure: M, K, seal and
open the blob, store addressing; runs on every OS); dw-app commands that tie them to the vault. All WinRT calls run on
a blocking thread (never the Tauri main thread), with the main window's HWND for the prompt's focus (R3).

### 6.1 Provider

`quick_unlock_provider() -> { kind: "windowsHello" | "unavailable", reason? }`, cached per launch and re-read when
the Security page opens. Unavailable reasons: `notSetUp` (`IsSupportedAsync` false), `remoteSession`, `noTpm` (known
only after a key exists; see 6.2), `notWindows`. macOS answers `unavailable` until a Tauri-side Touch ID store exists
(U-08, B4).

### 6.2 Enable ("Use Windows Hello")

1. The renderer asks for the passphrase and gets a `ChangeCredential` grant (as Change passphrase does).
2. `enable_quick_unlock(grant_id)` in dw-app:
   1. `RequestCreateAsync(name, ReplaceExisting)` (Hello prompt 1).
   2. `GetAttestationAsync`: `NotSupported` → delete the key, refuse `quick_unlock.no_tpm` (D3).
   3. New C; `RequestSignAsync(M)` (Hello prompt 2, if Windows asks again: R5); verify S as PKCS#1 v1.5 SHA-256
      against the public key, else delete the key and refuse `quick_unlock.unsupported`.
   4. K = HKDF(S, C). Only now redeem the grant: `Vault::enroll_quick_unlock(grant)` → W. A grant older than 120 s
      is `vault.grant_invalid` ("took too long, try again"); the key is deleted.
   5. Seal W under K, `put` the blob. If `put` fails: `remove_quick_unlock()` (rollback, needs no grant), delete
      the key, report `vault.os_store_unavailable`.
3. Returns the `QuickUnlockPolicy` (enrolled, limit). The renderer never sees W.

Hello steps come before the grant is redeemed so that a cancelled prompt leaves the vault untouched.

### 6.3 Use (a payment, a signed message, a Platform write)

Where it applies is D10 (**MG to confirm**). With the recommended scope, the confirm sheet's credential choice
becomes `windowsHello` when: the vault is encrypted, "Require authentication for every payment" is on, slot B is
enrolled, the amount (fee included) ≤ the limit, and the passphrase is fresher than 7 days (`quick_unlock_policy`
answers all of these while locked). Otherwise it is the passphrase, as today.

`authorize` with `CredentialSpec { kind: "windowsHello" }`: dw-app opens the key (no prompt), compares the public-key
hash, reads the blob, signs M (prompt), derives K, opens W, calls `Vault::authorize_set(…, QuickUnlock(W))`, drops W.
`vault.quick_unlock_limit_exceeded` and `vault.passphrase_stale` turn the sheet to the passphrase with the copy in §8.
A cancelled prompt leaves the sheet open with "Use passphrase instead".

### 6.4 Change the spending limit

As today: passphrase → `ChangeCredential` grant → `set_quick_unlock_spend_limit`. No Hello prompt.

### 6.5 Disable ("Stop using Windows Hello")

`disable_quick_unlock()`: `Vault::remove_quick_unlock()` first (removes the way in; no grant, `vault.rs:2629`), then
delete the blob, then `DeleteAsync(name)`. Host deletions that fail are retried by the sweep (6.7). No passphrase
needed: turning a credential off only removes access.

### 6.6 Hello removed, reset, or the vault copied

Detected on provider read and on every use, before any prompt:
- `OpenAsync` → `NotFound`, or the public-key hash differs from the blob's AAD, or the blob is missing while slot B
  exists → "reset". After a successful prompt: the blob fails to open, or the vault refuses W
  (`vault.quick_unlock_unavailable`) → "reset".
- On "reset": `remove_quick_unlock()`, delete the blob and the key if present, show §8 "Hello was reset" once, and
  use the passphrase. The Security row then shows Off, and the user can enable again (6.2).
- `IsSupportedAsync` false, `SecurityDeviceLocked`, or an RDP session are **not** a reset: nothing is removed; the
  passphrase is used this time (D11).

### 6.7 Recovery, destroy, sweep

- `recover_with_mnemonic` and `destroy` already drop slot B (`m2-engine.md:264-265`); the dw-app commands that call
  them then delete the blob and the key (the job `MacBiometricKeyStore.delete` had for Swift).
- At session open: slot B absent and a blob or key present → delete them (orphans from a failed delete or a crash).
- Change passphrase leaves slot B as it is (the DEK does not change).

## 7. Where Hello applies

The engine never unlocks the vault with W (`m2-engine.md:261`: "`Vault.unlock` with quick unlock is a host flow").
The lock screen covers the whole app while the vault is `Locked` and asks for the passphrase.

| Option | What Hello does | Cost |
|---|---|---|
| **A (recommended for 1.0)** | Replaces typing the passphrase at the confirm sheet (send, PSBT sign, sign message, DashPay and other Platform writes) up to the limit, while the app is unlocked. The lock screen stays passphrase only. | No engine change; one renderer credential kind. |
| B | Also on the lock screen: Hello dismisses the overlay but the vault stays `Locked` (DEK not in memory): balances and history show, spends within the limit prompt Hello again, CoinJoin mixing stays stopped, a reveal or larger spend asks for the passphrase. | A new "UI unlocked, vault locked" state in the shell and the lock reconcile; mixing semantics change. |
| C | Hello fully unlocks the vault. | Puts the DEK in memory on a Hello PIN; breaks the limit model. Not offered. |

D10: A for 1.0, B as a follow-up if users ask. **MG to confirm.**

## 8. Copy

| Situation | Code | Text |
|---|---|---|
| Row, available, off | — | Title "Windows Hello". Sub: "Approve payments up to your limit with your face, fingerprint or Windows Hello PIN instead of your passphrase." Button "Turn on…" |
| Row, on | — | "On. Payments up to {limit} are approved with Windows Hello. Larger payments, showing your recovery phrase and changing security settings still need your passphrase." Limit select (0, 0.1, 0.5, 1, 5 DASH). Button "Turn off" |
| Enable dialog (before the passphrase) | — | "Anyone who can use this Windows account and knows its Windows Hello PIN will be able to approve payments up to your limit. Your passphrase is still needed every 7 days." (D9) |
| Not set up | `quick_unlock.not_set_up` | "Set up Windows Hello in Windows Settings ▸ Accounts ▸ Sign-in options first." |
| No TPM | `quick_unlock.no_tpm` | "This PC keeps its Windows Hello keys in software, not in a security chip (TPM), so DashWallet can't use them to protect your wallet." |
| Unsupported key | `quick_unlock.unsupported` | "Windows Hello on this PC can't be used for quick unlock." |
| Remote session / temporarily unavailable | `quick_unlock.unavailable_now` | "Windows Hello isn't available right now. Enter your passphrase." |
| Cancelled | `platform.cancelled` | (no message; the passphrase field shows) |
| Hello was reset | `quick_unlock.reset` | "Windows Hello was turned off or reset on this PC, so it can no longer approve payments. Enter your passphrase. You can turn Windows Hello on again in Settings ▸ Security." |
| Over the limit | `vault.quick_unlock_limit_exceeded` | "This payment is above your Windows Hello limit of {limit}. Enter your passphrase." |
| Passphrase stale | `vault.passphrase_stale` | "For your security, enter your passphrase. It has been more than 7 days." |
| Took too long | `vault.grant_invalid` | "That took too long. Try again." |
| Store failed | `vault.os_store_unavailable` | "Windows Credential Manager couldn't save the key. Windows Hello was not turned on." |

New codes are dw-app's (`quick_unlock.*`), listed with the other dw-app codes and in the generated TS types.

## 9. Tests

**Fakes.** `FakeHelloKeys` (the `HelloKeys` trait): a per-name secret; `sign` = HMAC-SHA256(secret, M) (deterministic
like PKCS#1 v1.5); modes: not set up, cancel, `NotFound`, key replaced (new secret, new public key), software key
(attestation `NotSupported`), non-deterministic signer, counts prompts. With `MemoryOsStore` (and `unavailable()`).
`hello_blob` needs no fake.

| Test (dw-vault `hello_blob`) | Checks |
|---|---|
| `test_IOS_011_hello_blob_round_trip_and_aad_binding` | seal/open; a different vault id, network, key name or public key fails |
| `test_IOS_011_hello_blob_rejects_a_changed_challenge_or_version` | |
| `test_IOS_011_hello_kek_is_hkdf_of_the_signature` | fixed vector |

| Test (dw-app, fake Hello + `MemoryOsStore`) | Checks |
|---|---|
| `test_IOS_011_enable_then_spend_within_the_limit_with_hello` | one W per enable; `authorize` issues `Spend` ≤ limit |
| `test_IOS_011_enable_cancelled_leaves_the_vault_untouched` | no slot B, no blob, no key; grant not redeemed |
| `test_IOS_011_enable_refuses_a_software_key` / `_a_non_pkcs1_signature` | key deleted, no slot B |
| `test_IOS_011_enable_rolls_back_when_the_store_fails` | slot B removed, key deleted |
| `test_IOS_011_enable_with_an_expired_grant_deletes_the_key` | |
| `test_IOS_011_disable_removes_slot_b_blob_and_key` | and is idempotent |
| `test_IOS_011_hello_reset_removes_slot_b_and_says_so` | `NotFound`, replaced key, missing blob, vault refuses W; no prompt for the first three |
| `test_IOS_011_unavailable_now_keeps_the_enrolment` | |
| `test_IOS_016_over_the_limit_or_stale_asks_for_the_passphrase` | |
| `test_IOS_011_recovery_and_destroy_drop_the_blob_and_key`, `_sweep_removes_orphans` | |
| `test_IOS_011_two_vaults_have_separate_keys_and_blobs` | data roots × networks |
| `test_IOS_011_wrap_key_never_leaves_dw_app` | no command response or event serialises W (scan the JSON of every quick-unlock command) |

Renderer: vitest for the Security row states and `credentialPrompt` with the new kind; Playwright fixture scenarios
(`windowsHello` provider on, off, unavailable, reset) for the row and the confirm sheet, Chromium and WebKit; UI
artifacts per DEC-1484.

Windows CI (`windows-latest`, no Hello): `hello.rs` builds; `supported()` answers false and the provider answers
`unavailable/notSetUp` without panicking or prompting.

**Real-PC checklist** (one Windows 11 PC with a TPM 2.0 and a fingerprint or face sensor, plus, if at hand, a PC or
VM without a TPM; installed NSIS build):
1. Provider reads `windowsHello`; the row offers "Turn on…".
2. Enable: count the prompts (R5); the prompt is in front of the app and takes face/fingerprint (R3).
3. Attestation status on the TPM PC (`Success` or `TemporaryFailure`) and on the no-TPM machine (`NotSupported`) (R2).
4. Send 0.1 DASH (testnet) with Hello; a send above the limit asks for the passphrase.
5. Cancel the prompt: the passphrase field shows; nothing changes.
6. Use the PIN instead of the biometric: works (D9).
7. Set the clock forward 8 days: the passphrase is asked.
8. Turn off in the app: the Credential Manager entry and the key are gone (check with `cmdkey /list` and the key list of the "Microsoft Passport Key Storage Provider"; the exact command is found during the session).
9. Remove the Windows Hello PIN in Windows Settings, then pay: the reset message, slot B gone, passphrase works; set Hello up again and re-enable.
10. Add a fingerprint: quick unlock keeps working (differs from macOS `.biometryCurrentSet`; recorded, not a bug).
11. Over Remote Desktop: passphrase, enrolment kept (R4).
12. Two data roots: each has its own key; turning one off leaves the other working.
13. Close the app during the prompt; reopen: no orphan (sweep).

## 10. Decisions

| # | Decision | Why |
|---|---|---|
| D1 | The vault and its contract do not change. Hello protects the vault-made W in a host blob, the macOS keychain item's role. | `enroll_quick_unlock` already returns W once (`vault.rs:2586`); no key-handling change in dw-vault's slot logic. |
| D2 | W stays in Rust: new dw-app commands `quick_unlock_provider`, `enable_quick_unlock(grant_id)`, `disable_quick_unlock`, and a credential kind `windowsHello` resolved inside dw-app. The renderer's `quickUnlock{wrapKey}` path and `enroll_quick_unlock`'s returned key are not used on Windows. | The webview is the larger attack surface; W opens the DEK. |
| D3 | Enrolment needs a TPM-backed Hello key (attestation not `NotSupported`). **MG to confirm.** | A software Hello key falls to an offline PIN brute force (§3), weaker than the passphrase it stands in for. |
| D4 | Verify each enrolment signature as RSA PKCS#1 v1.5 SHA-256 against the key's public key; refuse anything else. | Proves determinism without a second prompt. |
| D5 | Random 32-byte challenge per enrolment, stored in the blob; K = HKDF-SHA256 with a domain-separated message. | Re-enrolment invalidates the old blob; no cross-protocol reuse of the signature. |
| D6 | The blob lives in Credential Manager (D-N8), not in the data root. | Not in backups or synced folders; DPAPI adds a layer. |
| D7 | One Hello key and one blob per vault (vault id × network). | Disabling or resetting one data root or network never touches another. |
| D8 | WinRT code in `dw-desktop/src/hello.rs` via the `windows` crate (0.62, already in the lockfile) behind a `HelloKeys` trait; the pure blob code in `dw-vault/src/hello_blob.rs` (it uses the vault's AEAD; `hkdf` 0.12 becomes a direct dep, already in the lockfile). | Testable on Linux with fakes; the crypto stays in the crate that already holds it. |
| D9 | Accept Hello's PIN fallback and say so in the enable dialog. **MG to confirm.** | `KeyCredentialManager` can't require biometrics; the limit and 7-day rule bound UI use. |
| D10 | Scope A: Hello at the confirm sheet only; the lock screen stays passphrase only. **MG to confirm.** | No engine or shell lock-state change in 1.0 (§7). |
| D11 | Only `NotFound`, a changed public key, a missing or unopenable blob, or the vault refusing W remove the enrolment; "unavailable now" never does. | Remote sessions and a locked TPM are temporary. |
| D12 | Spending limit choices and default as today (0/0.1/0.5/1/5 DASH, 0.5). | `m2-engine.md:263`, IOS-016. |
| D13 | Implementation ~500 non-test lines in three PRs: (1) `hello_blob` + `hello.rs` + dw-app commands; (2) the Security row and confirm sheet; (3) doc updates (parity IOS-011/016, `m2-engine.md:283-284` and `:387-388`, U-04 D11). | Lane N's estimate; keeps each PR under the size check. |

## 11. Open risks

| # | Risk | Plan |
|---|---|---|
| R1 | Same-user malware can call our key and ride a real Hello prompt the user approves (Bitwarden documents the same residual). | Accepted (§3); documented for H-01. Packaging as MSIX would scope keys per package; not in 1.0. |
| R2 | `GetAttestationAsync` may not separate TPM from software keys as expected, or may need a network round trip. | Real-PC step 3. If it doesn't, use the NCrypt Platform Crypto Provider presence instead, or **MG to confirm** dropping D3. |
| R3 | From an unpackaged Win32 app the Hello prompt can open behind the window, and face/fingerprint then don't start; KeePassXC and Bitwarden both bring "Credential Dialog Xaml Host" to the front by hand. | Same workaround (FindWindow + SetForegroundWindow, retried briefly); real-PC step 2. |
| R4 | Behaviour over RDP and with `SecurityDeviceLocked` is not documented precisely. | D11; real-PC step 11. |
| R5 | Enable may show two prompts (create, then sign). | Acceptable; record the count. |
| R6 | No CI can exercise Hello (`windows-latest` has none); the T-04 VM (`ROADMAP.md:97`) has no TPM-backed Hello either. | Fakes plus the real-PC checklist; if the 11-30 session slips, Windows 1.0 ships passphrase only (`PLAN-1.0.md:64`). |
