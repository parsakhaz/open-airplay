// Advanced in the Mirror sheet: how its choices are saved.
const test = require("node:test");
const assert = require("node:assert/strict");
const { createSaver } = require("../../ui/mirror-advanced.js");

/** A set_mirror_options stand-in whose replies the test releases one by one. */
function slowSend() {
  const sent = [];
  const replies = [];
  const send = (options) => new Promise((resolve, reject) => { sent.push(options); replies.push({ resolve, reject }); });
  return { sent, replies, send };
}
const tick = () => new Promise((r) => setImmediate(r));

test("choices made while a save is on its way are all kept, and only the newest options are sent", async () => {
  const s = slowSend();
  const results = [];
  const save = createSaver(s.send, (err) => results.push(err));
  const options = {};
  options.fps = 60; save(options);
  options.bitrate_kbps = 8000; save(options);  // before the first reply
  options.audio = false; save(options);
  assert.deepEqual(s.sent, [{ fps: 60 }], "one save at a time");
  s.replies[0].resolve({ mirror: { fps: 60 } }); // the older reply must not undo Quality
  await tick();
  assert.deepEqual(s.sent[1], { fps: 60, bitrate_kbps: 8000, audio: false });
  s.replies[1].resolve({});
  await tick();
  assert.equal(s.sent.length, 2);
  assert.deepEqual(results, [null]);
});

test("a save that fails is reported, and the next one can succeed", async () => {
  const s = slowSend();
  const results = [];
  const save = createSaver(s.send, (err) => results.push(err));
  save({ fps: 30 });
  s.replies[0].reject("disk full");
  await tick();
  assert.deepEqual(results, ["disk full"]);
  save({ fps: 60 });
  s.replies[1].resolve({});
  await tick();
  assert.deepEqual(results, ["disk full", null]);
});
