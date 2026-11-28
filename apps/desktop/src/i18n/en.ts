// English, the source catalog (MP-14), one file under ./catalog per screen area; `{name}` is a placeholder,
// `<tag>…</tag>` rich text, and `{ one, other }` a plural chosen by `count`. The terms in
// ./glossary.md keep their meaning in every language. docs/design/LOCALIZATION.md says how to add one.

import type { Tree } from "./core";
import { activity } from "./catalog/activity";
import { app } from "./catalog/app";
import { coinjoin } from "./catalog/coinjoin";
import { coins } from "./catalog/coins";
import { common } from "./catalog/common";
import { components } from "./catalog/components";
import { contacts } from "./catalog/contacts";
import { dashpay } from "./catalog/dashpay";
import { giftCards } from "./catalog/giftCards";
import { home } from "./catalog/home";
import { lock } from "./catalog/lock";
import { masternodes } from "./catalog/masternodes";
import { network } from "./catalog/network";
import { notifications } from "./catalog/notifications";
import { onboarding } from "./catalog/onboarding";
import { pay } from "./catalog/pay";
import { platform } from "./catalog/platform";
import { rates } from "./catalog/rates";
import { receive } from "./catalog/receive";
import { routes } from "./catalog/routes";
import { settings } from "./catalog/settings";
import { shell } from "./catalog/shell";
import { storeUpgrade } from "./catalog/storeUpgrade";
import { tools } from "./catalog/tools";
import { transfer } from "./catalog/transfer";

export const en = {
  common,
  routes,
  shell,
  lock,
  network,
  rates,
  settings,
  components,
  app,
  storeUpgrade,
  onboarding,
  giftCards,
  home,
  activity,
  receive,
  pay,
  coins,
  platform,
  contacts,
  dashpay,
  coinjoin,
  notifications,
  masternodes,
  tools,
  transfer,
} as const satisfies Tree;
