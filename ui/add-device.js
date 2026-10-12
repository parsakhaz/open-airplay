// Devices added by IP address or hostname: joining them into Send to, and the Add device flow.
// The Rust side checks addresses and remembers the list (src-tauri/src/added.rs); this file only
// decides what the list shows and in what order the add steps run. Tests: tests/ui/add-device.test.js.
const AddDevice = (() => {
  // Send to rows: discovered devices first, then added ones discovery can't see.
  // A row with `added` set came from (or matches) an added device; its page offers Remove.
  function merge(discovered, added) {
    const rest = [...added];
    const take = (d) => {
      const i = rest.findIndex((a) => (a.cast_id && a.cast_id === d.identifier) || a.host === d.address);
      return i < 0 ? null : rest.splice(i, 1)[0].host;
    };
    const found = discovered.map((d) => ({ ...d, added: take(d) }));
    const unseen = rest.map((a) => ({
      identifier: a.cast_id || `added:${a.host}`,
      name: a.name || a.host,
      address: a.host,
      port: a.port,
      model: null,
      // Answered a cast scan when added: it can take casts. Otherwise it is offered for mirroring only.
      services: a.cast_id ? ["AirPlay"] : [],
      added: a.host,
      unseen: true,
    }));
    return [...found, ...unseen];
  }

  // Checks the address (Rust: format, then that something answers), asks the cast sidecar who is
  // there, and saves. Resolves to the saved list; rejects with the plain sentence to show.
  async function add(form, { invoke, scanHosts }) {
    const addr = await invoke("added_device_check", { input: form.input, port: String(form.port ?? "").trim() || null });
    let found = null;
    try {
      found = (await scanHosts(addr.host))[0] || null;
    } catch { /* not a cast device, or no sidecar: it can still be mirrored to */ }
    const name = String(form.name ?? "").trim() || (found && found.name) || null;
    return invoke("added_device_save", { device: { host: addr.host, port: addr.port, name, cast_id: (found && found.identifier) || null } });
  }

  return { merge, add };
})();

if (typeof module !== "undefined") module.exports = AddDevice;
