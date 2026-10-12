-- The claim that drives a registration row (DEC-171): every driver, discard,
-- resume and "Finish transfers" takes the row by raising `owner`, and every
-- write and delete of the row names the owner it holds, so a write from a
-- claim that was taken over (a stale driver, a retired task, a discard
-- that lost the row) changes nothing. The writes that only record evidence
-- (the changeset tap's lock outpoint, a stranded lock's new row) name none.
ALTER TABLE dp_registration ADD COLUMN owner INTEGER NOT NULL DEFAULT 0 CHECK (owner >= 0);
