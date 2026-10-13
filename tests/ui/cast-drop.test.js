// Files dropped on the panel: which device, if any, they are cast to.
const test = require("node:test");
const assert = require("node:assert/strict");
const { castDropTarget } = require("../../ui/cast.js");

const tvA = { identifier: "tv-a", name: "Living Room" };

test("a drop on the open device page casts to that device", () => {
  assert.equal(castDropTarget(tvA, true, false), tvA);
});

test("a drop after leaving the device page (home, Settings, the Mirror sheet) goes nowhere", () => {
  assert.equal(castDropTarget(tvA, false, false), null);
});

test("a drop while that device is already casting, or with no device page, goes nowhere", () => {
  assert.equal(castDropTarget(tvA, true, true), null);
  assert.equal(castDropTarget(null, true, false), null);
});

test("in the panel the cast part is still on window", () => {
  const vm = require("node:vm");
  const fs = require("node:fs");
  const noop = () => Promise.resolve();
  const sandbox = { window: { __TAURI__: { core: { invoke: noop }, event: { listen: noop } } } };
  vm.runInNewContext(fs.readFileSync(require.resolve("../../ui/cast.js"), "utf8"), sandbox);
  assert.equal(typeof sandbox.window.OapCast?.show, "function");
});
