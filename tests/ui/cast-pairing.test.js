// The cast code sheet's decisions: a cast to a device that must be paired opens the sheet instead of
// a dead-end error, a wrong code asks again, and a successful pairing retries the cast.
const test = require("node:test");
const assert = require("node:assert/strict");
const { protocolFor, afterCastError, afterCode, codeSheet } = require("../../ui/cast-pairing.js");

const needsPairing = { event: "error", code: "needs_pairing", message: "Den TV needs to be paired with this PC first." };

test("a cast that needs pairing opens the code sheet, not an error", () => {
  assert.deepEqual(afterCastError(needsPairing, { paired: false }), { pair: true, message: "" });
});

test("other cast failures stay errors with their own sentence", () => {
  const msg = { event: "error", code: "unreachable", message: "Den TV didn't answer. Check it's on and on the same network." };
  assert.deepEqual(afterCastError(msg, {}), { pair: false, message: msg.message });
  assert.equal(afterCastError({ event: "error" }, {}).message, "Couldn't cast. Try again.");
});

test("needing pairing again right after pairing asks again and says it didn't work", () => {
  assert.deepEqual(afterCastError(needsPairing, { paired: true }), { pair: true, message: "That didn't work. Try again." });
});

test("the protocol paired is the one the cast uses", () => {
  assert.equal(protocolFor(false), "AirPlay");
  assert.equal(protocolFor(true), "RAOP");
});

test("after a code: paired retries the cast, a wrong code asks again, anything else is an error", () => {
  assert.deepEqual(afterCode({ event: "paired", identifier: "tv" }), { next: "retry" });
  const wrong = { event: "error", code: "wrong_code", message: "That code didn't work. Enter the new code shown on Den TV." };
  assert.deepEqual(afterCode(wrong), { next: "code", message: wrong.message });
  assert.deepEqual(afterCode({ event: "error", code: "not_pairing", message: "That code has expired. Try casting again." }),
    { next: "error", message: "That code has expired. Try casting again." });
});

test("the sheet asks for the code shown on the device, or for its password", () => {
  const pin = codeSheet({ state: "code", name: "Den TV", credential: "pin", codeRound: 1 });
  assert.equal(pin.title, "Enter the code shown on Den TV");
  assert.equal(pin.password, false);
  assert.equal(pin.message, "");
  const pw = codeSheet({ state: "code", name: "Den TV", credential: "password", codeRound: 1 });
  assert.equal(pw.title, "Den TV needs its password");
  assert.equal(pw.password, true);
  const waiting = codeSheet({ state: "pairing", name: "Den TV" });
  assert.equal(waiting.waiting, true);
  assert.equal(waiting.key, null);
});

test("a wrong code gives a new prompt, so the boxes start empty again", () => {
  const c = { state: "code", name: "Den TV", credential: "pin", codeRound: 1 };
  const first = codeSheet(c);
  const again = codeSheet({ ...c, codeRound: 2, codeMessage: "That code didn't work. Enter the new code shown on Den TV." });
  const twice = codeSheet({ ...c, codeRound: 3, codeMessage: "That code didn't work. Enter the new code shown on Den TV." });
  assert.notEqual(first.key, again.key);
  assert.notEqual(again.key, twice.key);
  assert.equal(again.message, "That code didn't work. Enter the new code shown on Den TV.");
  assert.equal(codeSheet({ ...c, codeBusy: true }).busy, true);
});
