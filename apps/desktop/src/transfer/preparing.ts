// A transfer's call can first wait for the wallet's start-up to run again (up to about 50 s after a
// restart): once it has been pending for `PREPARING_AFTER_MS`, its progress says it is preparing.
// The usual call answers sooner and shows nothing.

import { t } from "../i18n";

export const PREPARING_AFTER_MS = 1000;

export const preparingText = () => t("transfer.preparing");

/** Whether the call started last has been pending for `PREPARING_AFTER_MS`; `changed` when that flips. */
export class Preparing {
  shown = false;
  #timer: ReturnType<typeof setTimeout> | null = null;

  constructor(private readonly changed: () => void) {}

  start() {
    this.stop();
    this.#timer = setTimeout(() => {
      this.#timer = null;
      this.shown = true;
      this.changed();
    }, PREPARING_AFTER_MS);
  }

  stop() {
    if (this.#timer !== null) clearTimeout(this.#timer);
    this.#timer = null;
    this.shown = false;
  }
}
