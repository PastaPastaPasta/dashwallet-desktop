# W-02f: Windows Hello quick unlock

Status: design for the W-02f row of the 1.0 plan (ROADMAP W-02, `docs/design/ROADMAP.md:273`; parity IOS-011,
IOS-016). No code yet. The design gate is MG and pasta. The product and risk calls D3, D9 and D10 and the
disclosed residual R1 were accepted by MG on 2026-11-19. Implementation waits for W-02a and for the real-PC session
(`docs/PLAN-1.0.md:64`, by 11-30); H-01 reviews it.

Ruling taken as given (lane N D-N7): the slot B wrap key is protected by a key derived from a Windows Hello
`KeyCredential` signature (a TPM-held key that only signs after Hello verifies the user), not stored readable in
Credential Manager behind a `UserConsentVerifier` prompt, which is a UI gate only.

## 1. What exists on main

| Piece | Where | What it does today |
|---|---|---|
| Key hierarchy, slot B | `docs/design/DESIGN-opus.md:250-262` | DEK wrapped by slot P (Argon2id passphrase), slot B (quick unlock: "Windows: Hello KeyCredential signature over a fixed challenge → HKDF"), slot O (OS store, unencrypted mode only). |
| Credential model | `DESIGN-opus.md:24` (D10), `:293` | The passphrase is the root credential. Quick unlock is offered only where hardware-backed. Quick-unlock grants carry the spending limit (default 0.5 DASH) and the 7-day passphrase rule, enforced in Rust. |
| Slot B in the vault | `rust/crates/dw-vault/src/vault.rs:2586-2627` (`enroll_quick_unlock`), `:2629-2638` (`remove_quick_unlock`), `:1267-1320` (`authorize_quick_unlock`), `file.rs:142` (`slot_b_aad`) | Enrol needs an encrypted vault and a `ChangeCredential` grant (passphrase). It makes a random 256-bit wrap key W, seals the DEK under W (XChaCha20-Poly1305, AAD `dw-vault/slot-b/v1` ‖ vault id ‖ network), returns W and does not keep it. `QuickUnlock{W}` issues `Spend`, `MasternodeOp`, `PlatformOp` and `MasternodeWithdraw` up to the limit (each valued in duffs, credits rounded up), and `SignMessage` and `Governance` (valued 0) (`types.rs:215-229`, `m1-engine.md:143-154`); `RevealSecret`, `ChangeCredential`, `Wipe` and `IdentityScan` are `vault.credential_required`; a wrong W is `vault.quick_unlock_unavailable` and does not count toward the throttle; a passphrase older than 7 days is `vault.passphrase_stale`. The lock state does not change. |
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
- `KeyCredential.GetAttestationAsync()`: `Success` with an attestation for a TPM key; `TemporaryFailure` when an
  attestation can't be had right now; `NotSupported` (expected for a software key) or `UnknownError` otherwise. Only
  `Success` is taken as proof of a TPM key (D3); to be confirmed on the real PC (§11 R2).
- A per-key hardware property, the fallback if attestation proves unreliable: the key opened through NCrypt's
  "Microsoft Passport Key Storage Provider" with `NCRYPT_IMPL_TYPE_PROPERTY` carrying `NCRYPT_IMPL_HARDWARE_FLAG`
  (unverified; real-PC step 3). It may replace attestation only after the session has seen it **set** on a TPM Hello
  key and **clear** on a software Hello key (a Hyper-V VM without a vTPM is enough). TPM presence on the PC is not
  enough: a PC with a TPM can still keep its Hello container in software.
- `DeleteAsync(name)`.

**Determinism.** Hello keys sign with RSASSA-PKCS1-v1_5 over SHA-256. PKCS#1 v1.5 has no random padding, so one key
signing one message always gives the same signature; a hash of it is a stable secret that only that key, after a
Hello prompt, can reproduce. Two shipping password managers rely on this (read 2026-11-19):
KeePassXC `src/quickunlock/WindowsHello.cpp` (SHA-256 of the signature over a random per-database challenge →
AES-256-GCM key) and Bitwarden `apps/desktop/desktop_native/biometric/src/windows.rs` ("a signing API, that
deterministically signs a challenge, from which a windows hello key is derived").

We do not take it on faith (D4): at enrolment, dw-desktop verifies the signature against the key's public key
(`RetrievePublicKey(X509SubjectPublicKeyInfo)`) as PKCS#1 v1.5 SHA-256 (WinRT `AsymmetricKeyAlgorithmProvider`
`RsaSignPkcs1Sha256` + `CryptographicEngine.VerifySignature`, no new crate, no extra prompt). It verifies with the
`IBuffer` that `RequestSignAsync` returned, then copies S out and zeroes that buffer, so no second copy of S exists. A signature that verifies under that scheme is the only one that key can produce
for that message. Any other scheme (PSS, ECDSA in a future Windows) fails the check and enrolment is refused with
"can't be used for quick unlock". If a later Windows change made signatures differ, the blob would fail its AEAD
check, which §6.6 treats as a reset: the enrolment is removed and the passphrase is used. Nothing is weakened.

There is no Hello API that decrypts with the user-verified key, so "TPM key that decrypts a wrapped secret" means
exactly this: the signature is the decryption key's source.

## 3. Threat model

Assets: the DEK (opens every seed), reachable through slot P (passphrase) or slot B (W). Note that W opens the DEK
itself: the spending limit and the 7-day rule bind our app's use of W, not an attacker who has W and the vault file.

| Attacker | With D-N7 (this design) | With the rejected `UserConsentVerifier` design |
|---|---|---|
| **Another process of the same Windows user** (malware, a malicious extension host) | Can read the Credential Manager blob (DPAPI is per user, not per app) and can call `RequestSignAsync` on our key name. It still needs the user to pass a real Hello prompt, which Windows shows and which it cannot answer. Residual (R1, accepted with disclosure): if the user approves one such prompt, the process gets S, then K, then W, and with `vault.dwv` the data key, which opens **every seed in the vault**, outside the spending limit and the 7-day rule. It is **permanent**: the data key never rotates, not even on a passphrase change (`vault.rs:957-965`), so turning Hello off or re-enrolling afterwards does not undo it. The prompt cannot say what is being approved or who asked: `RequestSignAsync` takes no message, and a `UserConsentVerifier` text in front of our own calls does not gate the attacker's. Compared with a passphrase keylogger, which the same process could also run, the attacker gains only the choice of moment and a generic prompt; the enable dialog and the confirm sheet tell the user to approve a prompt only right after confirming in DashWallet (§8). | Reads W silently with no prompt. This is why D-N7 rejects it. |
| **Offline copy of the disk** (stolen laptop, backup, imaging) | Has `vault.dwv`, the Credential Manager blob (DPAPI master key needs the user's Windows password) and the NGC container. A TPM key cannot sign off this PC, so no K, no W. The vault stays protected by the passphrase's Argon2id cost. With a **software** Hello key the NGC key is protected by the Hello PIN without TPM anti-hammering: a short PIN can be brute-forced offline. D3 refuses enrolment without a TPM key. | DPAPI blob: falls to the Windows password, often weaker than the passphrase. |
| **A copy of the data root to another PC** (user migration, cloud sync of `%APPDATA%`) | The blob is not in the data root (D6). The copied vault still has slot B, but no key on the new PC opens it: Hello can't be used there; the passphrase works. The new PC detects it and removes slot B (§6.6). | Same. |
| **Removed or reset Hello** (user removes the PIN, a destructive PIN reset, a new Windows profile, TPM cleared) | The key is gone (`NotFound`) or replaced. Quick unlock stops; the passphrase is unaffected. §6.6 detects it, removes slot B and says so. | The readable key would keep working with no Hello at all. |
| **Someone who knows the user's Hello PIN** at the signed-in PC | Hello always offers the PIN as a fallback, and `KeyCredentialManager` cannot require biometrics. Through our UI they can spend up to the limit while the passphrase is fresh. With the ability to run their own program in that session they can get W (as in row 1) and with it the DEK. D9 (accepted) states this in the enable dialog. | Same, plus no prompt at all. |
| **Remote Desktop** | Hello signing is normally not offered in an RDP session; we fall back to the passphrase without removing anything (§6.6, R4). | — |

Out of scope: an administrator or kernel attacker; a malicious build of the app itself.

## 4. Key hierarchy

```
DEK ── slot P: Argon2id(passphrase)                       (unchanged)
    └─ slot B: XChaCha20-Poly1305(W, DEK, AAD slot-b/v1)   (unchanged, vault.rs:2605-2612)
W  (32 random bytes, made by the vault at enrolment, never stored in clear)
    └─ hello blob: XChaCha20-Poly1305(K, W, AAD = "dw-app/hello-blob/v1" ‖ f(vault_id) ‖ f(network) ‖ f(key_name) ‖ f(SHA-256(pubkey)))
K  = HKDF-SHA256(ikm = S, salt = C, info = "dw-app/hello-kek/v1")
S  = Hello RSA PKCS#1 v1.5 signature over M
M  = "dw-app/hello-challenge/v1" ‖ f(vault_id) ‖ f(network) ‖ f(C)
C  = 32 random bytes, made at enrolment, stored in the blob ("the fixed challenge")
f  = the vault's field framing, u32 LE length ‖ bytes (`push_field`, file.rs:112-115)
```

- The engine contract does not change: the vault still makes W and hands it out once; Hello protects W the way the
  keychain item does on macOS (D1).
- C is per enrolment, so re-enrolling gives a new M, S, K and W; an old blob opens nothing (D5).
- The public-key hash in the AAD binds the blob to one Hello key; dw-app compares it with `RetrievePublicKey()`
  before prompting, so a reset is found without a pointless prompt (§6.6).
- K, S and W live in `Zeroizing` buffers, on the Rust side only. W never crosses IPC to the renderer (D2).
- S arrives in a WinRT `IBuffer` that `Zeroizing` does not cover. dw-desktop verifies with that same buffer (D4),
  then copies S out and zeroes the buffer through `IBufferByteAccess` before releasing it. No other `IBuffer` ever
  holds S.

## 5. What is stored where

| Item | Store | Address | Contents |
|---|---|---|---|
| Slot B, policy | `vault.dwv` (unchanged) | — | as today |
| Hello key | the user's Hello (NGC) container | name `DashWallet.QuickUnlock.<hex16>`, hex16 = first 16 bytes of SHA-256(`"dw-app/hello-name/v1"` ‖ f(vault_id) ‖ f(network)) | RSA-2048, TPM-backed |
| Hello blob | Credential Manager through `OsSecretStore` (D-N8) | service = SHA-256(`"dw-app/hello-blob/v1"` ‖ len ‖ vault_id ‖ network) (the `os_service` pattern, `vault.rs:260`), label `quick-unlock` | `version(1) ‖ C(32) ‖ SHA-256(pubkey)(32) ‖ nonce(24) ‖ ciphertext(32+16)`, pubkey = `RetrievePublicKey(X509SubjectPublicKeyInfo)`; the hash is in the clear for the check before the prompt and bound again by the AAD |

One key and one blob per vault (vault id × network): two vaults with distinct vault ids, or two networks, never
share or delete each other's (D7). A data root copied on the same PC keeps the vault id, so both copies share the
enrolment. Either copy can reset it in both: turning Hello on or off in one, or opening a copy without slot B (whose
sweep deletes the shared key and blob), makes the other fall to "reset" (§6.6). This fails safe. Nothing new is
written to the data root.

## 6. Flows

Code shape (D8): `dw-desktop/src/hello.rs` (cfg(windows) WinRT calls behind a small `HelloKeys` trait:
`supported`, `create`, `public_key`, `sign`, `attest`, `delete`); `dw-vault/src/hello_blob.rs` (pure: M, K, seal and
open the blob, store addressing; runs on every OS); dw-app commands that tie them to the vault. All WinRT calls run on
a blocking thread (never the Tauri main thread), with the main window's HWND for the prompt's focus (R3).

**One lock.** dw-app runs every Hello flow of a vault (provider read, enable, use, disable, the sweep, the reset
handling, and the Hello deletion and vault call of recovery and destroy) under one per-vault async mutex, held for the whole flow, prompts included. Without it a provider re-read
during enable (slot B persisted, blob not yet `put`) would see "slot B, no blob" and reset it, and the sweep could
delete the key between enable's create and enrol steps. A second flow waits; the UI shows the first one's prompt.

### 6.1 Provider

`quick_unlock_provider() -> { kind: "windowsHello" | "unavailable", reason? }`, cached per launch and re-read when
the Security page opens. With slot B enrolled, the §6.6 checks run first, so a removed Hello is found as a reset
even when `IsSupportedAsync` is false. Unavailable reasons: `notSetUp` (`IsSupportedAsync` false), `remoteSession`,
`noTpm` (known only after a key exists; see 6.2), `notWindows`. macOS answers `unavailable` until a Tauri-side Touch ID store exists
(U-08, B4).

### 6.2 Enable ("Use Windows Hello")

1. The renderer asks for the passphrase and gets a `ChangeCredential` grant (as Change passphrase does).
2. `enable_quick_unlock(grant_id)` in dw-app:
   1. `RequestCreateAsync(name, ReplaceExisting)` (Hello prompt 1).
   2. `GetAttestationAsync`: only `Success` goes on. `TemporaryFailure` → delete the key, refuse
      `quick_unlock.try_later` (no enrolment). Any other status (`NotSupported`, `UnknownError`) → delete the key,
      refuse `quick_unlock.no_tpm` (D3).
   3. New C; `RequestSignAsync(M)` (Hello prompt 2, if Windows asks again: R5); verify S as PKCS#1 v1.5 SHA-256
      against the public key, else delete the key and refuse `quick_unlock.unsupported`.
   4. K = HKDF(S, C). Only now redeem the grant: `Vault::enroll_quick_unlock(grant)` → W. A grant older than 120 s
      is `vault.grant_invalid` ("took too long, try again"); the key is deleted.
   5. Seal W under K, `put` the blob. If `put` fails: `remove_quick_unlock()` (rollback, needs no grant), delete
      the key, report `vault.os_store_unavailable`.
3. Returns the `QuickUnlockPolicy` (enrolled, limit). The renderer never sees W.

Hello steps come before the grant is redeemed so that a cancelled prompt leaves the vault untouched.

### 6.3 Use (the payment confirm sheet)

Where it applies is D10 (accepted): the payment confirm sheet only. There the credential choice
becomes `windowsHello` when: the vault is encrypted, "Require authentication for every payment" is on, slot B is
enrolled, the quick-unlock value of the whole grant set ≤ the limit, and the passphrase is fresher than 7 days
(`quick_unlock_policy` answers all of these while locked). The value is summed over every grant in the set, as the
vault does (`vault.rs:1273-1279`): the payment, fee included, plus, for "Accept and pay", the accept's `platformOp`
value. Otherwise a 0.49 DASH payment with an accept fee against a 0.5 limit would offer Hello and then fail as over
the limit. Otherwise it is the passphrase, as today.

`authorize` with `CredentialSpec { kind: "windowsHello" }`: dw-app opens the key (no prompt), reads the blob, compares
the public-key hash (the §6.6 checks, in that order), signs M (prompt), derives K, opens W, calls `Vault::authorize_set(…, QuickUnlock(W))`, drops W.
`vault.quick_unlock_limit_exceeded` and `vault.passphrase_stale` turn the sheet to the passphrase with the copy in §8.
A cancelled prompt leaves the sheet open with "Use passphrase instead".

### 6.4 Change the spending limit

As today: passphrase → `ChangeCredential` grant → `set_quick_unlock_spend_limit`. No Hello prompt.

### 6.5 Disable ("Stop using Windows Hello")

`disable_quick_unlock()`: `Vault::remove_quick_unlock()` first (removes the way in; no grant, `vault.rs:2629`), then
delete the blob, then `DeleteAsync(name)`. Host deletions that fail are retried by the sweep (6.7). No passphrase
needed: turning a credential off only removes access.

### 6.6 Hello removed, reset, or the vault copied

Checked on every provider read and every use while slot B is enrolled, under the vault's lock (§6), in this order:
1. A remote session → touch nothing, use the passphrase (`quick_unlock.unavailable_now`).
2. `OpenAsync(name)`, whatever `IsSupportedAsync` says (removing the Hello PIN can make it false and delete the key at
   once). `NotFound` → **reset**. Any other non-`Success` status → `unavailable_now`.
3. The blob: `get` = `Ok(None)` → **reset**. `get` = `Err` (`OsStoreUnavailable`) → `unavailable_now`. A blob that
   doesn't parse (wrong length, unknown version) → **reset**, before any prompt. A public key
   (`RetrievePublicKey(X509SubjectPublicKeyInfo)`, as D4) whose hash differs from the one in the blob's header →
   **reset**.
4. The prompt (use only). `UserCanceled` → `platform.cancelled`. `UserPrefersPassword`, `SecurityDeviceLocked`,
   `UnknownError`, a `NotFound` here or any other status → `unavailable_now`.
5. After a successful prompt: the blob fails to open, or the vault refuses W (`vault.quick_unlock_unavailable`) →
   **reset**.

On **reset**: `remove_quick_unlock()`, delete the blob and the key if present, show §8 "Hello was reset" once, and use
the passphrase. The Security row then shows Off, and the user can enable again (6.2). `unavailable_now` removes
nothing; the passphrase is used this time (D11).

### 6.7 Recovery, destroy, sweep

- `recover_with_mnemonic` and `destroy` already drop slot B (`m2-engine.md:264-265`). The dw-app commands that call
  them, under the vault's lock, delete the Hello key and then the blob **before** the vault call, addressed by the
  current vault id and network (the job `MacBiometricKeyStore.delete` had for Swift). Afterwards would be too late:
  recovery makes a new vault id (`vault.rs:2744`), the sweep only looks at the current id, and Hello keys can't be
  enumerated, so a leftover key would stay a Hello-prompt path to the old data key, which the
  `vault.dwv.replaced-*` copy still holds (`m2-engine.md:381-383`).
  - `DeleteAsync` → `NotFound` counts as deleted. A key delete that fails while the key still exists (`OpenAsync`
    still `Success`) refuses recovery with `quick_unlock.delete_failed` (§8); nothing is recovered. A failed blob
    delete does not refuse: without the key the blob opens nothing.
  - Destroy goes on after a failed delete: it removes `vault.dwv` and every replaced copy, so a leftover key opens
    nothing.
  - If the vault call then fails, the unchanged vault keeps slot B without a key, and the next read resets it (§6.6).
    This fails safe: at worst the user turns Hello on again.
- At session open: slot B absent and a blob or key present → delete them (orphans from a failed delete or a crash).
- Change passphrase leaves slot B as it is (the DEK does not change).

## 7. Where Hello applies

The engine never unlocks the vault with W (`m2-engine.md:261`: "`Vault.unlock` with quick unlock is a host flow").
The lock screen covers the whole app while the vault is `Locked` and asks for the passphrase.

| Option | What Hello does | Cost |
|---|---|---|
| **A (accepted for 1.0)** | Replaces typing the passphrase at the payment confirm sheet (`ConfirmSheet.tsx`: send, pay to a contact, "Accept and pay") up to the limit, while the app is unlocked. Everything else stays passphrase only: the lock screen, PSBT signing in Tools, sign message, the other Platform writes (registration, top-up, profile), masternode, masternode-withdraw and governance grants. | No engine change; one renderer credential kind. |
| B | Also on the lock screen: Hello dismisses the overlay but the vault stays `Locked` (DEK not in memory): balances and history show, spends within the limit prompt Hello again, CoinJoin mixing stays stopped, a reveal or larger spend asks for the passphrase. | A new "UI unlocked, vault locked" state in the shell and the lock reconcile; mixing semantics change. |
| C | Hello fully unlocks the vault. | Puts the DEK in memory on a Hello PIN; breaks the limit model. Not offered. |

D10 (accepted): A for 1.0. B, and Hello on the other sheets, are follow-ups if users ask.

## 8. Copy

| Situation | Code | Text |
|---|---|---|
| Row, available, off | — | Title "Windows Hello". Sub: "Approve payments up to your limit with your face, fingerprint or Windows Hello PIN instead of your passphrase." Button "Turn on…" |
| Row, on | — | "On. Payments up to {limit} are approved with Windows Hello. Larger payments, showing your recovery phrase and changing security settings still need your passphrase." Limit select (0, 0.1, 0.5, 1, 5 DASH). Button "Turn off" |
| Enable dialog (before the passphrase) | — | "Anyone who can use this Windows account and knows its Windows Hello PIN will be able to approve payments up to your limit. Your passphrase is still needed every 7 days." (D9) "Only approve a Windows Hello prompt right after you confirm in DashWallet." (R1) |
| Confirm sheet, Hello button | — | "Confirm with Windows Hello", with the hint "Only approve a Windows Hello prompt right after you confirm in DashWallet." (R1) |
| Not set up | `quick_unlock.not_set_up` | "Set up Windows Hello in Windows Settings ▸ Accounts ▸ Sign-in options first." |
| Attestation not available now | `quick_unlock.try_later` | "Windows couldn't confirm that this PC keeps Windows Hello keys in its security chip (TPM) right now. Try again later." |
| No TPM | `quick_unlock.no_tpm` | "This PC keeps its Windows Hello keys in software, not in a security chip (TPM), so DashWallet can't use them to protect your wallet." |
| Unsupported key | `quick_unlock.unsupported` | "Windows Hello on this PC can't be used for quick unlock." |
| Remote session / temporarily unavailable | `quick_unlock.unavailable_now` | "Windows Hello isn't available right now. Enter your passphrase." |
| Cancelled | `platform.cancelled` | (no message; the passphrase field shows) |
| Hello was reset | `quick_unlock.reset` | "Windows Hello was turned off or reset on this PC, so it can no longer approve payments. Enter your passphrase. You can turn Windows Hello on again in Settings ▸ Security." |
| Over the limit | `vault.quick_unlock_limit_exceeded` | "This payment is above your Windows Hello limit of {limit}. Enter your passphrase." |
| Passphrase stale | `vault.passphrase_stale` | "For your security, enter your passphrase. It has been more than 7 days." |
| Took too long | `vault.grant_invalid` | "That took too long. Try again." |
| Key delete failed (recovery) | `quick_unlock.delete_failed` | "Windows couldn't remove DashWallet's Windows Hello key, so your wallet was not recovered. Try again in a moment." |
| Store failed | `vault.os_store_unavailable` | "Windows Credential Manager couldn't save the key. Windows Hello was not turned on." |

New codes are dw-app's (`quick_unlock.*`), listed with the other dw-app codes and in the generated TS types.

## 9. Tests

**Fakes.** `FakeHelloKeys` (the `HelloKeys` trait): a per-name secret; `sign` = HMAC-SHA256(secret, M) (deterministic
like PKCS#1 v1.5); modes: not set up, remote session, cancel, `UserPrefersPassword`, `UnknownError`, `NotFound`
(on open and on sign), key replaced (new secret, new public key), attestation `NotSupported`, `UnknownError` or
`TemporaryFailure`, non-deterministic signer, a pause inside a step (for interleaving), counts prompts. With
`MemoryOsStore` (and `unavailable()`).
`hello_blob` needs no fake.

| Test (dw-vault `hello_blob`) | Checks |
|---|---|
| `test_IOS_011_hello_blob_round_trip_and_aad_binding` | seal/open; a different vault id, network, key name or public key fails |
| `test_IOS_011_hello_message_and_aad_are_length_framed` | moving a byte between the network and the next field changes M and the AAD |
| `test_IOS_011_hello_blob_rejects_a_changed_challenge_version_or_key_hash` | an edited header field fails to open |
| `test_IOS_011_hello_kek_is_hkdf_of_the_signature` | fixed vector |

| Test (dw-app, fake Hello + `MemoryOsStore`) | Checks |
|---|---|
| `test_IOS_011_enable_then_spend_within_the_limit_with_hello` | one W per enable; `authorize` issues `Spend` ≤ limit |
| `test_IOS_011_enable_cancelled_leaves_the_vault_untouched` | no slot B, no blob, no key; grant not redeemed |
| `test_IOS_011_enable_refuses_a_software_key` | attestation `NotSupported` and `UnknownError` → `no_tpm`; `TemporaryFailure` → `try_later`; key deleted, no slot B in each |
| `test_IOS_011_enable_refuses_a_non_pkcs1_signature` | key deleted, no slot B |
| `test_IOS_011_enable_rolls_back_when_the_store_fails` | slot B removed, key deleted |
| `test_IOS_011_enable_with_an_expired_grant_deletes_the_key` | |
| `test_IOS_011_disable_removes_slot_b_blob_and_key` | and is idempotent |
| `test_IOS_011_hello_reset_removes_slot_b_and_says_so` | `NotFound` on open (also with "not set up"), replaced key, blob `Ok(None)`, a blob of the wrong length or an unknown version, blob that fails to open, vault refuses W; no prompt for the first five |
| `test_IOS_011_unavailable_now_keeps_the_enrolment` | remote session, `MemoryOsStore::unavailable()`, `UnknownError`, `UserPrefersPassword`, `SecurityDeviceLocked`: slot B, blob and key kept |
| `test_IOS_011_hello_flows_are_serialised` | a provider read and a sweep started while enable is paused between enrol and `put` wait, then see slot B with its blob; enable ends On |
| `test_IOS_016_over_the_limit_or_stale_asks_for_the_passphrase` | |
| `test_IOS_011_recovery_and_destroy_drop_the_blob_and_key` | deleted at the addresses of the vault id, before the vault call (recovery makes a new id), under the lock |
| `test_IOS_011_recovery_refuses_when_the_key_delete_fails` | key delete fails, key still there → `quick_unlock.delete_failed`, vault unchanged; `NotFound` on delete or a failing blob delete → recovery goes on; destroy goes on in every case |
| `test_IOS_011_sweep_removes_orphans` | |
| `test_IOS_011_two_vaults_have_separate_keys_and_blobs` | distinct vault ids × networks |
| `test_IOS_011_wrap_key_never_leaves_dw_app` | no command response or event serialises W (scan the JSON of every quick-unlock command) |

Renderer: vitest for the Security row states and `credentialPrompt` with the new kind; Playwright fixture scenarios
(`windowsHello` provider on, off, unavailable, reset) for the row and the confirm sheet, Chromium and WebKit; UI
artifacts per DEC-1484.

Windows CI (`windows-latest`, no Hello): `hello.rs` builds; `supported()` answers false and the provider answers
`unavailable/notSetUp` without panicking or prompting.

**Real-PC checklist** (one Windows 11 PC with a TPM 2.0 and a fingerprint or face sensor, and a machine whose Hello
keys are in software, for example a Hyper-V VM without a vTPM; installed NSIS build):
1. Provider reads `windowsHello`; the row offers "Turn on…".
2. Enable: count the prompts (R5); the prompt is in front of the app and takes face/fingerprint (R3).
3. Attestation status on the TPM PC (expect `Success`) and on the software-key machine (expect `NotSupported`), and,
   on both, whether the key's NCrypt `NCRYPT_IMPL_TYPE_PROPERTY` has `NCRYPT_IMPL_HARDWARE_FLAG` (R2). The
   software-key half is required, not optional: without a flag seen clear there, the fallback can't be used.
4. Send 0.1 DASH (testnet) with Hello; a send above the limit asks for the passphrase.
5. Cancel the prompt: the passphrase field shows; nothing changes.
6. Use the PIN instead of the biometric: works (D9).
7. Set the clock forward 8 days: the passphrase is asked.
8. Turn off in the app: the Credential Manager entry and the key are gone (check with `cmdkey /list` and the key list of the "Microsoft Passport Key Storage Provider"; the exact command is found during the session).
9. Remove the Windows Hello PIN in Windows Settings, then pay: the reset message, slot B gone, passphrase works; set Hello up again and re-enable.
10. Add a fingerprint: quick unlock keeps working (differs from macOS `.biometryCurrentSet`; recorded, not a bug).
11. Over Remote Desktop: passphrase, enrolment kept (R4).
12. Two data roots created separately (distinct vault ids): each has its own key; turning one off leaves the other
    working.
13. Close the app during the prompt; reopen: no orphan (sweep).
14. From a second test exe, open our key name and call `RequestSignAsync`: record what the prompt shows (title, app
    or exe name, any text) and whether it differs from ours (R1).
15. Recover with the recovery phrase while enrolled: the old key and blob are gone before the new vault is written.

## 10. Decisions

| # | Decision | Why |
|---|---|---|
| D1 | The vault and its contract do not change. Hello protects the vault-made W in a host blob, the macOS keychain item's role. | `enroll_quick_unlock` already returns W once (`vault.rs:2586`); no key-handling change in dw-vault's slot logic. |
| D2 | W stays in Rust: new dw-app commands `quick_unlock_provider`, `enable_quick_unlock(grant_id)`, `disable_quick_unlock`, and a credential kind `windowsHello` resolved inside dw-app. The renderer's `quickUnlock{wrapKey}` path and `enroll_quick_unlock`'s returned key are not used on Windows. | The webview is the larger attack surface; W opens the DEK. |
| D3 | Enrolment needs a TPM-backed Hello key: attestation `Success` only; `TemporaryFailure` is "try later"; anything else is `no_tpm`. If attestation proves unreliable, a per-key hardware property replaces it, only after it has been seen clear on a software Hello key (R2); if neither works, Windows ships without Hello. D3 is never dropped. **Accepted (MG 2026-11-19).** | A software Hello key falls to an offline PIN brute force (§3), weaker than the passphrase it stands in for. |
| D4 | Verify each enrolment signature as RSA PKCS#1 v1.5 SHA-256 against the key's public key (`RetrievePublicKey(X509SubjectPublicKeyInfo)`, the same encoding the blob hashes), with the `IBuffer` the sign call returned; refuse anything else. | Proves determinism without a second prompt. |
| D5 | Random 32-byte challenge per enrolment, stored in the blob; K = HKDF-SHA256 with a domain-separated message. | Re-enrolment invalidates the old blob; no cross-protocol reuse of the signature. |
| D6 | The blob lives in Credential Manager (D-N8), not in the data root. | Not in backups or synced folders; DPAPI adds a layer. |
| D7 | One Hello key and one blob per vault (vault id × network). | Two vaults with distinct vault ids, or two networks, never touch each other's enrolment. A data root copied on the same PC shares the vault id and the enrolment; turning Hello on or off, or opening a copy without slot B, can reset the enrolment in both (fails safe, §5). |
| D8 | WinRT code in `dw-desktop/src/hello.rs` via the `windows` crate (0.62, already in the lockfile) behind a `HelloKeys` trait; the pure blob code in `dw-vault/src/hello_blob.rs` (it uses the vault's AEAD; `hkdf` 0.12 becomes a direct dep, already in the lockfile). | Testable on Linux with fakes; the crypto stays in the crate that already holds it. |
| D9 | Accept Hello's PIN fallback and say so in the enable dialog. **Accepted (MG 2026-11-19).** | `KeyCredentialManager` can't require biometrics; the limit and 7-day rule bound UI use. |
| D10 | Scope A: Hello at the payment confirm sheet only (send, pay to a contact, "Accept and pay"). The lock screen, PSBT signing, sign message, other Platform writes, and the masternode, masternode-withdraw and governance grants stay passphrase only in 1.0, although the vault would issue them under quick unlock. **Accepted (MG 2026-11-19).** | No engine or shell lock-state change in 1.0 (§7); one sheet to prove on the real PC. |
| D11 | Reset (removes the enrolment) only on: `OpenAsync` `NotFound`, whatever `IsSupportedAsync` says; a public key that differs from the blob's; the blob `get` returning `Ok(None)`; a blob that doesn't parse (wrong length, unknown version), before any prompt; a blob that fails to open after a successful prompt; the vault refusing W. `unavailable_now` (removes nothing) on: a remote session; any other `OpenAsync` status; the blob `get` returning `Err`; `UserPrefersPassword`, `SecurityDeviceLocked`, `UnknownError` or any other sign status. `UserCanceled` is a cancel. Order as §6.6. | Remote sessions, a locked TPM and a store hiccup are temporary; removing the Hello PIN can make `IsSupportedAsync` false while the key is gone. |
| D12 | Spending limit choices and default as today (0/0.1/0.5/1/5 DASH, 0.5). | `m2-engine.md:263`, IOS-016. |
| D13 | Implementation ~500 non-test lines in two PRs, each carrying the doc edits for the behaviour it changes: (1) `hello_blob` + `hello.rs` + dw-app commands, with the regenerated Tauri bindings (`apps/desktop/src/api/generated/`, checked by ci.yml:166-173), `m2-engine.md:283-284` and `:387-388`, and DESIGN-opus.md:258-260 (slot B is now a random W sealed under K, not a key derived directly from the signature); (2) the Security row and confirm sheet, with parity IOS-011/016 and U-04 D11. | Lane N's estimate; keeps each PR under the size check; no doc is wrong while a PR is in. |

## 11. Open risks

| # | Risk | Plan |
|---|---|---|
| R1 | Same-user malware can call our key and ride a real Hello prompt the user approves (Bitwarden documents the same residual). One approved prompt gives it W, and with `vault.dwv` the data key: every seed, outside the spending limit and the 7-day rule, permanently (the data key does not rotate). The prompt can't name the action or the caller. | **Accepted for 1.0 with disclosure (MG 2026-11-19)**: §3 row 1, the enable-dialog and confirm-sheet line (§8), real-PC step 14; documented for H-01. Packaging as MSIX would scope keys per package; not in 1.0. |
| R2 | `GetAttestationAsync` may not return `Success` for a TPM key (it may need a network round trip, or not be offered for Hello keys), or may not separate TPM from software keys. | Real-PC step 3. If attestation fails, use the per-key hardware property instead (`NCRYPT_IMPL_TYPE_PROPERTY` with `NCRYPT_IMPL_HARDWARE_FLAG` on the key, §2), but only once the session has seen it set on a TPM Hello key and clear on a software Hello key (a Hyper-V VM without a vTPM is enough); TPM presence on the PC is not enough. Otherwise Windows 1.0 ships passphrase only. D3 is not dropped. |
| R3 | From an unpackaged Win32 app the Hello prompt can open behind the window, and face/fingerprint then don't start; KeePassXC and Bitwarden both bring "Credential Dialog Xaml Host" to the front by hand. | Same workaround (FindWindow + SetForegroundWindow, retried briefly); real-PC step 2. |
| R4 | Behaviour over RDP and with `SecurityDeviceLocked` is not documented precisely. | D11; real-PC step 11. |
| R5 | Enable may show two prompts (create, then sign). | Acceptable; record the count. |
| R6 | No CI can exercise Hello (`windows-latest` has none); the T-04 VM (`ROADMAP.md:97`) has no TPM-backed Hello either. | Fakes plus the real-PC checklist; if the 11-30 session slips, Windows 1.0 ships passphrase only (`PLAN-1.0.md:64`). |
