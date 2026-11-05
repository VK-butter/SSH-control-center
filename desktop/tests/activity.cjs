// Activity (roadmap entry 5) on the fixture (plain Chromium): Recent Jobs and the audit log on the current Master's
// Activity tab. Gates the core task, find what ran on host X yesterday and export CSV, at ≤ 3 decisions.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");
const { decisions, startFixture } = require("./decisions.cjs");
const artifacts = path.resolve(__dirname, "../../artifacts/activity-ui"); fs.mkdirSync(artifacts, { recursive: true });
const MAX_DECISIONS = 3;
const calls = async (page, url) => (await (await page.request.get(url + "/calls")).json());
const rpc = async (page, url, method, params = {}) => (await page.request.post(url + "/rpc", { data: { command: "rpc", args: { method, params } } })).json();
const lastCsv = async (page, url) => (await (await page.request.get(url + "/last-csv")).json());

async function shots(page, name) {
  for (const width of [1180, 820, 390]) {
    await page.setViewportSize({ width, height: 900 });
    await page.screenshot({ path: path.join(artifacts, name + "-" + width + ".png"), fullPage: true });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), "no sideways scroll at " + width + " px");
  }
  await page.setViewportSize({ width: 1180, height: 900 });
}
/** The current Master's page, then its Activity tab (counted as a decision when `d` is given). */
async function openActivity(page, url, d) {
  await page.goto(url);
  await page.locator('#dc-tree .dc-item[data-node="master:local"]').click();
  const tab = page.locator('[data-action="dc-tab"][data-tab="activity"]');
  if (d) await d.choose(tab, "Activity tab"); else await tab.click();
  await page.locator("#activity-list table, #activity-list .empty-inline:not(:has-text('Loading'))").first().waitFor();
}
const jobRows = page => page.locator("#activity-list tbody tr");
const rowTexts = async page => (await jobRows(page).allTextContents()).map(text => text.replace(/\s+/g, " ").trim());

const checks = {
  /** The core task: what ran on UI test target yesterday, as CSV. */
  async "core-task"(page, url, d) {
    await openActivity(page, url, d);
    // Typing the host and picking Yesterday is one decision: "which host, which day".
    await page.locator("#activity-search").fill("UI test");
    await d.choose(page.locator('[data-action="activity-range"][data-range="yesterday"]'), "host X, yesterday");
    await page.waitForFunction(() => document.querySelectorAll("#activity-list tbody tr").length === 1);
    const [row] = await rowTexts(page);
    assert.match(row, /14:03/, "Thai time of the job: " + row);
    assert.match(row, /systemctl restart nginx/);
    assert.ok(!(await rowTexts(page)).some(text => text.includes("Lab PC")), "only host X");
    await d.choose(page.locator('[data-action="activity-export"]'), "Export CSV");
    await page.locator("#notice", { hasText: "Saved" }).waitFor();
    const saved = await lastCsv(page, url);
    assert.match(saved.name, /^sshcc-jobs-\d{4}-\d{2}-\d{2}\.csv$/);
    assert.ok(saved.csv.startsWith("﻿"), "BOM for Excel");
    const lines = saved.csv.slice(1).trim().split("\r\n");
    assert.equal(lines[0], "time,host,group,status,exit_code,started,finished,cwd,as_admin,channel,job_id,script");
    assert.equal(lines.length, 2, "the filtered row only: " + saved.csv);
    assert.match(lines[1], /^\d{4}-\d{2}-\d{2}T14:03:00\+07:00,ui-test,,succeeded,0,/);
    assert.match(lines[1], /"systemctl restart nginx\nsystemctl status nginx"$/m);
    const sent = (await calls(page, url)).find(c => c.command === "save_csv");
    assert.deepEqual(sent.params, { range: "yesterday", hosts: ["ui-test"], text: "UI test", view: "jobs" }, "the same filter as on screen");
    await shots(page, "core-task");
  },
  /** Group rows expand; status and range filters; a running job finishes on its own; an offline host's record. */
  async "jobs-view"(page, url) {
    await openActivity(page, url);
    // Today: the group run (one entry) and the running job.
    const group = page.locator("#activity-list tr.group-row", { hasText: "Tag web · 2 hosts" });
    await group.waitFor();
    assert.match(await group.textContent(), /0 of 2 succeeded|1 of 2 succeeded/);
    await group.locator('[data-action="activity-toggle"]').click();
    const hosts = page.locator("#activity-list tr.group-host");
    assert.equal(await hosts.count(), 2);
    assert.match(await hosts.filter({ hasText: "Lab PC" }).textContent(), /Couldn’t reach/);
    // The running job is asked again while the page is open and finishes.
    const live = page.locator("#activity-list tr", { hasText: "sleep 5; echo done" });
    await live.locator(".badge", { hasText: "Succeeded" }).waitFor({ timeout: 15000 });
    // Status: Failed over 7 days keeps the group (a host couldn't be reached) and the failed apt-get.
    await page.locator('[data-action="activity-range"][data-range="7d"]').click();
    await page.locator("#activity-status").selectOption("failed");
    await page.waitForFunction(() => !document.querySelector("#activity-list")?.textContent.includes("systemctl"));
    const failed = await rowTexts(page);
    assert.ok(failed.some(t => t.includes("apt-get update")) && failed.some(t => t.includes("Tag web")), failed.join("\n"));
    assert.ok(!failed.some(t => t.includes("sleep 5")));
    // Filtering by a host shows the group run it was in.
    await page.locator("#activity-status").selectOption("all");
    await page.locator("#activity-search").fill("lab");
    await page.waitForFunction(() => !document.querySelector("#activity-list")?.textContent.includes("apt-get"));
    assert.ok((await rowTexts(page)).some(t => t.includes("Tag web")), "lab-pc's group run");
    // Custom: one day.
    await page.locator("#activity-search").fill("");
    await page.locator('[data-action="activity-range"][data-range="custom"]').click();
    const day = new Date(Date.now() + 7 * 3600000 - 3 * 86400000).toISOString().slice(0, 10);
    await page.locator("#activity-from").fill(day);
    await page.locator("#activity-to").fill(day);
    await page.waitForFunction(() => document.querySelectorAll("#activity-list tbody tr").length === 1);
    assert.match((await rowTexts(page))[0], /apt-get update/);
    // An offline host: the recorded job, with why its output can't be read.
    await page.locator('[data-action="activity-range"][data-range="yesterday"]').click();
    await page.locator('#activity-list [data-job="job-lab"]').click();
    const detail = page.locator("#activity-detail");
    await detail.waitFor();
    assert.match(await detail.textContent(), /can’t be reached/);
    assert.match(await detail.textContent(), /No route to host/);
    assert.match(await detail.textContent(), /df -h/);
    await shots(page, "jobs-view");
    // A reachable host opens the existing output view on its Jobs tab.
    await page.locator('[data-action="activity-detail-close"]').click();
    await page.locator('#activity-list [data-job="job-yesterday"]').click();
    await page.locator('#log-output').waitFor();
    assert.equal(await page.locator('[data-action="dc-tab"][data-tab="jobs"]').getAttribute("aria-selected"), "true");
  },
  /** The audit view: translated actions, red failed rows, channels, its own text search; CSV of the view. */
  async "audit-view"(page, url) {
    await openActivity(page, url);
    await page.locator('[data-action="activity-view"][data-view="audit"]').click();
    await page.locator('[data-action="activity-range"][data-range="7d"]').click();
    await page.locator("#activity-list .audit-table").waitFor();
    await page.waitForFunction(() => document.querySelectorAll("#activity-list tbody tr").length === 7);
    const texts = await rowTexts(page);
    assert.match(texts[0], /Open a Shell/, "newest first, translated: " + texts[0]);
    assert.ok(texts.some(t => /Start a group run.*tag web.*AI agent \(MCP\)/.test(t)), texts.join("\n"));
    const refused = page.locator("#activity-list tr.failed");
    assert.equal(await refused.count(), 1);
    assert.match(await refused.textContent(), /Delete a file.*E_PERMISSION.*Merge: Branch Master/);
    const [cell, rose] = await refused.evaluate(el => { const probe = document.createElement("i"); probe.style.color = "var(--rose)"; document.body.append(probe);
      const colors = [getComputedStyle(el.querySelector("td")).color, getComputedStyle(probe).color]; probe.remove(); return colors; });
    assert.equal(cell, rose, "failed rows in rose");
    // Search matches the action too ("tags" → Set Tags).
    await page.locator("#activity-search").fill("tags.set");
    await page.waitForFunction(() => document.querySelectorAll("#activity-list tbody tr").length === 1);
    assert.match((await rowTexts(page))[0], /Set Tags/);
    await page.locator('[data-action="activity-export"]').click();
    await page.locator("#notice", { hasText: "Saved" }).waitFor();
    const saved = await lastCsv(page, url);
    assert.match(saved.name, /^sshcc-audit-/);
    assert.match(saved.csv, /^﻿time,action,object,result,channel,user\r\n\d{4}-\d{2}-\d{2}T09:15:00\+07:00,tags\.set,ui-test · web,ok,desktop,fixture-owner\r\n$/);
    await page.locator("#activity-search").fill("");
    await shots(page, "audit-view");
    // Work done in the page is audited too: a Tag set now is the newest line, as "This app".
    await rpc(page, url, "tags.set", { targetId: "ui-test", tags: ["web", "db"] });
    await page.locator('[data-action="activity-range"][data-range="today"]').click();
    await page.locator("#activity-list tbody tr", { hasText: "ui-test · web, db" }).waitFor();
  },
  /** An empty history: "Recording since"; the keep-days setting on Summary; the warning. */
  async "keep-and-warning"(page, url) {
    await openActivity(page, url);
    await page.locator("#activity-list .activity-since", { hasText: "Recording since" }).waitFor();
    assert.match(await page.locator("#activity-list").textContent(), /Nothing in this range/);
    await page.locator('[data-action="dc-tab"][data-tab="summary"]').click();
    const form = page.locator("#activity-keep-form");
    assert.equal(await form.locator('[name="keepDays"]').inputValue(), "14", "two weeks by default");
    await form.locator('[name="keepDays"]').fill("30");
    await form.locator('button[type="submit"]').click();
    await page.locator("#notice", { hasText: "Activity is kept 30 days" }).waitFor();
    assert.equal((await rpc(page, url, "activity.status")).keepDays, 30);
    await page.request.post(url + "/scenario", { data: { flags: ["activity-warning"] } });
    await page.reload();
    await page.locator('#dc-tree .dc-item[data-node="master:local"]').click();
    await page.locator("#activity-keep #activity-warning", { hasText: "Activity can’t be recorded" }).waitFor();
    await page.locator('[data-action="dc-tab"][data-tab="activity"]').click();
    await page.locator("#activity #activity-warning", { hasText: "Activity can’t be recorded" }).waitFor();
    await shots(page, "warning");
  },
};
const scenarios = { "core-task": ["activity-history"], "jobs-view": ["activity-history"], "audit-view": ["activity-history"], "keep-and-warning": [] };

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
    fs.writeFileSync(path.join(artifacts, "activity-results.json"), JSON.stringify({ ok: true, gate: MAX_DECISIONS, results }, null, 2) + "\n");
    console.log(JSON.stringify({ ok: true }));
  } finally { await browser.close(); }
})().catch(e => { console.error(e); process.exitCode = 1; });
