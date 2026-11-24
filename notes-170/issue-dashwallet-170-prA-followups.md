Three follow-ups from the funds review of the #170 cases 3 and 4 change, which drops restored payments that can never confirm. That change ships without them. Line numbers are at 36e886df5.

### 1. A beaten payment's children lose their claims to a restored lock that itself lost (`history.rs:469`, `unmined_own_spends`)

The claim walk into a beaten payment's unlocked children counts a restored InstantSend lock as the winner. If that lock itself lost to a ChainLocked spend, the payment wasn't really beaten by it, but the child still loses its claim.

- **Example:** P spends A. P is beaten only by a restored lock L, and a ChainLocked Y beat L. P's child C spends D.
- **Today:** C loses its claim on D at load.
- **Expected:** C keeps its claim on D.
- **Fix:** walk into children only when the winner is mined.
- **Test:** the shape above. C keeps its claim on D after a restart.

### 2. No deterministic test for the re-check under the write lock (`drop_dead` and `drop_dead_unmined`)

`drop_dead_unmined` runs `beaten_unrecorded` again under the wallet manager's write lock, and keeps a root unless the winner (height included) is identical. Today the only argument for this is written; no test covers it.

- **Fix:** add a `pause_*` test hook between `drop_dead`'s read and `drop_dead_unmined`'s write lock.
- **Test:** pause there, truncate the winner's block, release, and check that nothing is dropped or freed.

### 3. Observed spends are pruned before the reconcile on a ChainLocked catch-up

On a ChainLocked catch-up, key-wallet prunes `observed_spent_outpoints` before the duplicate-coin reconcile runs. A drop that relies on `observed` to keep a coin a processed block spent can then miss that block.

- The sweep path's queued children also have no re-check under the write lock. They keep `winner.mined` from the event, which is the same problem.
- key-wallet's own sweep has already made the matching irreversible call, so the fix probably belongs in how the engine keeps observed spends across the catch-up.
- **Test:** a ChainLocked catch-up that truncates the winner's block before the reconcile. No coin that block spent is freed.
