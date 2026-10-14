import AxeBuilder from "@axe-core/playwright";
import type { Page } from "@playwright/test";

export const AXE_TAGS = ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "best-practice"];

/** Serious and critical axe violations on the page, one line each (U4: there must be none). */
export async function seriousAxeViolations(page: Page): Promise<string[]> {
  const result = await new AxeBuilder({ page }).withTags(AXE_TAGS).analyze();
  return result.violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`);
}
