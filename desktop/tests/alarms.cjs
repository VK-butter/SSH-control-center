// Health alarms (roadmap entry 7) on the fixture (plain Chromium): the Alarms page (Active and Rules), the sidebar
// count, host dots, the host page's alarms and OS updates, and the red-alarm notice. Gates the core task, alert when
// any `web` host's disk is over 90%, at ≤ 4 decisions.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");
const { decisions, startFixture } = require("./decisions.cjs");
const artifacts = path.resolve(__dirname, "../../artifacts/alarms-ui"); fs.mkdirSync(artifacts, { recursive: true });
const MAX_DECISIONS = 4;
const rpcCalls = async (page, url) => (await (await page.request.get(url + "/calls")).json()).filter(c => c.command === "rpc");
const rpc = async (page, url, method, params = {}) => (await page.request.post(url + "/rpc", { data: { command: "rpc", args: { method, params } } })).json();

async function shots(page, name) {
  for (const width of [1180, 820, 390]) {
    await page.setViewportSize({ width, height: 900 });
    await page.screenshot({ path: path.join(artifacts, name + "-" + width + ".png"), fullPage: true });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), "no sideways scroll at " + width + " px");
  }
  await page.setViewportSize({ width: 1180, height: 900 });
}
async function alarmsPage(page, url, tab) {
  await page.goto(url);
  await page.locator('.nav[data-page="alarms"]').click();
  await page.locator("h1", { hasText: /^Alarms$/ }).waitFor();
  if (tab) await page.locator(`[data-action="alarm-tab"][data-tab="${tab}"]`).click();
}
async function openTree(page) {
  await page.locator("#dc-tree .dc-row").first().waitFor();
  for (let i = 0; i < 6; i++) {
    const closed = page.locator('#dc-tree .dc-toggle[aria-expanded="false"]');
    if (!(await closed.count())) break;
    await closed.first().click();
  }
}

const checks = {
  /** The core task: every `web` host, disk over 90%, red. */
  async "core-task"(page, url, d) {
    await rpc(page, url, "tags.set", { targetId: "ui-test", tags: ["web"] });
    await alarmsPage(page, url, "rules");
    const rules = page.locator("tbody tr[data-rule]");
    assert.equal(await rules.count(), 3, "three rules from the start");
    await page.locator('[data-action="alarm-rule-new"]').click();
    const form = page.locator("#alarm-rule");
    await d.select(form.locator('[name="scope"]'), "tag", "hosts with a Tag");
    await d.fill(page.locator('#alarm-rule [name="tag"]'), "web", "Tag web");
    assert.equal(await form.locator('[name="condition"]').inputValue(), "disk", "disk is the first condition");
    assert.equal(await form.locator('[name="threshold"]').inputValue(), "90", "90 is pre-filled");
    assert.equal(await form.locator('[name="level"]').inputValue(), "red");
    await form.locator('[type="submit"]').click();
    await page.locator("#notice", { hasText: "Rule saved" }).waitFor();
    const sent = (await rpcCalls(page, url)).filter(c => c.method === "alarms.rule_set").map(c => c.params);
    assert.deepEqual(sent.map(({ id, ...rule }) => rule), [{ scope: { tag: "web" }, condition: "disk", threshold: 90, level: "red", containers: [] }]);
    await page.locator("tbody tr[data-rule]", { hasText: "Tag web" }).waitFor();
    await shots(page, "rules");
  },
  /** Chosen hosts, a container rule with names, editing, deleting, the interval. */
  async "rule-editor"(page, url) {
    await alarmsPage(page, url, "rules");
    await page.locator('[data-action="alarm-rule-new"]').click();
    await page.locator('#alarm-rule [name="scope"]').selectOption("hosts");
    await page.locator('#alarm-rule [name="host"][value="ui-test"]').check();
    await page.locator('#alarm-rule [name="condition"]').selectOption("container");
    assert.equal(await page.locator('#alarm-rule [name="threshold"]').count(), 0, "no percent for containers");
    await page.locator('#alarm-rule [name="containers"]').fill("api, worker");
    await page.locator('#alarm-rule [name="level"]').selectOption("yellow");
    await page.locator('#alarm-rule [type="submit"]').click();
    const row = page.locator("tbody tr[data-rule]", { hasText: "Container stopped · api, worker" });
    await row.waitFor();
    assert.match(await row.textContent(), /1 host · UI test target/);
    await row.locator('[data-action="alarm-rule-edit"]').click();
    await page.locator('#alarm-rule [name="level"]').selectOption("red");
    await page.locator('#alarm-rule [type="submit"]').click();
    await page.locator("tbody tr[data-rule]", { hasText: "Container stopped" }).locator(".alarm-level", { hasText: "Red" }).waitFor();
    page.once("dialog", dialog => dialog.accept());
    await page.locator("tbody tr[data-rule]", { hasText: "Container stopped" }).locator('[data-action="alarm-rule-delete"]').click();
    await page.locator("#notice", { hasText: "Rule deleted" }).waitFor();
    assert.equal(await page.locator("tbody tr[data-rule]").count(), 3);
    // A bad percentage is stopped in the field (1–99), before anything is sent.
    await page.locator('[data-action="alarm-rule-new"]').click();
    const before = (await rpcCalls(page, url)).filter(c => c.method === "alarms.rule_set").length;
    await page.locator('#alarm-rule [name="threshold"]').fill("120");
    await page.locator('#alarm-rule [type="submit"]').click();
    assert.ok(await page.locator('#alarm-rule [name="threshold"]').evaluate(el => !el.checkValidity()));
    assert.equal((await rpcCalls(page, url)).filter(c => c.method === "alarms.rule_set").length, before);
    await page.locator('[data-action="alarm-rule-cancel"]').click();
    await page.locator('#alarm-interval [name="interval"]').selectOption("15");
    await page.locator("#notice", { hasText: "checked every 15 minutes" }).waitFor();
    assert.equal((await rpc(page, url, "alarms.rules")).interval, 15);
  },
  /** Active alarms: the count, red first, Acknowledge, and the host dots. */
  async "active-alarms"(page, url) {
    await alarmsPage(page, url);
    const count = page.locator("#alarm-count");
    await page.waitForFunction(() => document.querySelector("#alarm-count")?.textContent === "3");
    assert.match(await count.getAttribute("class"), /is-red/);
    const rows = page.locator("#alarm-list tbody tr");
    assert.equal(await rows.count(), 3);
    assert.match(await rows.first().textContent(), /Red/);
    assert.match(await page.locator("#alarm-list").textContent(), /Disk 93\.5%/);
    await shots(page, "active");
    // Acknowledge both red alarms: the count turns yellow.
    for (let i = 0; i < 2; i++) await page.locator("#alarm-list tr:not(.acknowledged) .alarm-level.is-red").first().locator("xpath=ancestor::tr").locator('[data-action="alarm-ack"]').click();
    await page.waitForFunction(() => document.querySelector("#alarm-count")?.textContent === "1");
    assert.match(await count.getAttribute("class"), /is-yellow/);
    assert.equal(await page.locator("#alarm-list tr.acknowledged").count(), 2, "still listed, greyed");
    // Host dots: ui-test has an unacknowledged yellow left; hq-master's red is acknowledged.
    await page.locator('.nav[data-page="datacenter"]').click();
    await openTree(page);
    await page.locator('#dc-tree .dc-item[data-node="node:ui-test"] .dot.alarm-yellow').waitFor();
    assert.equal(await page.locator('#dc-tree .dc-item[data-node="node:hq-master"] .dot.alarm-red').count(), 0);
  },
  /** The host page: its alarms, its OS updates with packages and Check now, and a rule for just this host. */
  async "host-page"(page, url, d) {
    await page.goto(url);
    await openTree(page);
    await page.locator('#dc-tree .dc-item[data-node="node:ui-test"]').click();
    const section = page.locator("#host-alarms");
    await section.locator("#updates-badge", { hasText: "3 updates pending" }).waitFor();
    assert.match(await section.textContent(), /Disk 93\.5%/);
    await section.locator("summary", { hasText: "Packages" }).click();
    assert.equal(await section.locator(".package-list li").count(), 3);
    await shots(page, "host-page");
    await section.locator('[data-action="check-updates"]').click();
    await page.locator("#updates-badge", { hasText: "1 update pending" }).waitFor({ timeout: 15000 });
    // A rule for just this host: the host is already chosen.
    await page.locator('[data-action="alarm-for-host"]').click();
    await page.locator("#alarm-rule").waitFor();
    assert.equal(await page.locator('#alarm-rule [name="scope"]').inputValue(), "hosts");
    assert.ok(await page.locator('#alarm-rule [name="host"][value="ui-test"]').isChecked());
    await page.locator('#alarm-rule [type="submit"]').click();
    await page.locator("tbody tr[data-rule]", { hasText: "1 host · UI test target" }).waitFor();
  },
  /** A red alarm that appears while the window is open shows the notice bar. */
  async "new-red-notice"(page, url) {
    await page.clock.install();
    await page.goto(url);
    await page.locator("#dc-tree .dc-row").first().waitFor();
    await page.clock.runFor(61000);
    await page.locator("#notice", { hasText: "Red alarm · UI test target · Not answering" }).waitFor({ timeout: 15000 });
    await page.waitForFunction(() => document.querySelector("#alarm-count")?.textContent === "1");
  },
};
const scenarios = { "core-task": [], "rule-editor": [], "active-alarms": ["masters-page", "alarms-firing"], "host-page": ["alarms-firing"], "new-red-notice": ["alarm-new-red"] };

(async () => {
  const browser = await chromium.launch({ headless: true }); const results = [];
  try {
    for (const [name, flags] of Object.entries(scenarios)) {
      const fixture = await startFixture(flags);
      try {
        const page = await browser.newPage({ viewport: { width: 1180, height: 900 }, locale: "en-US" });
        const d = decisions();
        await checks[name](page, fixture.url, d);
        if (d.count) {
          console.log("[" + name + "] decisions: " + d.count + "\n" + d.summary());
          assert.ok(d.count <= MAX_DECISIONS, name + ": " + d.count + " decisions, target ≤ " + MAX_DECISIONS);
        }
        console.log("ok " + name);
        results.push({ scenario: name, decisions: d.count, log: d.log });
        await page.close();
      } finally { fixture.stop(); }
    }
    fs.writeFileSync(path.join(artifacts, "alarms-results.json"), JSON.stringify({ ok: true, gate: MAX_DECISIONS, results }, null, 2) + "\n");
    console.log(JSON.stringify({ ok: true }));
  } finally { await browser.close(); }
})().catch(e => { console.error(e); process.exitCode = 1; });
