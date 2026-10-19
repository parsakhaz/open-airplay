// Runs the uninstaller cleanup tests: `npm run test:installer`. They are Windows PowerShell scripts, so
// elsewhere they are skipped.
const path = require("node:path");
const { spawnSync } = require("node:child_process");

if (process.platform !== "win32") {
  console.log("installer tests: skipped (Windows only)");
  process.exit(0);
}
const tests = path.join(__dirname, "forget-credentials.tests.ps1");
const run = spawnSync("powershell", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", tests], { stdio: "inherit" });
process.exit(run.status ?? 1);
