// Pairing a cast device that asks for it: the decisions behind the code sheet on a device page.
// A cast that fails with "needs_pairing" opens the sheet (not a dead-end error); the device shows a
// code (four digit boxes) or has a password (a password field); once paired, the cast is retried.
// Pure: no DOM, no Tauri, so it runs under `node --test`. cast.js renders what it returns.
(function (root) {
  /** The protocol a cast uses, so the one that failed is the one paired: speakers get the audio stream. */
  const protocolFor = (speaker) => (speaker ? "RAOP" : "AirPlay");

  /** What a failed cast leads to. `pair`: open the code sheet (with `message` when a cast right after
   *  pairing still failed); otherwise the error banner shows `message`. */
  function afterCastError(msg, c) {
    if (!msg || msg.code !== "needs_pairing") return { pair: false, message: (msg && msg.message) || "Couldn't cast. Try again." };
    return { pair: true, message: c && c.paired ? "That didn't work. Try again." : "" };
  }

  /** What the reply to a typed code leads to: "retry" the cast, ask for the "code" again, or an "error". */
  function afterCode(msg) {
    // Paired but not saved (the shell couldn't write it): the cast goes on, and the user learns why
    // the device will ask again next time.
    if (msg.event === "paired") {
      return msg.saved === false
        ? { next: "retry", notice: "Paired, but this PC couldn't save the pairing. It works until open-airplay closes; then the device asks for the code again." }
        : { next: "retry" };
    }
    if (msg.code === "wrong_code") return { next: "code", message: msg.message || "That code didn't work. Try again." };
    return { next: "error", message: msg.message || "Couldn't pair. Try again." };
  }

  /** What the sheet shows for a device's cast state ("pairing": asking the device, "code": waiting for
   *  the user). `key` changes when the boxes should start empty again (a new prompt or a wrong code). */
  function codeSheet(c) {
    const name = c.name || "the device";
    if (c.state === "pairing") {
      return { waiting: true, password: false, title: `Pairing with ${name}…`, help: "Connecting…", message: "", key: null };
    }
    const password = c.credential === "password";
    return {
      waiting: false,
      password,
      title: password ? `${name} needs its password` : `Enter the code shown on ${name}`,
      help: password ? "The one set for AirPlay on the device." : "You only need it the first time. It continues by itself after the last digit.",
      message: c.codeMessage || "",
      busy: !!c.codeBusy,
      key: `${c.credential}|${c.codeMessage || ""}|${c.codeRound || 0}`,
    };
  }

  const api = { protocolFor, afterCastError, afterCode, codeSheet };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.CastPairing = api;
})(this);
