ALTER TABLE receive_requests ADD COLUMN include_username INTEGER
    CHECK (include_username IS NULL OR include_username IN (0, 1));
