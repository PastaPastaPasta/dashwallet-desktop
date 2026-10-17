# DP4-02: the avatar pipeline

ROADMAP row DP4-02 (L, engine, T0 + T2): "Avatar pipeline (DASHPAY §3.8): fetch guard, verification, re-encoding,
cache, privacy toggle; URL, Gravatar and file sources; Imgur only with B1". Acceptance: fixtures for an oversize
image, a wrong hash, a redirect to 10.0.0.1 and a GIF; a mismatch shows initials; no fetch reaches a private IP; hash
and dHash equal the library's; thumbnails are written through descriptors, not by path.

The calls are m4 §2.7's `prepare_avatar`, `avatar_upload_available`, `upload_avatar` and `avatar`, which were stubs,
plus `update_profile`'s `AvatarChange::Set` (DP4-01 D6, F2) and two new `NetworkSession` calls for the privacy toggle
(D17). The UI is DP4-03's (Edit Profile, avatars in every list); this task only wires My Profile's header (DP1-10 D4)
to `dashpay_avatar`.

| Part | Where |
|---|---|
| The facade bodies and the toggle calls | `dw-engine/src/platform/profile.rs` |
| The session's avatar state: candidates, the per-URL fetch lock, the fetch limit, the toggle, the read path (profile → cache → fetch → verify) | `dw-engine/src/platform/avatar.rs` |
| The fetch guard: URL rules, address policy, the checking resolver, redirects, size and time caps | `dw-engine/src/platform/avatar_fetch.rs` |
| Sniffing, the bounded decoder, thumbnails, the library's hash and dHash, the verification rule | `dw-engine/src/platform/avatar_image.rs` |
| The thumbnail store (directory descriptor, `openat`, `renameat`, `unlinkat`) and the `dp_avatar` index with rotation | `dw-engine/src/platform/avatar_cache.rs`; `dw-appdb/src/avatar.rs` |
| `AvatarChange::Set` in the profile write | `dw-engine/src/platform/profiles.rs` |
| Tests: a local HTTP server and a scripted resolver; fixtures; the decoder seed corpus | `dw-engine/src/platform/avatar_tests.rs` (fixtures built in code), `dw-engine/tests/fixtures/avatar-corpus/` (seeds) |
| dw-app: the toggle's two commands; My Profile's header picture | `dw-app/src/api/social.rs`; `apps/desktop/src/screens/contacts/Avatar.tsx`, `screens/dashpay/MyProfile.tsx`, `dashpay/profileQueries.ts` |
| dwcli | `profile pictures [--set on\|off]`, `profile prepare-avatar --url\|--file\|--gravatar`; `profile avatar` and `profile set --avatar-url` already existed |
| Contract | m4 §0, §1, §2.7 (the four calls, `update_profile`'s `Set`, the two toggle rows), §7 item 55, Contract-Version 29 |

## The read: `avatar(identity, size)`

1. **The profile.** `profile(identity)` (DP4-01: an own identity, else a contact's profile the contacts model holds).
   An identity neither knows (a search hit, an inviter) is read from Platform (`profile_document`, proved; D14) and
   kept in memory for 10 minutes; reads of one identity share one Platform read, at most 4 at once (D35). No profile,
   or one without an avatar, is `Ok(None)`.
2. **The toggle** (D17). Off: `Ok(None)` for every identity that is not one of the wallet's own, before any read
   (no Platform query, no fetch); an own identity is served from the cache only, never fetched. The read keeps what
   it saw when it began (a ticket) and asks again at every network step and before answering: a turn-off that has
   returned stops a Platform read or a fetch not yet started, abandons one running, and makes another user's read
   answer `Ok(None)` whatever it found (D33). A profile
   whose `avatarHash` is not 64 hex digits or whose `avatarFingerprint` is not 16 is a mismatch without a fetch
   (D26).
3. **The cache index** (`dp_avatar`, keyed by the SHA-256 of the URL as published). Under the URL's fetch lock, so
   one fetch serves every caller asking for the same URL at once (D12):
   - a row `ok` whose content is the profile's `avatarHash`: the thumbnail is read through the directory descriptor,
     re-decoded and its size checked (D10). Done (the fingerprint is not compared, D32);
   - a row `ok` with other content, or a failure row, younger than 10 minutes: no fetch (D11). Other content is
     `avatar.hash_mismatch`, a failure is its code;
   - otherwise (and with the toggle on, or for a `prepare_avatar`) the URL is fetched again.
4. **Fetch** (the guard below), **decode** and **thumbnail**, then the row is written, `ok` with `content_sha`, `file`
   and `bytes` (and `dhash` only for a candidate's fetch: a read skips the library's second decode, D32), or the
   failure's status; then step 3's verdict on the fresh row.
5. **Verdict.** The SHA-256 of the fetched bytes must equal `avatarHash` exactly (D6). The fingerprint is not compared
   (D32); it only has to be well formed (D26). A mismatch is `avatar.hash_mismatch`; the UI shows the initials for that
   and for every other error (D7).

## The fetch guard (`avatar_fetch.rs`)

- **URL.** At most 2048 bytes (the contract's and the library's limit); `https` only; no user name or password; the
  default port only (443, D3). Android's crop parameter `dashpay-profile-pic-zoom=l,t,r,b` is taken out of the URL
  that is fetched and applied to the picture (D15).
- **Addresses.** One resolution per hop, by the engine: an IP-literal host is checked as it is; a name is resolved
  (the system resolver, on a blocking thread) and **every** address must be public, else the hop is refused (D4). The
  HTTP client is given only the checked addresses (reqwest's `dns_resolver`), so it connects to exactly what was
  checked and never resolves the name again: a rebinding DNS server cannot move a connection to a private address.
  Not public: `0.0.0.0/8`, `10/8`, `100.64/10` (CGNAT), `127/8`, `169.254/16` (link-local and cloud metadata),
  `172.16/12`, `192.0.0/24`, `192.0.2/24`, `192.88.99/24`, `192.168/16`, `198.18/15`, `198.51.100/24`,
  `203.0.113/24`, `224/4`, `240/4`; in IPv6 everything outside `2000::/3`, and inside it `2001::/23` (Teredo,
  benchmarking, ORCHID), `2001:db8::/32`, `2002::/16` (6to4), plus `::ffff:0:0/96`, `64:ff9b::/96` (NAT64) and
  `::/96`, whose embedded IPv4 address is checked instead (`64:ff9b:1::/48` is refused). ULA `fc00::/7`
  (including `fd00:ec2::254`), link-local `fe80::/10`, site-local `fec0::/10` and multicast `ff00::/8` fall outside
  `2000::/3`.
- **Redirects** are not followed by the client: up to 3 are followed by the engine, each `Location` resolved against
  the hop's URL and put through the same URL and address rules. A refused hop fails the fetch before any connection
  to it.
- **Caps.** The whole fetch, every hop, connection and body included, has 10 seconds; a connection 5. The body is
  read in chunks and dropped at 5 MiB (a `Content-Length` above it is refused before reading): `avatar.too_large`.
  Only status 200 counts.
- **What is sent.** No cookies, no `Referer`, a fixed `User-Agent` and `Accept: image/png, image/jpeg, image/gif`. No
  proxy: the client ignores the environment's proxy variables (D5); the app has no proxy setting yet.
- At most 4 fetches run at once in a session.

## Decoding and re-encoding (`avatar_image.rs`)

- **Sniffing.** The format comes from the first bytes, never from `Content-Type` or the URL: PNG, JPEG or GIF (D8).
  Anything else is `avatar.unsupported`.
- **Decoding** uses the `image` crate's pure-Rust decoders with strict limits: at most 4096 × 4096 pixels, 128 MiB
  of decoder allocations and 64 MiB of decoded picture (D24); past them `avatar.too_large`. A GIF gives its first frame only (D9). JPEG's EXIF orientation
  is applied. It runs on a blocking thread, at most 2 at once, and a panic in a decoder is caught and is
  `avatar.unsupported`.
- **Hash and fingerprint** of the original bytes come from platform-wallet's own `calculate_avatar_hash` and
  `calculate_dhash_fingerprint` (D6), called only after the bounded decode accepted the bytes and the thumbnails are
  made (the full-size picture is dropped first): the library decodes the bytes again without limits of its own, so
  this order is what bounds it.
- **Thumbnails.** The picture (after the crop, if any) is cut to a centred square and resized (Lanczos3) to 128 and
  256 px, then encoded as 8-bit RGBA PNG. Only these re-encoded PNGs are stored or cross the facade.

## The cache (`avatar_cache.rs`, `dp_avatar`)

- **Directory.** `avatars_dir()` was checked at session open (dw-fs: created 0700, or a user's symlink followed).
  The engine opens it once, `O_DIRECTORY`, and keeps the descriptor; from then on it never uses the path. It refuses a
  directory it does not own or that group or others may write (D10): thumbnails are then kept in memory only, the last
  64 (D23), so a read past those fetches again (failures and mismatches are still remembered in the index).
- **Files** are named `<stem>-128.png` and `<stem>-256.png`, the stem the SHA-256 of the content's SHA-256 and the
  crop (D15). A file is written to a temporary name with `openat(O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600)`,
  synced, and renamed into place with `renameat`; read with `openat(O_NOFOLLOW)` and `fstat` (a regular file, at most
  1 MiB); unlinked with `unlinkat`. Windows has no `openat`: there the files are opened by path below the directory,
  `create_new` and no reparse points followed (D16).
- **Index.** `dp_avatar` (DASHPAY §3.4): one row per URL; its `status` is only the URL's fetch and decode outcome
  (`ok`, `fetch_failed`, `too_large`, `unsupported`), never a verdict on a profile, which is made at each read.
- **Rotation.** After a write, while the distinct files' bytes exceed 200 MB, or the rows 10 000, the oldest rows go
  first; a file is unlinked only when no remaining row names it.

## Sources: `prepare_avatar(src)`

| Source | What happens | Candidate |
|---|---|---|
| `Url{url}` | trimmed and parsed; what is published is the parsed form, at most 2048 bytes (D25); the URL rules (a refused URL is `invalid_argument`); fetched now whatever the toggle (D17); decoded; cached | `url` set, `needs_upload: false`, hash and fingerprint of the bytes fetched |
| `Gravatar{email}` | trimmed and lower-cased (D13); a shape check (`invalid_argument`); MD5 in Rust; `https://www.gravatar.com/avatar/<md5>?s=200&d=404&rating=g` (Android's URL) fetched as `Url`. The address is never stored or logged; its copies are zeroed | as `Url` |
| `File{bytes, crop}` | at most 5 MiB (`avatar.too_large`); decoded as fetched bytes are; `crop` (pixels, inside the picture, else `invalid_argument`) applied; re-encoded at most 512 px as the image to upload (no EXIF or other metadata survives) | `url: None`, `needs_upload: true`, hash and fingerprint of the PNG to upload |

The preview is the 256 px thumbnail. Candidates live in the session (m4 §0) for 30 minutes, at most 16 per wallet,
the wallet's oldest dropped first (D27); ids are `av-` and 16 random hex digits. A candidate belongs to the wallet whose `DashPay` made it.

`avatar_upload_available()` is `false` and `upload_avatar(candidate)` is `avatar.upload_unconfigured` until DCG's
Imgur credentials arrive (B1, D2); an unknown candidate is `invalid_argument`, one that already has a URL
`invalid_argument`.

`update_profile` with `Set{candidate}`: the candidate must exist for this wallet and have a URL (`invalid_argument`
otherwise), before the identity's lock is taken (DP4-01 D8); the write sets `avatarUrl`, `avatarHash` (32 bytes) and
`avatarFingerprint` (8 bytes) together.

## Decisions

| # | Decision | Why |
|---|---|---|
| D1 | The pipeline is four non-facade files (`avatar.rs`, `avatar_fetch.rs`, `avatar_image.rs`, `avatar_cache.rs`) plus a dw-appdb module, each well under 800 lines; `profile.rs` keeps only the facade bodies. | DASHPAY §3.1's file budget, and the shared files stay small for the siblings in flight. |
| D2 | Imgur is not implemented: `avatar_upload_available` is `false`, `upload_avatar` is `avatar.upload_unconfigured`, and a `File` candidate cannot be published. | B1: DCG has not supplied credentials (DASHPAY R9, DEC-15). An upload client written against an API nobody here can call would be untested code. Follow-up F1 adds it with the credentials, and fetches the uploaded URL back through the guard to take the hash of what Imgur serves (it may re-encode). |
| D3 | Only port 443 is fetched. | Mobile wallets allow any port; refusing others costs a picture on a non-standard port (shown as initials) and removes scanning of public hosts' other services through other users' profiles. |
| D4 | A name with any non-public address is refused as a whole, rather than its public addresses used. | An attacker who mixes addresses gets nothing; a legitimate host never publishes private records next to public ones. |
| D5 | No proxy: reqwest's system-proxy support is switched off (`no_proxy`). | A proxy resolves names itself, so the engine could not check where a connection goes; and the environment's `HTTPS_PROXY` must not route around the guard. The app has no proxy setting (U-04 D7). When one arrives (follow-up F2), avatars over a proxy must use it, and the address check then has to be the proxy's (or avatars off). |
| D6 | Hash and dHash are platform-wallet's own functions, so they equal the library's by construction (a test asserts it on every fixture and every corpus seed). The read's verdict is the SHA-256 alone, matched exactly; the fingerprint is published (the library's dHash) but not compared on reading (D32). | The SHA-256 binds the bytes: a host cannot serve other bytes under it. The fingerprint is the publisher's claim as much as the hash, so with an exact hash it adds nothing against a hostile host, and a fingerprint check could only refuse pictures that are exactly what the profile names. Accepting a *near* fingerprint with another hash (DIP-15's recompressed-image case) is not done: the acceptance is "wrong hash shows initials". |
| D7 | A mismatch is `avatar.hash_mismatch`, not `Ok(None)`; `None` is only "no avatar" or the toggle. Hosts show the initials for every error. | The code is in the contract since E0-08, and a host (or dwcli, or T2) can tell "no picture" from "a picture that failed its check". |
| D8 | PNG, JPEG and GIF only; WebP and the rest are `avatar.unsupported`. The format is sniffed. | Those are what the mobile wallets publish (Gravatar, Imgur, camera and gallery JPEGs) and what the library's dHash decodes (its `image` features). WebP would add a decoder for no profile seen (follow-up F3). A `Content-Type` is the server's claim. |
| D9 | A GIF, animated or not, is accepted and its first frame used. | The thumbnail is a still PNG anyway, the library's dHash (`load_from_memory`) hashes the first frame, and decoding only the first frame bounds an animation of any length. Refusing animated GIFs would show initials for pictures Android shows. |
| D10 | Thumbnails are trusted only as files the engine wrote: the directory must be the user's and not group- or other-writable (a symlink target keeps its own mode, but one others may write turns the disk cache off), and a thumbnail read back is re-decoded and its size checked before it is returned. | §3.8's descriptor rule stops symlink and path races; the owner check stops another local user planting files; the re-decode means a truncated or replaced file reaches the UI only as a valid PNG of the right size. The same user is out of scope. |
| D11 | A failed URL, and a URL whose content does not match the profile asking, is fetched again at most every 10 minutes. | A mismatch or a dead host must not cost a fetch per list row per render; 10 minutes lets an avatar changed on its host and then on Platform show soon. |
| D12 | One fetch per URL at a time (a lock per URL); callers waiting on it read the row it wrote. | Contact lists ask for the same avatar from several rows at once. |
| D13 | The Gravatar address is trimmed and lower-cased before its MD5, as Gravatar specifies; Android hashes it as typed. The URL is Android's (`s=200&d=404&rating=g`). | "Foo@Example.com" and "foo@example.com" are one Gravatar account; Android's form would show a 404 for the first. The URL form matches Android's, so a profile looks the same. |
| D14 | `avatar` reads the profile of an identity the wallet does not hold from Platform (proved), cached in memory for 10 minutes, 256 entries. | m4 §2.7: "any identity (a contact, a search hit, the inviter, an own identity)"; search hits and inviters are not in the contacts model. |
| D15 | Android's `dashpay-profile-pic-zoom=l,t,r,b` (fractions of the picture) is removed from the URL before fetching and applied as a crop; a malformed value is ignored. The thumbnail's file stem includes the crop, so two URLs of one picture with different crops do not share a file. | Android publishes it (`ProfilePictureHelper.setPicZoomParameter`) and hashes the picture at the URL without it; ignoring it would fetch a URL Android never fetches. |
| D16 | Windows: no `openat`; thumbnails are created with `create_new` below the directory path and read by path. | The descriptor rule is the Unix one (§3.8); Windows' per-user profile ACLs protect the directory, and the reparse-point equivalent of the race is follow-up F4 with W-01. |
| D17 | "Load contact pictures" is the engine's setting, `dashpay.load_contact_pictures` in app.sqlite (global scope, so per network, like `trust.trusted_only`), on unless `"0"`. Two `NetworkSession` calls read it (sync, in memory) and set it. Off: no avatar of anyone else is fetched or shown (`Ok(None)`), and an own identity's is shown from the cache only. `prepare_avatar` fetches anyway: the user asked for that URL. | §3.8: on by default, can be turned off, no network fetch when off. The engine must know it, since it is the one that fetches. The Settings row is DP4-03's (follow-up F5). |
| D18 | A `File` candidate's image to upload is the cropped picture re-encoded as PNG, at most 512 px, never the original bytes. | Photos carry EXIF (location, device); the upload is public. 512 px is above the largest thumbnail with margin for Hi-DPI. |
| D19 | Registration's `initial_profile.avatar_candidate` stays `invalid_argument`. | The registration's profile step publishes through the library's `create_profile`, which takes avatar bytes, not a hash, and DP1-02 owns that step; with uploads off only `Url` and `Gravatar` candidates would qualify. Follow-up F6. |
| D20 | Tests reach the pipeline through a test-only policy: plain `http` to the test server, and `127.0.0.1` counted as public. Nothing else changes: the resolver seam, the redirect loop and the address check are the production ones, and `127.0.0.2` (a second listener that counts connections) stays private. | A local HTTPS server with a test root would need a TLS stack in dev-dependencies; the HTTPS path itself is T2's. |
| D21 | No new error codes. A URL refused by its form (scheme, port, user info, length) at `prepare_avatar` is `invalid_argument`; a refused address, a timeout, a status other than 200 or too many redirects is `avatar.fetch_failed`. | The contract's five `avatar.*` codes cover what a host shows; the detail names the rule for logs. |
| D22 | If the toggle cannot be read at session open, it is off for the session (logged). | A user who turned pictures off must not see fetches because app.sqlite failed once; the next open reads it again. |
| D23 | Without a usable cache directory (not created, or refused by D10) the session keeps the latest 64 thumbnail pairs in memory. | Otherwise every read of a verified picture would fetch it again, a request per DashPay pass that the host could track. The index still records failures and mismatches. |
| D24 | A picture decoded is at most 64 MiB (4096 × 4096 8-bit RGBA; a 16-bit one that large is refused), checked before `from_decoder`, which does not hold its output to `max_alloc`. The thumbnails are made, and the picture dropped, before the library's own decode of the same bytes. No decode deadline: a decode cannot be cancelled on a blocking thread, and two run at once at most. | A small, highly compressible 16-bit PNG would otherwise take 128 MiB per decode plus copies; this bounds one ingest to about two pictures' worth. A deadline is follow-up F7. |
| D25 | `prepare_avatar(Url)` publishes the URL as parsed (`Url::to_string`: scheme and host lower-cased, whitespace removed), at most 2048 bytes; Android's zoom parameter is removed from the fetched URL segment by segment, the rest of the query kept as written. | What is published is then what the guard checked and what the library's `https://` check reads; re-encoding the rest of a query (`%20` to `+`) could change what a host serves. |
| D26 | A profile whose hash or fingerprint could match nothing (missing, or not 64 / 16 hex digits) is `avatar.hash_mismatch` without a fetch. | DIP-15 publishes the two together and the library writes both; a profile breaking that is malformed, and a fetch would only tell the host the wallet looked. The hash alone decides a well-formed one (D32). |
| D27 | Candidates are capped per wallet (16), not per session. | One wallet's prepares must not evict another's candidate before its edit is published. |
| D28 | The address policy cannot see three cases: a hairpin NAT to the user's own public address, LAN hosts with global IPv6 addresses, and a provider's NAT64 prefix other than `64:ff9b::/96`. They are accepted residuals. | Each is a public address by every rule the engine can apply; the fetch is still `https` on 443, capped, and its bytes only shown if they match the profile. |
| D29 | Swift T0 is run (m4's contract changed: version 29, no §4 code). | dw-common's rule; Swift's `engineCodes` mirrors m1 §4 only (DP4-01 D5), so no Swift source changes. |
| D30 | The renderer shows the picture in My Profile's header only, from `dashpayAvatar(wallet, main identity, large)`, falling back to the initials on `null`, any error, or an image that fails to load; the fixture has a `dashpay-avatar` scenario. | The row's UI part is "wire the existing placeholder to the engine read"; lists and Edit Profile are DP4-03's. |
| D31 | dwcli's `profile prepare-avatar --gravatar EMAIL` takes the address on the command line. | dwcli is a developer and test tool; the address is not a credential (the session grammar types it as text). The app never logs or stores it. |
| D32 | The fingerprint does not veto an exact hash (revises the first D6, which required the library's dHash within 20 bits of `avatarFingerprint`). | T2 (below): of the 53 testnet avatars whose bytes match `avatarHash` exactly, 9 have the library's dHash bit for bit (published with platform-wallet) and the other 44 differ by 12 to 35 bits (median 21; 28 of 53 above 20; no byte or bit order explains it). The mobile wallets' dHash is another algorithm, and 32 bits is what two unrelated pictures differ by on average, so no threshold both accepts their pictures and means anything. A 20-bit rule would have shown initials for over half of the real profiles that verify. |
| D33 | "Load contact pictures" off holds from the moment `set_load_contact_pictures(false)` returns, for reads already running too (revises D17's "read again once the URL's lock is held"). The toggle carries an epoch moved by every transition; a read takes a ticket (on, epoch) as it begins and may use the network only while the toggle is on at that epoch: checked before the Platform read and before the fetch, both run against a watch that drops them when the ticket is revoked (nothing is recorded for an abandoned fetch), and checked again before another user's read answers (`Ok(None)` then, a cached hit included). An own identity's cached thumbnail still shows. | Sol r1 M1: a read that saw "on", paused at the cache lookup, then fetched, or returned a hit or a fetch that was running, after the user turned pictures off. A turn-on does not revive a read that began off (its ticket was never "on"); one that began on and saw an off-and-on is dropped too, as the epoch moved: the caller asks again. A decode already running is not stopped (F7); its result is discarded. |
| D34 | Transitions of the toggle are serialized per session: a lock held across the app.sqlite write and the memory store, so memory is always the last value written. A failed write leaves memory as it was. | Sol r1 M2: two calls at once could store `on` in memory after a durable `off`. |
| D35 | Reads of one identity the wallet does not hold share one Platform read (a lock per identity; the waiters take the kept profile), and at most 4 such reads run at once. Failures are not kept, so the next waiter reads again. | Sol r1 / F8: lists (DP4-03) ask for the same search hit's avatar from several rows; without it each asked Platform. |

## T2 evidence (testnet, 2026-10-17, read only)

- A scratch tool (not committed) paged testnet's DashPay `profile` documents through the SDK (proved, `$ownerId` order):
  201 profiles, 59 with an avatar, every one with hash and fingerprint. Each URL was fetched once and the library's
  `calculate_avatar_hash` and `calculate_dhash_fingerprint` compared with the published fields: 53 SHA-256 exact (9 of
  them with a dHash equal bit for bit, the other 44 at 12 to 35 bits, median 21: D32), 5 SHA-256 different (re-served
  bytes; one at 4 bits, the recompressed case D6 does not accept), 1 WebP (D8), 1 HTTP 404, 1 network failure.
- `dwcli --network testnet profile avatar <id> --size large`, on a fresh wallet holding nothing of these users, through
  the production guard (HTTPS, checked resolution, the redirect loop): a profile whose dHash equals the library's
  (`GgfFYvbn…`, Imgur), one 35 bits off (`EfPggELW…`, Imgur) and one 21 bits off (`EKDFyWWk…`, Gravatar) each return a
  256 × 256 PNG; the re-served Imgur picture (`yqwzfRNk…`) is `avatar.hash_mismatch`; the WebP one (`7VA7RejC…`) is
  `avatar.unsupported`. A second read is served from `avatars/` (its row's `fetched_at` unchanged; thumbnails written through the directory descriptor, `0600` in a
  `0700` directory). With `profile pictures --set off`, another user's avatar is `null` without a read, and the toggle
  reads back `true` after `--set on`. Nothing was signed or broadcast.

## Follow-ups

- F1 (D2): Imgur upload with DCG's credentials (B1), the delete hash in the vault (`integration/imgur/<delete-hash>`).
- F2 (D5): the engine proxy setting, then avatars through it.
- F3 (D8): WebP, if profiles with it appear (T2 found one of 59).
- F4 (D16): Windows handle-relative thumbnail I/O (`NtCreateFile` with a root handle) with W-01.
- F5 (D17): the Settings ▸ Privacy row "Load contact pictures" (DP4-03), and pictures in every list.
- F6 (D19): an avatar in the registration's initial profile.
- F7 (D24): a decode deadline (a decoder that checks a cancel flag, or a decode subprocess). Residual until then
  (Sol r1): two decode slots, and a caller that gives up (or is abandoned, D33) does not stop the blocking decode,
  so a hostile valid image within the limits can hold both slots for as long as it takes to decode.
- F8: rotation by last use rather than by fetch time (the single-flight half is D35).
- F9 (DP4-03): `useAvatar` keys invalidated when the toggle changes, and an e2e case for `Avatar`'s `onError`
  fallback (a thumbnail that is not a PNG).
- F10 (BL-190): before this wallet publishes any `avatarFingerprint` (F1 upload, F6, a DP4-01 edit setting an avatar),
  compute it the way the mobile wallets do, or their 20-bit check refuses our avatars. T2 compared the library's dHash
  with fingerprints already on testnet, not with a mobile implementation; D6's "9 × 9, mean grey, bilinear" for
  Android is unverified. Check dash-wallet (Android) and the iOS DashPay fingerprint code first.
