// dw-fixture-module: never in a production build (vite.config.ts, noFixtureModules).
// The fixture's answers for the commands dw-app does not have yet (api/pending.ts): the network
// choice and the shielded and Platform balances. They are not in the generated `COMMANDS`, so
// they sit beside `HANDLERS` (index.ts looks here second). A test makes one "absent" by setting
// its field to `null`: the handler then rejects as an unknown command does.

import { PENDING_COMMANDS as PENDING, type NetworkChoice } from "../api/pending";
import type { Ctx } from "./handlers";

export interface PendingWorld {
  /** `get_network_choice`; `null`: dw-app has no such command. */
  networkChoice: NetworkChoice | null;
  /** The shielded balance in duffs; `null`: the command is absent. */
  shielded: number | null;
  /** The Platform addresses' balance in duffs; `null`: the command is absent. */
  platform: number | null;
}

export const newPendingWorld = (): PendingWorld => ({ networkChoice: "testnet", shielded: 125_000_000, platform: 40_000_000 });

const absent = (call: string) => ({ code: "platform.not_implemented", message: `the fixture backend has no handler for ${call}`, data: { kind: "call", call } });

export const PENDING_HANDLERS: Record<string, (args: never, ctx: Ctx) => unknown> = {
  [PENDING.getNetworkChoice]: (_: unknown, { world }: Ctx) => {
    if (world.pending.networkChoice === null) throw absent(PENDING.getNetworkChoice);
    return world.pending.networkChoice;
  },
  [PENDING.setNetworkChoice]: ({ choice }: { choice: NetworkChoice }, { world }: Ctx) => {
    if (world.pending.networkChoice === null) throw absent(PENDING.setNetworkChoice);
    world.pending.networkChoice = choice;
    return null;
  },
  [PENDING.shieldedBalance]: (_: unknown, { world }: Ctx) => {
    if (world.pending.shielded === null) throw absent(PENDING.shieldedBalance);
    return world.pending.shielded;
  },
  [PENDING.platformBalance]: (_: unknown, { world }: Ctx) => {
    if (world.pending.platform === null) throw absent(PENDING.platformBalance);
    return world.pending.platform;
  },
};
