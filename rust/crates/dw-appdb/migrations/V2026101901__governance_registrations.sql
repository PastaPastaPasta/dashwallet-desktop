-- The ProRegTx of each listed masternode, keyed by its proTxHash (wire
-- order), as governance fetched it to learn the collateral outpoint a vote
-- names (MG-02 D80). Each row checks itself: the engine keeps a row only if
-- its bytes hash to the key and name a collateral, and deletes it
-- otherwise. Not wallet data: no `wallet_id`, not carried by a `.dwbackup`.
CREATE TABLE governance_registrations (
    pro_tx_hash BLOB PRIMARY KEY NOT NULL CHECK (length(pro_tx_hash) = 32),
    registration BLOB NOT NULL
) STRICT;
