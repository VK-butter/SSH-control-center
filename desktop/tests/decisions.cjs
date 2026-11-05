// Decision counter for "How to measure easy": every fill or choice the user must make on a field that was not
// pre-filled counts once. Clicking a button to move on, or leaving a pre-filled value as it is, does not count.
// Use these helpers instead of the raw Playwright calls for anything the user would decide.
function decisions() {
  const log = [];
  const record = (kind, what, value) => { log.push({ kind, what, value }); };
  return {
    log,
    get count() { return log.length; },
    /** Type into a field. Counts only when the field was empty (not pre-filled). */
    async fill(locator, value, what) { if (!(await locator.inputValue())) record("fill", what, value); await locator.fill(value); },
    /** Pick an option in a select. Counts only when it changes the pre-selected value. */
    async select(locator, value, what) { if ((await locator.inputValue()) !== value) record("select", what, value); await locator.selectOption(value); },
    /** A choice between alternatives with no default (menu item, radio, one of several buttons). Always counts. */
    async choose(locator, what) { record("choose", what); await locator.click(); },
    /** One line per decision, for the console. */
    summary() { return log.map((d, i) => (i + 1) + ". " + d.kind + " " + d.what + (d.value ? " = " + d.value : "")).join("\n"); },
  };
}
/** Starts desktop/tests/fixture.cjs on a free port with the given scenario flags; resolves {url, stop}. */
function startFixture(flags = [], env = {}) {
  const { spawn } = require("node:child_process"), path = require("node:path");
  const server = spawn(process.execPath, [path.join(__dirname, "fixture.cjs")], { env: { ...process.env, SSHCC_UI_TEST_PORT: "0", SSHCC_FIXTURE_SCENARIO: flags.join(","), ...env } });
  return new Promise((resolve, reject) => {
    server.stdout.on("data", data => { const m = String(data).match(/http:\/\/127\.0\.0\.1:\d+/); if (m) resolve({ url: m[0], stop: () => server.kill() }); });
    server.once("error", reject); server.once("exit", code => reject(new Error("fixture exited " + code)));
  });
}
module.exports = { decisions, startFixture };
