// A spend refused while the wallet's filter scan is replayed (0.1.x-bal §1): `rescan_in_progress`
// ends by itself; `rescan_interrupted` (a rescan the user stopped part way) needs a rescan again.
export const RESCAN_RUNNING = "The wallet is catching up with the network. You can send once it has finished.";
export const RESCAN_INTERRUPTED = "The last rescan was interrupted. Finish it in Repair ▸ Rescan before sending.";
