# C3 (after #794/#739 lands) — funds reviewer Q75b conditions
- merge main rerere off; assert_ne!(DROP_HOLDS_PREFIX, DROPPED_PREFIX) (test or const assert)
- C1 test: write the #739 key via #739's real writer (not literal "dropped:")
- N2: orphan_exempt must ALSO include #739's load_dropped set; claim_swept: keys go into forget_swept's single durable write
- keep the two dropped sets separate (#739 LoadedHistory.dropped vs PR A orphan_exempt)
- combined full dw-engine --tests incl dropped_claim_tests.rs + a_claim_with_an_input_script_that_isnt_push_only_is_kept_after_a_restart
- send PO: `-- rust/` remerge-diff + sha256
- PR body: L5, L6, failed-load fail-closed, F2 wording (PO 11-24 msg); #789, #797 (claim_swept residual gap), #749, L9 downgrade note
