# External blockers (DCG / pasta)

Status: 2026-10-08. Only true blockers are listed here: things this program cannot produce itself (credentials,
signing identities, money, hardware). The product and engineering decisions are settled in [`DASHPAY.md`](DASHPAY.md)
§0, either by the manager (DEC-01, DEC-12, DEC-13, DEC-15) or by the design. Task IDs are in
[`ROADMAP.md`](ROADMAP.md). **None of these blocks the DashPay core.** Each blocks only the items named, and each is
raised when that work is reached, not before.

| # | Blocker | Owner | Blocks | Until it arrives | Raise it |
|---|---|---|---|---|---|
| **B1** | **Integration credentials** (DEC-15), in priority order: (1) a desktop **Imgur** client id; (2) a **CTX** `X-Client-Id` for desktop; (3) **Coinbase/Uphold** redirect URIs, preferably an RFC 8252 loopback with PKCE; (4) **Topper** and **ZenLedger**, with server-side signing or an accepted extraction risk; (5) a **SwapKit** API key | DCG | DP4-02's Imgur upload; MP-10 DashSpend, MP-11 Buy & Sell, MP-12 ZenLedger | Each integration ships **disabled** and hidden. Avatars use URL, Gravatar or file sources. | now (lead time) |
| **B2** | **Code-signing identities as CI secrets**: an Apple Developer ID certificate plus notarization credentials, and a Windows Authenticode certificate | DCG | signed and notarized release artifacts (PK-02's signed half, W-03), the auto-update feed (PK-03, 1.1) | Unsigned test builds from CI. macOS Gatekeeper and Windows SmartScreen warn on them, so a public 1.0 release needs B2. | now (lead time) |
| **B3** | **Mainnet canary budget**, ≤ 0.5 DASH: a non-contested name, payments both ways with an iOS user, fees | pasta | H-07 (release gate T4) | Everything else is tested on regtest, the dashmate devnet and testnet. The faucet's mainnet invitation covers the claim test itself. | at W10 |
| **B4** | **One real-Mac session** (about half a day; with nothing opening windows on the developer's working session, per `CLAUDE.md`): Touch ID quick unlock in the new stack, the notarized first launch (needs B2), VoiceOver spot checks | pasta | shipping Touch ID quick unlock on macOS; H-05's VoiceOver part | CI's macOS runners cover building, launching and screenshots. Without B4, macOS 1.0 ships passphrase unlock only. | at W9–W10 |
| **B5** | **Go-ahead for one public platform PR**: the dispatch fence of DASHPAY §2.6 ("Commit points"), which platform-wallet calls at every hand-off to a transport so that Lock orders against it. The desktop may carry it only as a cherry-pick of that public PR (DEC-18), and DASHPAY §0 R3 opens no upstream PR unless pasta asks | pasta | E0-04's commit points for the flows platform-wallet signs and submits itself (registration, top-up, documents, contact payments) | Those flows offer no "Lock to cancel" once they have signed: Lock still refuses every later signature, and the UI says a send already signed may still go out | at E0-04 start |
