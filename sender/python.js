// Runs the cast sidecar's venv Python with the given arguments, from any shell:
// `node sender/python.js sender/freeze.py`. npm runs scripts in cmd.exe on Windows, which can't start
// "sender/.venv/Scripts/python" (it reads "sender" as the command), and the venv's layout differs by OS.
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const venv = path.join(__dirname, ".venv");
const python = process.platform === "win32" ? path.join(venv, "Scripts", "python.exe") : path.join(venv, "bin", "python");
const run = spawnSync(python, process.argv.slice(2), { stdio: "inherit" });
if (run.error) {
  console.error(`Couldn't start ${python}: ${run.error.message}. Create the venv first (see docs/DEVELOPING.md).`);
}
process.exit(run.status ?? 1);
