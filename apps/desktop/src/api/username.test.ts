import { describe, expect, it } from "vitest";
import type { ReceiveRequest } from "./generated/ReceiveRequest";
import { changingUsernameInRequests, isChangingUsernameInRequests, withoutUsername } from "./username";

const request = (uri: string, username: string | null): ReceiveRequest => ({
  id: 1,
  createdAt: 0,
  uri,
  address: "yAddr",
  amount: null,
  label: null,
  message: null,
  username,
});

describe("withoutUsername (DP3-03b D22)", () => {
  it("drops the trailing user= the engine appends, leaving dash-qt's URI", () => {
    expect(withoutUsername(request("dash:yAddr?user=alice", "alice")).uri).toBe("dash:yAddr");
    const named = request("dash:yAddr?amount=1.50000000&label=Rent%20%26%20co&message=May%2FJune&user=al%20ice", "al ice");
    expect(withoutUsername(named)).toEqual({
      ...named,
      uri: "dash:yAddr?amount=1.50000000&label=Rent%20%26%20co&message=May%2FJune",
      username: null,
    });
  });

  it("leaves a request without a username as it is", () => {
    const plain = request("dash:yAddr?label=user", null);
    expect(withoutUsername(plain)).toBe(plain);
  });
});

it("a change of the setting is under way until it is done", () => {
  const done = changingUsernameInRequests("w1");
  expect(isChangingUsernameInRequests("w1")).toBe(true);
  expect(isChangingUsernameInRequests("w2")).toBe(false);
  done();
  expect(isChangingUsernameInRequests("w1")).toBe(false);
});
