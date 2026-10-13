// The Mirror sheet's view model: what the sheet shows for a mirror snapshot, the device list and the ticks.
const test = require("node:test");
const assert = require("node:assert/strict");
const { mirrorSheet } = require("../../ui/mirror-sheet.js");

const atv = { name: "Living Room", ip: "192.168.1.20" };
const bed = { name: "Bedroom", ip: "192.168.1.21" };
const mac = { name: "Study Mac", ip: "192.168.1.22" };
const devices = [atv, bed, mac];
const t = (d, phase, extra = {}) => ({ target: d.ip, device: d.name, phase, credential: null, message: null, reason: null, since_ms: null, ...extra });

test("before mirroring, the sheet lists the devices to tick and the button names the targets", () => {
  let m = mirrorSheet({ snapshot: { targets: [] }, devices, ticked: [atv.ip] });
  assert.equal(m.mode, "pick");
  assert.deepEqual(m.rows.map((r) => [r.target, r.ticked]), [[atv.ip, true], [bed.ip, false], [mac.ip, false]]);
  assert.deepEqual(m.button, { label: "Mirror to Living Room", action: "start", disabled: false });

  m = mirrorSheet({ snapshot: { targets: [] }, devices, ticked: [atv.ip, bed.ip] });
  assert.equal(m.button.label, "Mirror to 2 devices");

  m = mirrorSheet({ snapshot: { targets: [] }, devices, ticked: [] });
  assert.equal(m.button.disabled, true);
});

test("while mirroring, a row per TV with its own state and an ×, and one Stop all", () => {
  const snapshot = { targets: [t(atv, "mirroring", { since_ms: 1000 }), t(bed, "connecting")] };
  const m = mirrorSheet({ snapshot, devices, ticked: [] });
  assert.equal(m.mode, "live");
  assert.deepEqual(
    m.rows.map((r) => [r.target, r.name, r.sub, r.tone, r.removable]),
    [[atv.ip, "Living Room", "Mirroring", "live", true], [bed.ip, "Bedroom", "Connecting…", "busy", true]],
  );
  assert.deepEqual(m.button, { label: "Stop all", action: "stop_all", disabled: false });
  assert.deepEqual(m.live, { count: 2, since_ms: 1000 });
});

test("one TV live: the button is just Stop", () => {
  const m = mirrorSheet({ snapshot: { targets: [t(atv, "connecting")] }, devices, ticked: [] });
  assert.equal(m.button.label, "Stop");
  assert.equal(m.button.action, "stop_all");
});

test("Add a device lists the TVs not already in the mirror, ticked as the user picks them", () => {
  const snapshot = { targets: [t(atv, "mirroring", { since_ms: 1 })] };
  let m = mirrorSheet({ snapshot, devices, ticked: [] });
  assert.deepEqual(m.add, { open: false, rows: [] });
  m = mirrorSheet({ snapshot, devices, ticked: [mac.ip], adding: true });
  assert.deepEqual(m.add.rows.map((r) => [r.target, r.ticked]), [[bed.ip, false], [mac.ip, true]]);
  assert.deepEqual(m.add.button, { label: "Add Study Mac", action: "add", disabled: false });
});

test("a code that couldn't be sent says why on the code sheet", () => {
  const snapshot = { targets: [t(atv, "code", { credential: "pin" })] };
  let m = mirrorSheet({ snapshot, devices, ticked: [], error: "Mirroring stopped. Try again." });
  assert.equal(m.mode, "code");
  assert.equal(m.code.error, "Mirroring stopped. Try again.");
  // A password works the same way, and without an error the TV's own message shows.
  m = mirrorSheet({ snapshot: { targets: [t(atv, "code", { credential: "password", message: "That password didn't work. Try again." })] }, devices, ticked: [] });
  assert.deepEqual([m.code.message, m.code.error], ["That password didn't work. Try again.", null]);
});

test("a TV asking for a code takes over the sheet; the code goes to that TV", () => {
  const snapshot = { targets: [t(atv, "mirroring", { since_ms: 1 }), t(bed, "code", { credential: "password" })] };
  const m = mirrorSheet({ snapshot, devices, ticked: [] });
  assert.equal(m.mode, "code");
  assert.deepEqual(m.code, { target: bed.ip, name: "Bedroom", credential: "password", message: null, error: null });
  assert.deepEqual(m.button, { label: "Cancel", action: "remove", target: bed.ip, disabled: false });
  assert.equal(m.rows.find((r) => r.target === bed.ip).sub, "Password needed");
});

test("a failed TV says why in its row, the sentence goes in a banner, and the others keep going", () => {
  const message = "Bedroom didn't answer. Check it's on and on the same network.";
  const snapshot = { targets: [t(atv, "mirroring", { since_ms: 1 }), t(bed, "error", { message, reason: "Didn't answer" })] };
  const m = mirrorSheet({ snapshot, devices, ticked: [] });
  assert.equal(m.mode, "live");
  assert.equal(m.banner, message);
  const row = m.rows.find((r) => r.target === bed.ip);
  assert.deepEqual([row.sub, row.tone, row.removable], ["Didn't answer", "bad", true]);
  assert.equal(m.live.count, 1);
});

test("after everything failed or ended, the sheet is back to picking, with the note and the failed TV ticked for a retry", () => {
  const snapshot = { targets: [t(atv, "error", { message: "Living Room didn't answer.", reason: "Didn't answer" }), t(bed, "idle", { message: "Mirroring to Bedroom ended." })] };
  const m = mirrorSheet({ snapshot, devices, ticked: [atv.ip] });
  assert.equal(m.mode, "pick");
  assert.equal(m.banner, "Living Room didn't answer.");
  assert.deepEqual(m.rows.map((r) => [r.target, r.sub, r.tone]), [[atv.ip, "Didn't answer", "bad"], [bed.ip, "Ended", "note"], [mac.ip, null, null]]);
  assert.deepEqual(m.button, { label: "Mirror to Living Room", action: "start", disabled: false });
});

test("a TV added by address that discovery doesn't list still gets its row", () => {
  const snapshot = { targets: [t({ name: "", ip: "10.0.0.9" }, "connecting", { device: null })] };
  const m = mirrorSheet({ snapshot, devices: [atv], ticked: [] });
  assert.deepEqual(m.rows.map((r) => [r.target, r.name]), [["10.0.0.9", "10.0.0.9"]]);
});

test("a failed TV can be added again from Add a device", () => {
  const snapshot = { targets: [t(atv, "mirroring", { since_ms: 1 }), t(bed, "error", { message: "x", reason: "Didn't answer" })] };
  const m = mirrorSheet({ snapshot, devices, ticked: [bed.ip], adding: true });
  assert.deepEqual(m.add.rows.map((r) => r.target), [bed.ip, mac.ip]);
  assert.equal(m.add.button.label, "Add Bedroom");
});
