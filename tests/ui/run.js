// Runs every UI test file: `npm run test:ui`. It lists the files itself because neither cmd.exe nor
// Node 20's --test expands "tests/ui/*.test.js".
const { readdirSync } = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const files = readdirSync(__dirname).filter((f) => f.endsWith(".test.js")).map((f) => path.join(__dirname, f));
const run = spawnSync(process.execPath, ["--test", ...files], { stdio: "inherit" });
process.exit(run.status ?? 1);
