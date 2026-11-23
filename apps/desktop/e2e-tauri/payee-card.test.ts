import { describe, expect, it } from "vitest";
import { payeeCardMismatch } from "./payee-card.ts";

describe("the Pay form's payee card (DP3-06)", () => {
  it("shows the display name over @username, each in its own place", () => {
    expect(payeeCardMismatch({ name: "QA B", sub: "@dwo4ab8372" }, "dwo4ab8372", "QA B")).toBeNull();
    expect(payeeCardMismatch({ name: "@dwo4ab8372", sub: "QA B" }, "dwo4ab8372", "QA B")).toMatch(/name "@dwo4ab8372", not "QA B"/);
    expect(payeeCardMismatch({ name: "QA C", sub: "@dwo4ab8372" }, "dwo4ab8372", "QA B")).toMatch(/name "QA C"/);
    expect(payeeCardMismatch({ name: "QA B", sub: null }, "dwo4ab8372", "QA B")).toMatch(/subtitle null, not "@dwo4ab8372"/);
    expect(payeeCardMismatch({ name: "QA B", sub: "@dwo4ab83721" }, "dwo4ab8372", "QA B")).toMatch(/subtitle/);
  });

  it("shows the username alone when the contact has no display name", () => {
    expect(payeeCardMismatch({ name: "dwo4ab8372", sub: null }, "dwo4ab8372", "")).toBeNull();
    expect(payeeCardMismatch({ name: "dwo4ab8372", sub: "@dwo4ab8372" }, "dwo4ab8372", "")).toMatch(/subtitle "@dwo4ab8372", not null/);
    expect(payeeCardMismatch({ name: "", sub: null }, "dwo4ab8372", "  ")).toMatch(/name ""/);
  });

  it("without a known display name takes either shape, each part in its own place", () => {
    expect(payeeCardMismatch({ name: "QA B", sub: "@dwo4ab8372" }, "dwo4ab8372")).toBeNull();
    expect(payeeCardMismatch({ name: "dwo4ab8372", sub: null }, "dwo4ab8372")).toBeNull();
    expect(payeeCardMismatch({ name: "@dwo4ab8372", sub: "QA B" }, "dwo4ab8372")).not.toBeNull();
    expect(payeeCardMismatch({ name: "dwo4ab8372", sub: "@dwo4ab8372" }, "dwo4ab8372")).not.toBeNull();
    expect(payeeCardMismatch({ name: "QA B", sub: null }, "dwo4ab8372")).not.toBeNull();
    expect(payeeCardMismatch({ name: "", sub: "@dwo4ab8372" }, "dwo4ab8372")).not.toBeNull();
  });
});
