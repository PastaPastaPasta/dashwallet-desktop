ALTER TABLE core_instant_locks
         ADD COLUMN verified INTEGER NOT NULL DEFAULT 0 CHECK (verified IN (0, 1));