// Core's ProTx refusal reasons the renderer names (`masternodes.protx_refused{reason}`), shared with
// the fixture so the production bundle holds them only once.

export const PAYEE_REUSE = "bad-protx-payee-reuse";
/** Another listed masternode uses the service address. */
export const DUP_SERVICE = "bad-protx-dup-netinfo-entry";
/** Another listed evonode uses the Platform node id. */
export const DUP_NODE = "bad-protx-dup-platformnodeid";
/** Another listed masternode uses the key. */
export const DUP_KEY = "bad-protx-dup-key";
/** One of the wallet's own pending operations holds the key. */
export const PENDING_DUP = "protx-dup";
