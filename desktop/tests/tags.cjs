// Tags, filter and group run (roadmap entry 4) on the fixture (plain Chromium). Gates the core task, tag 3 hosts `web` and
// run one command on all three, at ≤ 5 decisions. Also checks the filter (folders and Tags, merged hosts included).
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");
const { decisions, startFixture } = require("./decisions.cjs");
const artifacts = path.resolve(__dirname, "../../artifacts/tags-ui"); fs.mkdirSync(artifacts, { recursive: true });
const MAX_DECISIONS = 5;
const calls = async (page, url) => (await (await page.request.get(url + "/calls")).json());
/** A request straight to the fixture, as the page would send it (to set up state). */
const rpc = async (page, url, method, params = {}) => (await page.request.post(url + "/rpc", { data: { command: "rpc", args: { method, params } } })).json();
const treeHosts = page => page.locator("#dc-tree .dc-item[data-node^='node:']");

async function shots(page, name) {
  for (const width of [1180, 820]) {
    await page.setViewportSize({ width, height: 900 });
    await page.screenshot({ path: path.join(artifacts, name + "-" + width + ".png"), fullPage: true });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), "no sideways scroll at " + width + " px");
  }
  await page.setViewportSize({ width: 1180, height: 900 });
}
/** Every Master branch open, so the tree shows every host the filter keeps. */
async function openTree(page) {
  // Wait for the tree to be drawn (a filter may hide every row, so don't insist on one).
  await page.locator("#dc-tree .dc-row").first().waitFor({ timeout: 5000 }).catch(() => {});
  for (let i = 0; i < 6; i++) {
    const closed = page.locator('#dc-tree .dc-toggle[aria-expanded="false"]');
    if (!(await closed.count())) return;
    await closed.first().click();
  }
}
async function filter(page, text) {
  await page.locator("#dc-search").fill(text);
  await openTree(page);
}
const visibleHosts = async page => (await treeHosts(page).allTextContents()).map(text => text.trim());

/** A host's page, Summary tab, from the tree. */
async function openHost(page, id) {
  await filter(page, "");
  await page.locator(`#dc-tree .dc-item[data-node="node:${id}"]`).click();
  await page.locator("#host-tags").waitFor();
}
const chips = page => page.locator("#host-tags .tag-chip .tag-name");
async function addTag(page, text, d, what) {
  const box = page.locator("#tag-add [name=tag]");
  if (d) await d.fill(box, text, what); else await box.fill(text);
  await box.press("Enter");
}

/** New job on a host's Jobs tab. */
async function newJob(page, id) {
  await page.locator(`#dc-tree .dc-item[data-node="node:${id}"]`).click();
  await page.locator('[data-action="dc-tab"][data-tab="jobs"]').click();
  if (!(await page.locator("#start-job").count())) await page.locator('[data-action="toggle-new-job"]').first().click();
  return page.locator("#start-job");
}
const groupRows = page => page.locator("#group-run tbody tr");

const checks = {
  /** The filter box matches folder names, for direct and merged hosts. */
  async "filter-folders"(page, url) {
    const web = await rpc(page, url, "folders.create", { name: "Web servers" });
    await rpc(page, url, "folders.assign", { targetId: "ui-test", folderId: web.id });
    const branch = await rpc(page, url, "folders.create", { name: "Branch office", master: "branch-master" });
    await rpc(page, url, "folders.assign", { targetId: "branch-master/child-2", folderId: branch.id });
    await page.goto(url);
    await page.locator("#dc-tree").waitFor();
    await filter(page, "web serv");
    await page.waitForFunction(() => document.querySelectorAll("#dc-tree .dc-item[data-node^='node:']").length === 1);
    assert.deepEqual((await visibleHosts(page)).map(t => t.includes("UI test target")), [true], "the host in Web servers");
    await filter(page, "branch office");
    await page.locator("#dc-tree .dc-item[data-node='node:branch-master/child-2']").waitFor();
    assert.equal(await treeHosts(page).filter({ hasText: "UI test target" }).count(), 0);
    await filter(page, "192.0.2.20"); // addresses still match
    await page.locator("#dc-tree .dc-item[data-node='node:ui-test']").waitFor();
    await shots(page, "filter-folder");
  },
  /** Tags on the host page: add with suggestions, case-insensitive with the first spelling, errors in place, ×. */
  async "host-tags"(page, url) {
    await page.goto(url);
    await openTree(page);
    await openHost(page, "ui-test");
    await addTag(page, "Web");
    await chips(page).filter({ hasText: /^Web$/ }).waitFor();
    await addTag(page, "บางกอก");
    await chips(page).filter({ hasText: "บางกอก" }).waitFor();
    for (const [text, sentence] of [["a,b", "A Tag can’t contain a comma"], ["x".repeat(33), "A Tag has 1 to 32 characters"]]) {
      await addTag(page, text);
      await page.locator("#host-tags .add-check.bad", { hasText: sentence }).waitFor();
      assert.equal(await chips(page).count(), 2, "a refused Tag changes nothing");
    }
    await shots(page, "host-tags");
    // A merged host: the existing Tag is suggested, and a different case is the same Tag.
    await openHost(page, "branch-master/child-1");
    assert.deepEqual(await page.locator("#tag-suggestions option").evaluateAll(options => options.map(o => o.value)), ["Web", "บางกอก"]);
    await addTag(page, "WEB");
    await chips(page).filter({ hasText: /^Web$/ }).waitFor();
    assert.ok((await calls(page, url)).some(c => c.method === "tags.set" && c.params.targetId === "branch-master/child-1"));
    // Filter by typing a Tag, or by clicking a chip.
    await filter(page, "web");
    await page.waitForFunction(() => document.querySelectorAll("#dc-tree .dc-item[data-node^='node:']").length === 2);
    assert.deepEqual((await treeHosts(page).evaluateAll(els => els.map(el => el.dataset.node))).sort(), ["node:branch-master/child-1", "node:ui-test"]);
    await filter(page, "");
    await openHost(page, "ui-test");
    await chips(page).filter({ hasText: "บางกอก" }).click();
    assert.equal(await page.locator("#dc-search").inputValue(), "บางกอก");
    await openTree(page);
    await page.waitForFunction(() => document.querySelectorAll("#dc-tree .dc-item[data-node^='node:']").length === 1);
    // × removes it; a Tag no host has is no longer suggested.
    await page.locator('#host-tags [data-action="tag-remove"][data-tag="บางกอก"]').click();
    await page.waitForFunction(() => document.querySelectorAll("#host-tags .tag-chip").length === 1);
    await openHost(page, "branch-master/child-1");
    assert.equal(await page.locator('#tag-suggestions option[value="บางกอก"]').count(), 0);
  },
  /** Chips in tables and on the Inventory host rows (none in the tree), "Tag…" in the tree menu, Tags kept by a backup. */
  async "tags-everywhere"(page, url) {
    await rpc(page, url, "tags.set", { targetId: "ui-test", tags: ["web", "db"] });
    await rpc(page, url, "tags.set", { targetId: "branch-master/child-1", tags: ["web"] });
    await page.goto(url);
    const card = page.locator('.inv-row[data-host="ui-test"]');
    await card.locator(".tag-chip", { hasText: "web" }).waitFor();
    await page.locator('.inv-row[data-host="branch-master/child-1"] .tag-chip', { hasText: "web" }).waitFor();
    assert.equal(await page.locator("#dc-tree .tag-chip").count(), 0, "the tree stays without Tags");
    await shots(page, "topology-tags");
    await openTree(page);
    await page.locator('#dc-tree .dc-item[data-node="master:local"]').click();
    await page.locator('#dc-panel tr[data-host="ui-test"] .tag-chip', { hasText: "db" }).waitFor();
    await page.locator('#dc-panel tr[data-host="ui-test"] .tag-chip .tag-name', { hasText: "db" }).click();
    assert.equal(await page.locator("#dc-search").inputValue(), "db", "a chip in a table filters too");
    await filter(page, "");
    // Tag… from the tree menu opens the host's Tag box.
    await page.locator('#dc-tree .dc-item[data-node="node:hq-master"]').click({ button: "right" });
    await page.locator('#dc-menu [data-action="ctx-tag"]').click();
    await page.locator("#tag-add [name=tag]").and(page.locator(":focus")).waitFor();
    await addTag(page, "Bangkok");
    await page.locator("#host-tags .tag-chip", { hasText: "Bangkok" }).waitFor();
    // A (fixture) backup and restore keeps the Tags.
    const saved = await (await page.request.post(url + "/rpc", { data: { command: "save_backup", args: { passphrase: "a long passphrase for tests", name: "t.sshccbak" } } })).json();
    assert.ok(saved);
    const picked = await (await page.request.post(url + "/rpc", { data: { command: "open_backup", args: {} } })).json();
    await page.request.post(url + "/scenario", { data: { flags: ["empty"] } });
    await rpc(page, url, "backup.restore", { file: picked.file, passphrase: "a long passphrase for tests" });
    assert.deepEqual((await rpc(page, url, "tags.list")).hosts["hq-master"], ["Bangkok"]);
  },
  /** The New job form starts in / (Linux) or C:\ (Windows), and an edited value is the one sent. */
  async "prefilled-cwd"(page, url) {
    await page.goto(url);
    await openTree(page);
    for (const [id, cwd] of [["branch-master/child-1", "C:\\"], ["ui-test", "/"]]) {
      await page.locator(`#dc-tree .dc-item[data-node="node:${id}"]`).click();
      await page.locator('[data-action="dc-tab"][data-tab="jobs"]').click();
      if (!(await page.locator("#start-job").count())) await page.locator('[data-action="toggle-new-job"]').first().click();
      assert.equal(await page.locator('#start-job [name="cwd"]').inputValue(), cwd, id);
    }
    await page.locator('#start-job [name="cwd"]').fill("/srv/app");
    await page.locator('#start-job [name="script"]').fill("pwd");
    await page.locator('#start-job button[type="submit"]').click();
    await page.waitForTimeout(500);
    const started = (await calls(page, url)).filter(c => c.method === "jobs.start");
    assert.equal(started.at(-1).params.cwd, "/srv/app");
  },
  /** The core task: tag 3 hosts `web`, run one command on all three. ≤ 5 decisions. */
  async "core-task"(page, url, d) {
    await page.goto(url);
    await openTree(page);
    for (const [i, id] of ["ui-test", "hq-master", "branch-master"].entries()) {
      await openHost(page, id);
      await addTag(page, "web", d, i ? "pick Tag web (suggested)" : "type Tag web");
      await page.locator("#host-tags .tag-chip", { hasText: "web" }).waitFor();
    }
    const form = await newJob(page, "branch-master");
    assert.equal(await form.locator('[name="cwd"]').inputValue(), "/", "pre-filled");
    await d.select(form.locator('[name="runOn"]'), "tag:web", "run on every host with Tag web");
    assert.ok(await form.locator(".host-only").evaluateAll(els => els.length === 2 && els.every(el => el.hidden)), "no Owner session or Shell for a group");
    await d.fill(form.locator('[name="script"]'), "uptime", "command");
    await form.locator('button[type="submit"]').click();
    const dialog = page.locator("#group-dialog");
    await dialog.locator("#group-title", { hasText: "Run on 3 hosts?" }).waitFor();
    assert.deepEqual((await dialog.locator("#group-hosts li").allTextContents()).map(t => t.replace(/\s+Linux$/, "")).sort(), ["Branch Master", "HQ Master", "UI test target"]);
    await shots(page, "group-confirm");
    assert.ok(!(await calls(page, url)).some(c => c.method === "groups.run"), "nothing runs before the confirmation");
    await dialog.getByRole("button", { name: "Run" }).click(); // a confirmation, not a decision
    await groupRows(page).first().waitFor();
    assert.equal(await groupRows(page).count(), 3, "one row per host");
    await page.locator("#group-run .group-summary", { hasText: "3 of 3 hosts succeeded" }).waitFor({ timeout: 15000 });
    const sent = (await calls(page, url)).find(c => c.method === "groups.run").params;
    assert.deepEqual([sent.selector.tag, sent.scripts, sent.cwd], ["web", { linux: "uptime" }, "/"]);
    assert.ok(!(await calls(page, url)).some(c => c.method === "jobs.start"), "no one-host job");
    await shots(page, "group-done");
  },
  /** A folder group with a host that can't be reached and one without the service. */
  async "group-offline"(page, url) {
    const far = await rpc(page, url, "folders.create", { name: "Far", master: "local" });
    for (const id of ["ui-test", "hq-master", "old-box"]) await rpc(page, url, "folders.assign", { targetId: id, folderId: far.id });
    await page.goto(url);
    await openTree(page);
    const form = await newJob(page, "ui-test");
    const option = form.locator('[name="runOn"] option', { hasText: "Every host in folder Far" });
    await form.locator('[name="runOn"]').selectOption(await option.getAttribute("value"));
    await form.locator('[name="script"]').fill("df -h");
    await form.locator('button[type="submit"]').click();
    const dialog = page.locator("#group-dialog");
    await dialog.locator("#group-title", { hasText: "Run on 2 hosts?" }).waitFor();
    await dialog.getByText("Skipped: the SSH Control Center service is not installed").waitFor();
    await dialog.locator("#group-skipped li", { hasText: "Old box" }).waitFor();
    await dialog.getByRole("button", { name: "Run" }).click();
    const hq = page.locator('#group-run tr[data-host="hq-master"]');
    await hq.locator(".badge", { hasText: "Couldn’t reach" }).waitFor();
    await hq.getByText("No route to host").waitFor();
    await page.locator('#group-run tr[data-host="old-box"] .badge', { hasText: "Skipped" }).waitFor();
    await page.locator("#group-run .group-summary", { hasText: "1 of 2 hosts succeeded" }).waitFor({ timeout: 15000 });
    await shots(page, "group-offline");
    await page.locator('#group-run tr[data-host="ui-test"] [data-action="group-log"]').click();
    await page.locator('[data-action="dc-tab"][data-tab="jobs"][aria-selected="true"]').waitFor();
    assert.equal(await page.locator("#group-run").count(), 0);
  },
  /** Mixed OS: two script boxes; cancel one, cancel all; retry only what failed; recent runs after a reload. */
  async "group-more"(page, url) {
    await rpc(page, url, "tags.set", { targetId: "ui-test", tags: ["mix"] });
    await rpc(page, url, "tags.set", { targetId: "branch-master/child-1", tags: ["mix"] }); // Windows
    await rpc(page, url, "tags.set", { targetId: "hq-master", tags: ["mix"] });
    await page.goto(url);
    await openTree(page);
    const form = await newJob(page, "ui-test");
    await form.locator('[name="runOn"]').selectOption("tag:mix");
    await form.locator('[name="scriptLinux"]').waitFor();
    assert.ok(await form.locator('[name="script"]').isHidden(), "one box per OS instead of one");
    assert.ok(await form.locator('[name="cwd"]').isHidden(), "each OS runs in its own default folder");
    assert.ok(await form.locator(".host-only").evaluateAll(els => els.every(el => el.hidden)));
    await form.locator('[name="scriptLinux"]').fill("uname -a");
    await form.locator('[name="scriptWindows"]').fill("Get-ComputerInfo");
    await form.locator("details summary").click();
    await form.locator('[name="asAdmin"]').check();
    await form.locator('[name="env"]').fill('{"STAGE":"prod"}');
    await shots(page, "group-mixed");
    await form.locator('button[type="submit"]').click();
    await page.locator("#group-dialog").getByRole("button", { name: "Run" }).click();
    const sent = (await calls(page, url)).find(c => c.method === "groups.run").params;
    assert.deepEqual([sent.scripts, sent.cwd, sent.asAdmin, sent.env], [{ linux: "uname -a", windows: "Get-ComputerInfo" }, { linux: "/", windows: "C:\\" }, true, { STAGE: "prod" }]);
    // Cancel one host, then the rest.
    await page.locator('#group-run tr[data-host="hq-master"] [data-action="group-cancel"]').click();
    await page.locator('#group-run tr[data-host="hq-master"] .badge', { hasText: "Cancelled" }).waitFor();
    await page.locator('#group-run .page-actions [data-action="group-cancel"]').click();
    await page.locator("#group-run .group-summary", { hasText: "0 of 3 hosts succeeded" }).waitFor({ timeout: 15000 });
    // Try again: only the hosts that didn't succeed get new jobs.
    await page.locator('[data-action="group-retry"]').click();
    await page.locator("#group-run .group-summary", { hasText: "3 of 3 hosts succeeded" }).waitFor({ timeout: 15000 });
    assert.equal(await page.locator('[data-action="group-retry"]').count(), 0, "nothing left to retry");
    // The run on the current Master's Activity tab (entry 5), also after a reload.
    await page.reload();
    await openTree(page);
    await page.locator('#dc-tree .dc-item[data-node="master:local"]').click();
    await page.locator('[data-action="dc-tab"][data-tab="activity"]').click();
    const row = page.locator("#activity-list tr.group-row", { hasText: "Tag mix · 3 hosts" });
    await row.waitFor();
    await row.locator('[data-action="activity-toggle"]').click();
    await page.locator("#activity-list tr.group-host").first().waitFor();
    await shots(page, "group-recent");
  },
};
const scenarios = { "filter-folders": ["masters-page"], "host-tags": ["masters-page"], "tags-everywhere": ["masters-page"], "prefilled-cwd": ["masters-page"],
  "core-task": ["masters-page"], "group-offline": ["masters-page", "legacy-host", "group-offline"], "group-more": ["masters-page", "group-slow"] };

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
    fs.writeFileSync(path.join(artifacts, "tags-results.json"), JSON.stringify({ ok: true, gate: MAX_DECISIONS, results }, null, 2) + "\n");
    console.log(JSON.stringify({ ok: true }));
  } finally { await browser.close(); }
})().catch(e => { console.error(e); process.exitCode = 1; });
