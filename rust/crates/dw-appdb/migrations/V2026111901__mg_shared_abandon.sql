-- A shared session's abandon spends (MG-10 §4.4 EP 4, storage).
--   abandon_tx     consensus bytes of the latest abandon spend (`abandon_txid`'s).
--   abandon_log    JSON array, one entry per abandon spend the session signed,
--                  oldest first: {txid, fee, inputs: ["txid:vout", ...],
--                  signed_at, outcome: null | "sent" | "not_sent" | "maybe_sent",
--                  code, legacy}. Entries are only appended; an outcome only
--                  moves forward.
--   legacy_bound   1 while a spend back must take the whole `reserved`: an
--                  older build may have spent any reserved coin and kept no
--                  record of it. Cleared only by a spend taking all of it.
--   resend_marked  1 once a resend of the session's own bytes was handed off;
--                  never cleared.
ALTER TABLE shared_mn_sessions ADD COLUMN abandon_tx BLOB
    CHECK (abandon_tx IS NULL OR abandon_txid IS NOT NULL);
ALTER TABLE shared_mn_sessions ADD COLUMN abandon_log TEXT
    CHECK (abandon_log IS NULL OR json_valid(abandon_log));
ALTER TABLE shared_mn_sessions ADD COLUMN legacy_bound INTEGER NOT NULL DEFAULT 0
    CHECK (legacy_bound IN (0, 1));
ALTER TABLE shared_mn_sessions ADD COLUMN resend_marked INTEGER NOT NULL DEFAULT 0
    CHECK (resend_marked IN (0, 1));

-- An older build's abandon: its id only. Whether its first hand-off was
-- refused can't be shown, so it reads `maybe_sent`, signed at the upgrade.
UPDATE shared_mn_sessions
SET abandon_log = json_array(json_object(
        'txid', abandon_txid, 'fee', NULL, 'inputs', json('[]'),
        'signed_at', CAST(strftime('%s', 'now') AS INTEGER),
        'outcome', 'maybe_sent', 'code', NULL, 'legacy', json('true'))),
    legacy_bound = 1
WHERE abandon_txid IS NOT NULL;

-- An older build's refusal on an out row may be a resend's, after an earlier
-- hand-off that may be out: it reads unknown. The list is dw-appdb
-- `INHERITED_REFUSED_CODES`.
UPDATE shared_mn_sessions SET last_error = 'send.broadcast_unknown'
WHERE out_at IS NOT NULL
  AND last_error IN ('send.no_peers', 'send.broadcast_rejected', 'send.prepared_tx_spent',
                     'send.cancelled', 'network_not_open', 'wallet_not_found');
