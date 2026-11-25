import type { Tree } from "../core";

export const shell = {
  experimental: "Experimental",
  sidebar: {
    label: "Main",
    waiting: { one: "{count} waiting", other: "{count} waiting" },
  },
  toolbar: {
    wallet: "Wallet",
    notifications: "Notifications",
    notificationsUnread: { one: "Notifications, {count} unread", other: "Notifications, {count} unread" },
    showBalance: "Show balance",
    hideBalance: "Hide balance",
    lockWallet: "Lock wallet",
    myProfile: "My profile",
    myProfileNamed: "My profile, {name}",
    usernameNotLoaded: "Username not loaded",
    noUsername: "No username",
    updating: "Updating…",
    contestPending: "{name} · Pending",
    registeringName: "{name} · Registering",
    registering: "Registering…",
  },
  status: {
    connecting: "Connecting…",
    notSyncing: "Not syncing",
    upToDate: "Up to date",
    synchronizing: "Synchronizing with network…",
    syncProgress: "Sync progress",
    hd: "HD",
    peers: { one: "{count} peer", other: "{count} peers" },
    offline: "Offline",
    synced: "Synced",
    syncing: "Syncing",
    /** The sync button while a Platform or Shielded sync did not finish (nothing still syncing). */
    incomplete: "Incomplete",
    /** The status bar's state while the network is up to date and Platform's first sync runs. */
    syncingPlatform: "Syncing Platform…",
    /** The status bar's state while the network is up to date and Shielded's first scan runs. */
    syncingShielded: "Syncing Shielded…",
    platformIncomplete: "Platform sync didn't finish",
    shieldedIncomplete: "Shielded sync didn't finish",
    unlockFully: "Unlock fully",
    lock: {
      noVault: "No keys",
      noKeys: "Watch-only",
      unencrypted: "Not encrypted",
      locked: "Locked",
      unlockedMixingOnly: "Unlocked for mixing only",
      unlocked: "Unlocked",
    },
    /** The status bar's sync item is a button that opens Sync progress. */
    showSync: "Show sync progress",
  },
  /** Sync progress (IOS-023): each SPV phase, the latest block, the peers. */
  sync: {
    /** Sync progress's list of the syncs the wallet waits on, above the network's phases. */
    items: {
      title: "Syncs",
      core: "Dash network",
      platform: "Platform",
      shielded: "Shielded",
      syncing: "Syncing…",
      /** Shielded's first scan; `percent` is formatted. */
      scan: "{percent} scanned",
      unlock: "Unlock to sync",
      watchOnly: "Not available in a watch-only wallet",
      platformIncomplete: "Didn't finish. It tries again the next time you start Dash Wallet.",
      shieldedIncomplete: "The last pass failed. It tries again shortly.",
    },
    phase: {
      headers: "Block headers",
      filterHeaders: "Filter headers",
      filters: "Block filters",
      masternodes: "Masternode list",
    },
    done: "Done",
    /** Under "Up to date" while the status bar holds it through a new block (the rows may read 99.9%). */
    processingBlock: "Processing the latest block…",
    waiting: "Waiting",
    /** `current` and `target` are block heights; `percent` is formatted. */
    progress: "{current} of {target} ({percent})",
    latestBlock: "Latest block",
    latestBlockValue: "{height} · {time}",
    chainlock: "Latest ChainLocked block",
    peers: "Connected peers",
    stalled: { one: "No progress for {count} minute. Check your network connection, or choose Change peers in Tools ▸ Peers.", other: "No progress for {count} minutes. Check your network connection, or choose Change peers in Tools ▸ Peers." },
  },
} as const satisfies Tree;
