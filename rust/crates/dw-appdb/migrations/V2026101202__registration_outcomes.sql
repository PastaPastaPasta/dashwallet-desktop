-- A registration settles once its identity exists (DP1-02 Opus-high MJ-1):
-- the funds are the identity's now, so a name or an initial profile that
-- fails after it does not hold the flow. The row ends `done` with the step's
-- failure code kept here for the UI, which offers `register_name` or the
-- profile edit; `error` stays for the failures that stop the flow.
--
-- V2026101001's note on evidence writes predates DEC-173: evidence recorded
-- on an existing row (the changeset tap's lock outpoint, a stranded lock
-- handed to a waiting row) is written under the owner read with it.
ALTER TABLE dp_registration ADD COLUMN name_error TEXT;
ALTER TABLE dp_registration ADD COLUMN profile_error TEXT;
