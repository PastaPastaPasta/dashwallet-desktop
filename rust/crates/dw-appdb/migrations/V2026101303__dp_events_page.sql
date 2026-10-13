-- The bell's pages (DP2-05): one identity's journal rows below a cursor,
-- newest first. Without it the page query finds every row of the identity
-- through the journal's unique key and sorts them in a temporary B-tree on
-- each page. An index only: `.dwbackup` carries rows, not the schema.
CREATE INDEX dp_events_page ON dp_events (wallet_id, identity, id DESC);
