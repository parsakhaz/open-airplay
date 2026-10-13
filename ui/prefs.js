// The app's preferences as the panel uses them (saved by the Rust side; read with get_prefs).
// Two ways to read them: at start a failure keeps the defaults so the panel still opens; after a
// change (a device added or removed by address) a failure is reported, so the user isn't shown a
// list that silently didn't update. Pure apart from `invoke`, so it runs under `node --test`.
(function (root) {
  const RELOAD_FAILED = "The device list didn't update. Go back to see it.";

  function createPrefs(invoke) {
    const values = { startAtLogin: false, addedDevices: [] };
    const apply = (p) => {
      values.startAtLogin = !!p.start_at_login;
      values.addedDevices = Array.isArray(p.added_devices) ? p.added_devices : [];
    };
    return {
      values,
      /** At start: a failure keeps the defaults. */
      async load() {
        try { apply(await invoke("get_prefs")); } catch { /* keep defaults */ }
      },
      /** After a change: rejects with a plain sentence when the prefs can't be read. */
      async reload() {
        let p;
        try { p = await invoke("get_prefs"); } catch { throw new Error(RELOAD_FAILED); }
        apply(p);
      },
    };
  }

  /** The refresh the panel runs after adding or removing a device: a strict reload, then a redraw.
   *  A failed reload rejects (and draws nothing), so the caller shows it. */
  const refresher = (store, render) => async () => {
    await store.reload();
    render();
  };

  const api = { createPrefs, refresher, RELOAD_FAILED };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.OapPrefs = api;
})(this);
