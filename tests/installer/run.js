// Runs the uninstaller tests (tests/installer/*.tests.ps1): `npm run test:installer`. They are Windows
// PowerShell scripts, so elsewhere they are skipped.
const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

if (process.platform !== "win32") {
  console.log("installer tests: skipped (Windows only)");
  process.exit(0);
}
let failed = false;
for (const file of fs.readdirSync(__dirname).filter((f) => f.endsWith(".tests.ps1"))) {
  console.log(`# ${file}`);
  const run = spawnSync("powershell", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", path.join(__dirname, file)], { stdio: "inherit" });
  if (run.status !== 0) failed = true;
}
process.exit(failed ? 1 : 0);
