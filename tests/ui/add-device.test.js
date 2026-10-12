// Run: node --test tests/ui/*.test.js
const test = require("node:test");
const assert = require("node:assert/strict");
const AddDevice = require("../../ui/add-device.js");

const den = { identifier: "AA", name: "Den", address: "192.168.1.20", model: "Apple TV 4K", services: ["AirPlay"] };
const office = { identifier: "BB", name: "Office", address: "192.168.1.30", model: null, services: ["RAOP"] };

test("an added device that discovery also found shows once, marked as added", () => {
  const rows = AddDevice.merge([den, office], [{ host: "192.168.1.20", port: null, name: "Den", cast_id: "AA" }]);
  assert.deepEqual(rows.map((r) => [r.name, r.added]), [["Den", "192.168.1.20"], ["Office", null]]);
});

test("a match on the cast identifier counts even when the address changed", () => {
  const rows = AddDevice.merge([{ ...den, address: "192.168.1.99" }], [{ host: "den.local", port: null, name: null, cast_id: "AA" }]);
  assert.equal(rows.length, 1);
  assert.equal(rows[0].added, "den.local");
});

test("an added device discovery can't see is listed after the found ones, by its name or address", () => {
  const rows = AddDevice.merge([den], [
    { host: "192.168.1.40", port: 7100, name: "Garage TV", cast_id: "CC" },
    { host: "tv.example", port: null, name: null, cast_id: null },
  ]);
  assert.deepEqual(rows.map((r) => r.name), ["Den", "Garage TV", "tv.example"]);
  assert.deepEqual(rows[1], { identifier: "CC", name: "Garage TV", address: "192.168.1.40", port: 7100, model: null, services: ["AirPlay"], added: "192.168.1.40", unseen: true });
  assert.equal(rows[2].identifier, "added:tv.example", "rows are keyed even without a cast identifier");
  assert.deepEqual(rows[2].services, [], "never answered a cast scan: mirror only");
});

function fakes({ check, scan, saved = [] } = {}) {
  const calls = [];
  return {
    calls,
    invoke: async (cmd, args) => {
      calls.push([cmd, args]);
      if (cmd === "added_device_check") {
        if (check instanceof Error) throw check.message;
        return check || { host: args.input.trim(), port: args.port };
      }
      if (cmd === "added_device_save") return [...saved, args.device];
      throw new Error(`unexpected ${cmd}`);
    },
    scanHosts: async (host) => {
      calls.push(["scan", host]);
      if (scan instanceof Error) throw scan;
      return scan || [];
    },
  };
}

test("adding checks the address, learns the name and identifier from a cast scan, and saves", async () => {
  const f = fakes({ check: { host: "192.168.1.40", port: null }, scan: [{ ...den, identifier: "CC", name: "Garage TV" }] });
  const list = await AddDevice.add({ input: " 192.168.1.40 ", name: "", port: "" }, f);
  assert.deepEqual(f.calls[0], ["added_device_check", { input: " 192.168.1.40 ", port: null }]);
  assert.deepEqual(f.calls[1], ["scan", "192.168.1.40"]);
  assert.deepEqual(list, [{ host: "192.168.1.40", port: null, name: "Garage TV", cast_id: "CC" }]);
});

test("the name typed in the form wins over the device's own", async () => {
  const f = fakes({ scan: [{ ...den, name: "Living Room" }] });
  const [saved] = await AddDevice.add({ input: "192.168.1.20", name: " Den TV ", port: "7100" }, f);
  assert.equal(saved.name, "Den TV");
  assert.equal(f.calls[0][1].port, "7100", "Rust checks the port field, so its messages stay in one place");
});

test("a device that doesn't answer a cast scan is still saved, for mirroring", async () => {
  const f = fakes({ scan: new Error("Couldn't search the network.") });
  const [saved] = await AddDevice.add({ input: "tv.example", name: "", port: "" }, f);
  assert.deepEqual(saved, { host: "tv.example", port: null, name: null, cast_id: null });
});

test("a refused address is reported as the plain sentence and nothing is saved", async () => {
  const f = fakes({ check: new Error("192.168.1.300 isn't a valid IP address.") });
  await assert.rejects(AddDevice.add({ input: "192.168.1.300", name: "", port: "" }, f), (e) => e === "192.168.1.300 isn't a valid IP address.");
  assert.deepEqual(f.calls.map((c) => c[0]), ["added_device_check"]);
});

test("Remove shows only on the page of a device that was added by address", () => {
  assert.equal(AddDevice.removeHtml({ ...den, added: null }), "");
  assert.equal(AddDevice.removeHtml(null), "");
  const html = AddDevice.removeHtml({ ...den, added: "192.168.1.20" });
  assert.match(html, />Remove from this list</);
  assert.match(html, /data-host="192\.168\.1\.20"/);
  assert.match(AddDevice.removeHtml({ added: `a"><b>` }), /data-host="a&quot;&gt;&lt;b&gt;"/, "escaped");
});

test("a scan that answers with other devices too only names the one at that address", async () => {
  const f = fakes({ check: { host: "192.168.1.30", port: null }, scan: [den, office] });
  const [saved] = await AddDevice.add({ input: "192.168.1.30", name: "", port: "" }, f);
  assert.deepEqual([saved.name, saved.cast_id], ["Office", "BB"]);
  const g = fakes({ check: { host: "192.168.1.99", port: null }, scan: [den, office] });
  const [none] = await AddDevice.add({ input: "192.168.1.99", name: "", port: "" }, g);
  assert.deepEqual([none.name, none.cast_id], [null, null], "several answers and none at that address: none of them");
});
