// The Mirror sheet's view model: what the sheet shows, given the mirror snapshot ("mirror-status":
// one entry per TV, in the order they were added), the TVs that can take a mirror, and the ones the
// user ticked. Pure: no DOM, no Tauri, so it drops into any renderer and runs under `node --test`.
//
// Modes: "pick" (tick TVs, then Mirror to …), "live" (a row per TV with an ×, Add a device, Stop all),
// "code" (a TV shows a code or has a password; the sheet asks for it).
(function (root) {
  const SENDING = new Set(["connecting", "code", "mirroring"]);

  // A row's state in a few words; the full sentence for a failure is the banner.
  function rowState(s) {
    switch (s.phase) {
      case "mirroring": return { sub: "Mirroring", tone: "live" };
      case "connecting": return { sub: "Connecting…", tone: "busy" };
      case "code": return { sub: s.credential === "password" ? "Password needed" : "Enter the code", tone: "busy" };
      case "error": return { sub: s.reason || "Couldn't mirror", tone: "bad" };
      default: return s.message ? { sub: "Ended", tone: "note" } : { sub: null, tone: null };
    }
  }

  // `error`: the last command that failed (a code that couldn't be sent), in plain words.
  function mirrorSheet({ snapshot, devices, ticked, adding, error }) {
    const targets = (snapshot && snapshot.targets) || [];
    const list = devices || [];
    const tickedSet = new Set(ticked || []);
    const nameOf = (s) => s.device || (list.find((d) => d.ip === s.target) || {}).name || s.target;
    const byTarget = new Map(targets.map((s) => [s.target, s]));
    const sending = targets.filter((s) => SENDING.has(s.phase));
    const failed = targets.find((s) => s.phase === "error");
    const banner = failed ? failed.message : (targets.find((s) => s.message && !SENDING.has(s.phase)) || {}).message || null;

    const pickRows = (from) => from.map((d) => {
      const s = byTarget.get(d.ip);
      return { target: d.ip, name: s ? nameOf(s) : d.name, ticked: tickedSet.has(d.ip), ...(s ? rowState(s) : { sub: null, tone: null }) };
    });
    const startButton = (rows, action, verb) => {
      const chosen = rows.filter((r) => r.ticked);
      const label = chosen.length === 1 ? `${verb} ${chosen[0].name}` : `${verb} ${chosen.length} devices`;
      return { label, action, disabled: chosen.length === 0 };
    };

    if (sending.length === 0) {
      // TVs the sheet knows only from the snapshot (added by address, then failed) keep their row.
      const known = new Set(list.map((d) => d.ip));
      const extra = targets.filter((s) => !known.has(s.target)).map((s) => ({ ip: s.target, name: nameOf(s) }));
      const rows = pickRows([...list, ...extra]);
      return { mode: "pick", banner, rows, button: startButton(rows, "start", "Mirror to") };
    }

    const rows = targets.map((s) => ({
      target: s.target,
      name: nameOf(s),
      removable: true,
      ...rowState(s),
    }));
    const mirroring = targets.filter((s) => s.phase === "mirroring" && s.since_ms != null);
    const live = { count: sending.length, since_ms: mirroring.length ? Math.min(...mirroring.map((s) => s.since_ms)) : null };

    const asking = targets.find((s) => s.phase === "code");
    if (asking) {
      return {
        mode: "code",
        banner,
        rows,
        live,
        // A code that couldn't even be sent says so here, where the user is looking, not in a hidden banner.
        code: { target: asking.target, name: nameOf(asking), credential: asking.credential || "pin", message: asking.message || null, error: error || null },
        button: { label: "Cancel", action: "remove", target: asking.target, disabled: false },
      };
    }

    const addRows = adding ? pickRows(list.filter((d) => !(byTarget.has(d.ip) && SENDING.has(byTarget.get(d.ip).phase)))) : [];
    const add = adding ? { open: true, rows: addRows, button: startButton(addRows, "add", "Add") } : { open: false, rows: [] };
    return {
      mode: "live",
      banner,
      rows,
      live,
      add,
      // TVs getting video but not this PC's sound (it couldn't be captured or its stream failed).
      noSound: targets.filter((s) => s.phase === "mirroring" && s.no_sound).map(nameOf),
      button: { label: sending.length > 1 ? "Stop all" : "Stop", action: "stop_all", disabled: false },
    };
  }

  /** Orders the first mirror status (asked for once) against the ones the shell pushes: a reply
   *  that arrives after a push is older than it, so it is dropped. Pushes are always newer. */
  function snapshotGate() {
    let pushes = 0;
    return {
      pushed() { pushes++; },
      /** Call when asking; the returned check says whether the reply is still the newest. */
      asking() { const at = pushes; return () => pushes === at; },
    };
  }

  const api = { mirrorSheet, snapshotGate };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.MirrorSheet = api;
})(this);
