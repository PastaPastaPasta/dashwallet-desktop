// A spend refused while the wallet's filter scan is replayed (0.1.x-bal §1): `rescan_in_progress`
// ends by itself; `rescan_interrupted` (a rescan the user stopped part way) needs a rescan again.

import { messageTable } from "../i18n";

export const RESCAN_TEXT = messageTable("app.rescan", ["running", "interrupted"] as const);
