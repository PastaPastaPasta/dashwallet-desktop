import type { Tree } from "../core";

export const routes = {
  home: "Home",
  pay: "Pay",
  receive: "Receive",
  contacts: "Contacts",
  activity: "Activity",
  coinJoin: "CoinJoin",
  explore: "Explore",
  masternodes: "Masternodes",
  notifications: "Notifications",
  profile: "My Profile",
  settings: "Settings",
  tools: "Tools",
  joinDashPay: "Join DashPay",
} as const satisfies Tree;
