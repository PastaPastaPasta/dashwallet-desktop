-- Evonode Platform credit withdrawals (MG-13 P5, P6) and payout-change
-- barriers (MG-13 P16). Both tables are app-wide: a record is keyed by the
-- evonode's identity, and `created_by` is only a label naming the wallet that
-- wrote it, never a `wallet_id` a wallet removal deletes (neither table is in
-- dw-appdb `WALLET_TABLES`, nor in a `.dwbackup`).

-- One row per signed withdrawal transition, written before any network step
-- and never deleted (P5).
--   id                 engine-chosen.
--   transition_hash    the signed transition's hash, 64 lower-case hex.
--   pro_tx_hash        the evonode, 64 lower-case hex as displayed.
--   identity_id        its Platform identity (the reversed ProTx hash), in
--                      the engine's encoding.
--   key_kind           the signing key: `owner` (the destination is Drive's
--                      at execution, `expected_script` what the quote saw) or
--                      `payout` (`script` is the signed `output_script`).
--   amount             credits; core_fee_per_byte, pooling (dpp `Pooling` as
--                      its integer) and expected_duffs as signed.
--   nonce              the identity nonce signed at; one identity's records
--                      at one nonce are a nonce group (P6).
--   signed_bytes       the transition's serialized bytes, rebroadcast
--                      unchanged only.
--   paid_script        the script of the matching withdrawal document (P7).
--   document_status    that document's `status` (Drive's integer), kept
--                      current.
--   signed_at, sent_at UNIX seconds; `sent_at` is set before the first
--                      hand-off and never cleared.
--   state              P6's table; `state_reason` the refusal or the
--                      undecidable read.
--   proof_*            the proved document behind `done`: its id, the proof's
--                      Platform height and block time (`proof_block_time`
--                      in milliseconds since the UNIX epoch, as Platform
--                      gives it).
--   evidence_*         P8's heights behind `not_executed`: (i) the nonce
--                      read, (ii) the identity read, (iv) the document reads.
--   floor_height       the highest proved Platform height an SPV-verified read
--                      for the record used; it only rises, and every record
--                      of a group carries the group's.
--   last_checked       UNIX seconds of the last recheck.
--   user_mark          the user's mark (P9), `marked_at` its time; apart from
--                      the proof.
--   late_proof_link    the id of the later record a late proof points at.
CREATE TABLE mn_withdrawals (
    id TEXT NOT NULL PRIMARY KEY,
    transition_hash TEXT NOT NULL
        CHECK (length(transition_hash) = 64 AND transition_hash NOT GLOB '*[^0-9a-f]*'),
    created_by TEXT NOT NULL,
    pro_tx_hash TEXT NOT NULL
        CHECK (length(pro_tx_hash) = 64 AND pro_tx_hash NOT GLOB '*[^0-9a-f]*'),
    identity_id TEXT NOT NULL CHECK (identity_id <> ''),
    key_kind TEXT NOT NULL CHECK (key_kind IN ('owner', 'payout')),
    amount INTEGER NOT NULL CHECK (amount > 0),
    core_fee_per_byte INTEGER NOT NULL CHECK (core_fee_per_byte >= 0),
    pooling INTEGER NOT NULL CHECK (pooling >= 0),
    expected_duffs INTEGER NOT NULL CHECK (expected_duffs >= 0),
    nonce INTEGER NOT NULL CHECK (nonce >= 0),
    signed_bytes BLOB NOT NULL CHECK (length(signed_bytes) > 0),
    script BLOB,
    expected_script BLOB,
    paid_script BLOB,
    document_status INTEGER CHECK (document_status >= 0),
    signed_at INTEGER NOT NULL,
    sent_at INTEGER,
    state TEXT NOT NULL CHECK (state IN (
        'signed', 'sent', 'refused', 'superseded', 'couldnt_confirm',
        'marked_received', 'marked_not_received', 'done', 'sibling_paid', 'not_executed')),
    state_reason TEXT,
    proof_document_id TEXT,
    proof_height INTEGER,
    proof_block_time INTEGER,
    evidence_nonce_height INTEGER,
    evidence_identity_height INTEGER,
    evidence_document_height INTEGER,
    floor_height INTEGER NOT NULL DEFAULT 0 CHECK (floor_height >= 0),
    last_checked INTEGER,
    user_mark TEXT CHECK (user_mark IN ('received', 'not_received')),
    marked_at INTEGER,
    late_proof_link TEXT,
    -- The payout key signs its destination; the owner key's is Drive's.
    CHECK ((key_kind = 'payout') = (script IS NOT NULL)),
    CHECK ((key_kind = 'owner') = (expected_script IS NOT NULL)),
    -- `done` rests on a proved document (P7).
    CHECK (state <> 'done' OR (proof_document_id IS NOT NULL AND proof_height IS NOT NULL
        AND paid_script IS NOT NULL)),
    -- `not_executed` rests on P8's reads.
    CHECK (state <> 'not_executed' OR (evidence_nonce_height IS NOT NULL
        AND evidence_identity_height IS NOT NULL AND evidence_document_height IS NOT NULL)),
    CHECK ((user_mark IS NULL) = (marked_at IS NULL))
) STRICT;

-- P6's guard across processes: at most one open record per identity. SQLite
-- checks it per statement, so Try again moves the refused record out of the
-- open set before it inserts the sibling (P15).
CREATE UNIQUE INDEX mn_withdrawals_open ON mn_withdrawals (identity_id)
WHERE state IN ('signed', 'sent', 'refused', 'couldnt_confirm');

CREATE INDEX mn_withdrawals_group ON mn_withdrawals (identity_id, nonce);
CREATE INDEX mn_withdrawals_pro_tx ON mn_withdrawals (pro_tx_hash);

-- A signed row is never deleted (P5).
CREATE TRIGGER mn_withdrawals_kept BEFORE DELETE ON mn_withdrawals
BEGIN SELECT RAISE(ABORT, 'mn_withdrawals rows are never deleted'); END;

-- What was signed never changes: the bytes are rebroadcast unchanged only.
CREATE TRIGGER mn_withdrawals_signed_fixed BEFORE UPDATE OF
    id, transition_hash, created_by, pro_tx_hash, identity_id, key_kind, amount,
    core_fee_per_byte, pooling, expected_duffs, nonce, signed_bytes, script,
    expected_script, signed_at ON mn_withdrawals
WHEN NEW.id IS NOT OLD.id OR NEW.transition_hash IS NOT OLD.transition_hash
    OR NEW.created_by IS NOT OLD.created_by OR NEW.pro_tx_hash IS NOT OLD.pro_tx_hash
    OR NEW.identity_id IS NOT OLD.identity_id OR NEW.key_kind IS NOT OLD.key_kind
    OR NEW.amount IS NOT OLD.amount OR NEW.core_fee_per_byte IS NOT OLD.core_fee_per_byte
    OR NEW.pooling IS NOT OLD.pooling OR NEW.expected_duffs IS NOT OLD.expected_duffs
    OR NEW.nonce IS NOT OLD.nonce OR NEW.signed_bytes IS NOT OLD.signed_bytes
    OR NEW.script IS NOT OLD.script OR NEW.expected_script IS NOT OLD.expected_script
    OR NEW.signed_at IS NOT OLD.signed_at
BEGIN SELECT RAISE(ABORT, 'a signed withdrawal is not changed'); END;

-- `sent_at` is set once; `floor_height` only rises (P5).
CREATE TRIGGER mn_withdrawals_sent_once BEFORE UPDATE OF sent_at ON mn_withdrawals
WHEN OLD.sent_at IS NOT NULL AND NEW.sent_at IS NOT OLD.sent_at
BEGIN SELECT RAISE(ABORT, 'sent_at is set once'); END;
CREATE TRIGGER mn_withdrawals_floor_rises BEFORE UPDATE OF floor_height ON mn_withdrawals
WHEN NEW.floor_height < OLD.floor_height
BEGIN SELECT RAISE(ABORT, 'floor_height only rises'); END;

-- P6's table, for any writer: nothing leaves a final state, and nothing
-- moves a record that is not open into the open set.
CREATE TRIGGER mn_withdrawals_transitions BEFORE UPDATE OF state ON mn_withdrawals
WHEN NEW.state IS NOT OLD.state AND NOT (
    (OLD.state = 'signed' AND NEW.state IN (
        'sent', 'refused', 'couldnt_confirm', 'done', 'sibling_paid', 'not_executed'))
    OR (OLD.state = 'sent' AND NEW.state IN (
        'refused', 'couldnt_confirm', 'done', 'sibling_paid', 'not_executed'))
    OR (OLD.state = 'refused' AND NEW.state IN (
        'superseded', 'couldnt_confirm', 'done', 'sibling_paid', 'not_executed'))
    OR (OLD.state = 'superseded' AND NEW.state IN ('done', 'sibling_paid', 'not_executed'))
    OR (OLD.state = 'couldnt_confirm' AND NEW.state IN (
        'marked_received', 'marked_not_received', 'done', 'sibling_paid', 'not_executed'))
    OR (OLD.state IN ('marked_received', 'marked_not_received')
        AND NEW.state IN ('done', 'sibling_paid', 'not_executed'))
)
BEGIN SELECT RAISE(ABORT, 'mn_withdrawals: transition not in MG-13 P6'); END;

-- Payout-change barriers (P16): one per signed ProUpRegTx whose terms change
-- the payout, written in the transaction that writes the operation's signed
-- bytes. A barrier holds withdrawals of its masternode (P4 (b)) until a
-- clearing term holds; a wallet removal never deletes one (fail closed).
--   op_id, created_by  the `masternode_ops` row (`id`, `wallet_id`).
--   txid               its transaction, 64 lower-case hex.
--   payout_script      the new payout script.
CREATE TABLE mn_payout_barriers (
    id INTEGER PRIMARY KEY,
    pro_tx_hash TEXT NOT NULL
        CHECK (length(pro_tx_hash) = 64 AND pro_tx_hash NOT GLOB '*[^0-9a-f]*'),
    op_id TEXT NOT NULL,
    txid TEXT NOT NULL CHECK (length(txid) = 64 AND txid NOT GLOB '*[^0-9a-f]*'),
    payout_script BLOB NOT NULL CHECK (length(payout_script) > 0),
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (created_by, op_id)
) STRICT;
CREATE INDEX mn_payout_barriers_pro_tx ON mn_payout_barriers (pro_tx_hash);
