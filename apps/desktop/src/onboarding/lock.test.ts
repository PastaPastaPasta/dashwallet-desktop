// The lock screen and the iOS lockout policy (QT-111, QT-112, IOS-012, IOS-013): the Swift
// `LockViewModelTests`, ported by parity ID, over a fake vault that throttles as dw-vault does.

import { describe, expect, it } from "vitest";
import { LockModel, LockoutPolicy, LockText } from "./lock";
import { tryAgainIn, waitText, WRONG_PASSPHRASE } from "./model";
import { appError, FakeWorld, status, WALLET_A } from "./testWorld";

function lockedWorld(failedAttempts = 0): FakeWorld {
  const world = new FakeWorld();
  world.status = status({ state: "locked", failedAttempts, walletsWithSecrets: [WALLET_A] });
  if (failedAttempts > 0) world.lastFailureAt = world.clock;
  return world;
}

async function loaded(world: FakeWorld) {
  const model = new LockModel(world, world.now);
  await model.load();
  return model;
}

const failOnce = (model: LockModel) => model.unlock("wrong", false);

describe("lock view model", () => {
  it("IOS012_policyIsThreeFreeThenSixfoldWaitsThenDisabledAtEight", () => {
    for (const n of [0, 1, 2]) expect(LockoutPolicy.verdict(n)).toEqual({ kind: "allowed" });
    expect(LockoutPolicy.verdict(3)).toEqual({ kind: "wait", secs: 60 });
    expect(LockoutPolicy.verdict(4)).toEqual({ kind: "wait", secs: 360 });
    expect(LockoutPolicy.verdict(5)).toEqual({ kind: "wait", secs: 2_160 });
    expect(LockoutPolicy.verdict(6)).toEqual({ kind: "wait", secs: 12_960 });
    expect(LockoutPolicy.verdict(7)).toEqual({ kind: "wait", secs: 77_760 });
    expect(LockoutPolicy.verdict(8)).toEqual({ kind: "disabled" });
    expect(LockoutPolicy.verdict(20)).toEqual({ kind: "disabled" });
    expect(LockoutPolicy.attemptsRemaining(0)).toBe(8);
    expect(LockoutPolicy.attemptsRemaining(9)).toBe(0);
  });

  it("QT111_unlockSucceeds", async () => {
    const world = lockedWorld();
    const model = await loaded(world);
    expect(model.locked).toBe(true);
    expect(await model.unlock("right", false)).toBe(true);
    expect(world.unlockCalls).toEqual([{ passphrase: "right", mixingOnly: false }]);
    expect(model.lockState).toBe("unlocked");
    expect(model.locked).toBe(false);
    expect(model.message).toBeNull();
  });

  it("QT112_unlockForMixingOnly", async () => {
    const world = lockedWorld();
    const model = await loaded(world);
    await model.unlock("right", true);
    expect(world.unlockCalls[0].mixingOnly).toBe(true);
    expect(model.lockState).toBe("unlockedMixingOnly");
    expect(model.locked).toBe(true);
  });

  it("emptyPassphraseIsNotSent", async () => {
    const world = lockedWorld();
    const model = await loaded(world);
    await model.unlock("", false);
    expect(world.unlockCalls).toEqual([]);
    expect(model.message).toBe("Enter your wallet passphrase.");
  });

  it("IOS012_wrongPassphrasesCountDownThenThrottle", async () => {
    const world = lockedWorld();
    const model = await loaded(world);
    await failOnce(model);
    expect(model.failedAttempts).toBe(1);
    expect(model.message).toBe(`${WRONG_PASSPHRASE} 7 attempts remaining`);
    expect(model.retryAfterSecs).toBeNull();
    await failOnce(model);
    await failOnce(model);
    expect(model.failedAttempts).toBe(3);
    expect(model.retryAfterSecs).toBe(60);
    expect(model.message).toBe(`${WRONG_PASSPHRASE} ${tryAgainIn(60)}`);
    // While waiting the passphrase is not even tried.
    await model.unlock("right", false);
    expect(world.unlockCalls).toHaveLength(3);
    expect(model.message).toBe(tryAgainIn(60));
    world.clock += 30_000;
    expect(model.retryAfterSecs).toBe(30);
    world.clock += 31_000;
    model.refresh();
    expect(model.retryAfterSecs).toBeNull();
    expect(model.message).toBeNull();
    expect(await model.unlock("right", false)).toBe(true);
    expect(world.unlockCalls).toHaveLength(4);
    expect(model.failedAttempts).toBe(0);
  });

  it("aReadThatAnswersAfterAnAttemptIsDropped", async () => {
    // Review r1-Sol minor 4: the screen's first read used to land after the first attempt and
    // put back the count and wait from before it.
    const world = lockedWorld(2);
    let open!: () => void;
    world.statusGate = new Promise((r) => (open = r));
    const model = new LockModel(world, world.now);
    const read = model.load();
    await failOnce(model);
    expect(model.failedAttempts).toBe(3);
    open();
    await read;
    expect(model.failedAttempts).toBe(3);
    expect(model.retryAfterSecs).toBe(60);
    expect(model.message).toBe(`${WRONG_PASSPHRASE} ${tryAgainIn(60)}`);
  });

  it("IOS012_fourthFailureWaitsSixMinutes", async () => {
    const world = lockedWorld();
    const model = await loaded(world);
    for (let i = 0; i < 3; i++) await failOnce(model);
    world.clock += 61_000;
    await failOnce(model);
    expect(model.failedAttempts).toBe(4);
    expect(model.retryAfterSecs).toBe(360);
    expect(model.message).toBe(`${WRONG_PASSPHRASE} ${tryAgainIn(360)}`);
    expect(tryAgainIn(360)).toContain("6 minutes");
  });

  it("IOS012_eightFailuresDisableThePassphrase", async () => {
    const world = lockedWorld(7);
    const model = await loaded(world);
    expect(model.failedAttempts).toBe(7);
    expect(model.retryAfterSecs).toBe(77_760);
    expect(model.message).toBe(tryAgainIn(77_760));
    expect(model.message).toContain("22 hours");
    world.clock += 77_761_000;
    await failOnce(model);
    expect(model.disabled).toBe(true);
    expect(model.message).toBe(LockText.disabled);
    await model.unlock("right", false);
    expect(world.unlockCalls).toHaveLength(1);
  });

  it("IOS012_failureCountSurvivesARestart", async () => {
    const model = await loaded(lockedWorld(3));
    expect(model.failedAttempts).toBe(3);
    expect(model.retryAfterSecs).toBe(60);
    const disabled = await loaded(lockedWorld(8));
    expect(disabled.disabled).toBe(true);
    expect(disabled.message).toBe(LockText.disabled);
  });

  it("IOS012_aWaitThatRanOutWhileClosedIsNotShown", async () => {
    const world = lockedWorld(3);
    world.clock += 120_000;
    const model = await loaded(world);
    expect(model.retryAfterSecs).toBeNull();
    expect(model.message).toBeNull();
  });

  it("engineThrottleWinsWhenLonger", async () => {
    const world = lockedWorld();
    world.unlockErrors = [appError("vault.throttled", { kind: "retryAfter", secs: 600 })];
    const model = await loaded(world);
    await model.unlock("x", false);
    expect(model.failedAttempts).toBe(0);
    expect(model.retryAfterSecs).toBe(600);
    expect(model.message).toBe(tryAgainIn(600));
  });

  it("IOS013_offersForgotPassphraseOnlyWhenTheVaultHoldsSecrets", async () => {
    expect((await loaded(lockedWorld())).canRecover).toBe(true);
    const empty = new FakeWorld();
    empty.status = status({ state: "locked" });
    expect((await loaded(empty)).canRecover).toBe(false);
  });

  it("durationText", () => {
    expect(waitText(1)).toBe("1 minute");
    expect(waitText(360)).toBe("6 minutes");
    expect(waitText(7_199)).toBe("2 hours");
    expect(waitText(12_960)).toBe("4 hours");
  });
});
