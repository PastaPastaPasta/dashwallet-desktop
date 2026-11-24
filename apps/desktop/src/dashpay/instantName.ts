// The instant username beside a contested request (DASHPAY F5; DP1-09 D4 reversed, DP1-10 D13):
// the suggestion and the form's live checks in iOS's order. Plain TypeScript over the generated
// types; the offer's copy and the card's state are instantOffer.ts, the form
// screens/dashpay/Registration.tsx.
// Mobile: iOS `CreateUsernameViewModel.swift` (suggestion, checks), `CreateInstantUsernameSheet.swift`
// (offer); Android `strings` (same-name line).

import type { NameAvailability } from "../api/generated/NameAvailability";
import type { UsernameCheck } from "../api/generated/UsernameCheck";
import { Model } from "../onboarding/model";
import { homographSafe, INSTANT_COPY } from "./instantOffer";
import { AVAILABILITY_DEBOUNCE_MS, availabilityLine, MAX_LABEL, RULE_TEXT, type AvailabilityLine } from "./registration";

/** iOS `suggestedInstantUsername`: the request with a "2", which takes a label out of the contested
 * set; a request at the engine's cap loses its last character first. */
export function suggest(requested: string): string {
  const base = [...requested.trim()];
  return (base.length >= MAX_LABEL ? base.slice(0, MAX_LABEL - 1) : base).join("") + "2";
}

// ---------------------------------------------------------------------------------------------
// The form's live checks (iOS 1939-2000)

export type InstantCheckKind = "empty" | "rules" | "contested" | "same" | "availability";

export interface InstantCheck {
  kind: InstantCheckKind;
  /** The line under the field; `null` for an empty field or while the checklist is not read. */
  line: AvailabilityLine | null;
  /** The name may be chosen. */
  ok: boolean;
}

/** The checks in iOS's order: the rules, not contested, not the request (Android), then the
 * network's availability. `check` is `check_username` of `candidate`; `availability` and
 * `pending` are its debounced lookup's. */
export function instantCheck(
  requested: string,
  candidate: string,
  check: UsernameCheck | undefined,
  availability: NameAvailability | undefined,
  pending: boolean,
): InstantCheck {
  if (candidate === "") return { kind: "empty", line: null, ok: false };
  if (!check) return { kind: "rules", line: null, ok: false };
  if (!check.valid) {
    const broken = check.rules.find((r) => !r.passed);
    return { kind: "rules", line: broken ? { text: RULE_TEXT[broken.rule], tone: "problem" } : null, ok: false };
  }
  if (check.contested) return { kind: "contested", line: { text: INSTANT_COPY.contested, tone: "problem" }, ok: false };
  if (check.normalized === homographSafe(requested)) return { kind: "same", line: { text: INSTANT_COPY.same, tone: "problem" }, ok: false };
  const line = availabilityLine(check, availability, pending);
  const ok = !pending && (availability?.kind === "available" || availability?.kind === "unknown");
  return { kind: "availability", line, ok };
}

/** The availability lookup behind the checks: asked once the label has stayed the same for
 * `delay`, and only for a label `set` still holds when its answer comes (a stale one is dropped).
 * A failed lookup is "could not check", as on step 1. */
export class AvailabilityChecker extends Model {
  #label: string | null = null;
  #seq = 0;
  #timer: ReturnType<typeof setTimeout> | undefined;
  #answer: { label: string; availability: NameAvailability } | null = null;

  constructor(
    private readonly lookup: (label: string) => Promise<NameAvailability>,
    private readonly delay = AVAILABILITY_DEBOUNCE_MS,
  ) {
    super();
  }

  /** The label to look up, `null` for none (it failed an earlier check). */
  set(label: string | null) {
    if (label === this.#label) return;
    this.#label = label;
    const seq = ++this.#seq;
    clearTimeout(this.#timer);
    if (label !== null) {
      this.#timer = setTimeout(() => {
        const settle = (availability: NameAvailability) => {
          if (seq !== this.#seq) return;
          this.#answer = { label, availability };
          this.changed();
        };
        this.lookup(label).then(settle, () => settle({ kind: "unknown" }));
      }, this.delay);
    }
    this.changed();
  }

  /** The label set now. */
  get label(): string | null {
    return this.#label;
  }

  /** The answer for the label set now; `undefined` while it is pending. */
  get availability(): NameAvailability | undefined {
    return this.#answer !== null && this.#answer.label === this.#label ? this.#answer.availability : undefined;
  }

  get pending(): boolean {
    return this.#label !== null && this.availability === undefined;
  }

  dispose() {
    this.#seq++;
    clearTimeout(this.#timer);
  }
}
