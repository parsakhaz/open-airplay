// The panel's preferences: lenient at start, strict after a change, through the same refresh the
// panel passes to Add device.
const test = require("node:test");
const assert = require("node:assert/strict");
const { createPrefs, refresher, RELOAD_FAILED } = require("../../ui/prefs.js");
const AddDevice = require("../../ui/add-device.js");

/** get_prefs (and added_device_remove) stand-ins; `prefsFail` makes get_prefs reject. */
function shell({ prefsFail = false } = {}) {
  const calls = [];
  const invoke = async (cmd, args) => {
    calls.push(cmd);
    if (cmd === "get_prefs") {
      if (prefsFail) throw "config folder unreadable";
      return { start_at_login: true, added_devices: [] };
    }
    if (cmd === "added_device_remove") return { added_devices: [] };
    throw new Error(`unexpected ${cmd}`);
  };
  return { invoke, calls };
}

test("a remove whose list reload fails says so instead of going back with a stale list", async () => {
  const s = shell({ prefsFail: true });
  const store = createPrefs(s.invoke);
  store.values.addedDevices = [{ host: "192.168.1.20" }];
  let drawn = 0;
  const refresh = refresher(store, () => drawn++); // what app.js passes as host.refresh
  const problem = await AddDevice.remove("192.168.1.20", { invoke: s.invoke, refresh });
  assert.equal(problem, "Removed, but the list didn't update. Go back to see it.");
  assert.deepEqual(s.calls, ["added_device_remove", "get_prefs"]);
  assert.equal(drawn, 0, "nothing is redrawn from the stale list");
});

test("a remove whose reload works redraws the new list and reports no problem", async () => {
  const s = shell();
  const store = createPrefs(s.invoke);
  store.values.addedDevices = [{ host: "192.168.1.20" }];
  let drawn = 0;
  const problem = await AddDevice.remove("192.168.1.20", { invoke: s.invoke, refresh: refresher(store, () => drawn++) });
  assert.equal(problem, null);
  assert.deepEqual(store.values.addedDevices, []);
  assert.equal(drawn, 1);
});

test("at start, prefs that can't be read keep the defaults; after a change they reject", async () => {
  const store = createPrefs(shell({ prefsFail: true }).invoke);
  await store.load();
  assert.deepEqual(store.values, { startAtLogin: false, addedDevices: [] });
  await assert.rejects(store.reload(), { message: RELOAD_FAILED });
});
