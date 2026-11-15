# Empty states, pull request 25

Fixture data only. Viewport 1280×820. Light and dark.

- Before: `b43c4154` (`origin/main` after "Pay that never dead-ends").
- After: `250ebc3d` (head of `ui/empty-states`).

Shared setup:

- Syncing shots start from the `empty` wallet and set sync to the filters phase at 60 percent. The status bar reads "Synchronizing with network… 65.0%".
- All-mixed coin selection starts from `funded` and marks that wallet's coins mixed.
- The no-wallet page and the empty-list tool tab are covered by onboarding, which cannot be cancelled. Onboarding is hidden in those two shots so the page underneath is visible.
- The failed wallet list is `funded` with `list_wallets` returning no data, then Tools → Address Book.
- Receive shows the empty wallet's fixture address.
