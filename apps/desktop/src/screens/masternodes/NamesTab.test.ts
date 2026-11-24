// The Username voting pane as React renders it (MG-11 F1): a contender's own spelling is text with
// its identity beside it, never markup, and one without a spelling shows its identity alone.

import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ContestedName } from "../../api/generated/ContestedName";
import type { ContestedNames } from "../../api/generated/ContestedNames";
import { FIXTURE_NAMES, ID_1, ID_2 } from "../../fixture/names";
import { NamePane } from "./NamesTab";

const NOW = 1_790_000_000;

function pane(contenders: ContestedName["contenders"]): string {
  const contest: ContestedName = { ...structuredClone(FIXTURE_NAMES.contests[0]), contenders };
  const names: ContestedNames = { ...structuredClone(FIXTURE_NAMES), contests: [contest] };
  return renderToStaticMarkup(createElement(NamePane, { contest, names, nodeNames: new Map(), watchOnly: false, now: NOW, onVote: () => {} }));
}

const text = (html: string) => html.replace(/<[^>]*>/g, "").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&#x27;/g, "'").replace(/&amp;/g, "&");

describe("the pane's contenders", () => {
  it("tells look-alike spellings apart by their identities", () => {
    const html = pane([
      { identity: ID_1, votes: 9, name: "Alice" },
      { identity: ID_2, votes: 4, name: "A11ce" },
    ]);
    expect(text(html)).toContain("Alice (8ZtW…R97P)");
    expect(text(html)).toContain("A11ce (4EfA…iSeF)");
    expect(text(html)).toContain("Alice (8ZtW…R97P) leads with 9 of 18 votes");
    expect(html).toContain(`title="${ID_1}"`);
    expect(html).toContain(`title="${ID_2}"`);
  });

  it("shows a contender without a spelling by its identity alone, in the identity's mono face", () => {
    const html = pane([
      { identity: ID_1, votes: 9, name: null },
      { identity: ID_2, votes: 4, name: "A11ce" },
    ]);
    expect(html).toContain(`<span class="mono" title="${ID_1}">8ZtW…R97P</span>`);
    expect(text(html)).not.toContain("(8ZtW…R97P)");
    expect(text(html)).toContain("A11ce (4EfA…iSeF)");
  });

  it("renders a spelling with markup in it as its literal characters", () => {
    const name = '<b>Alice</b><img src=x onerror="alert(1)">';
    const html = pane([{ identity: ID_1, votes: 9, name }]);
    expect(html).not.toMatch(/<b>|<img src="?x|onerror="alert/);
    expect(html).toContain("&lt;b&gt;Alice&lt;/b&gt;&lt;img src=x onerror=&quot;alert(1)&quot;&gt; (8ZtW…R97P)");
    expect(text(html)).toContain(`${name} (8ZtW…R97P) leads with 9 of 14 votes`);
  });
});
