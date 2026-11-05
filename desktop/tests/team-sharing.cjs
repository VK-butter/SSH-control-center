// Team sharing through merge (roadmap entry 6, Stage A) on the fixture (plain Chromium): the grant controls on
// "This Master is controlled by" (role, Tag limit, pass-through), and what a View-only Merge looks like on the merging
// side. Gates the core task, give a teammate's Master View only, at ≤ 2 decisions.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");
const { decisions, startFixture } = require("./decisions.cjs");
const artifacts = path.resolve(__dirname, "../../artifacts/team-sharing-ui"); fs.mkdirSync(artifacts, { recursive: true });
const MAX_DECISIONS = 2;
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
async function mastersPage(page, url) {
  await page.goto(url);
  await page.locator('nav [data-page="masters"], [data-page="masters"].nav').first().click();
  const incoming = page.locator("section", { has: page.locator("h2", { hasText: /^This Master is controlled by$/ }) });
  await incoming.waitFor();
  return incoming;
}
/** A host's page from the tree (every branch opened). */
async function openHost(page, id) {
  await page.locator("#dc-tree .dc-row").first().waitFor();
  for (let i = 0; i < 6; i++) {
    const closed = page.locator('#dc-tree .dc-toggle[aria-expanded="false"]');
    if (!(await closed.count())) break;
    await closed.first().click();
  }
  await page.locator(`#dc-tree .dc-item[data-node="node:${id}"]`).click();
}

const checks = {
  /** The core task: Peer Master gets View only, from the Masters page. */
  async "core-task"(page, url, d) {
    const incoming = await mastersPage(page, url);
    const row = incoming.locator('tr[data-relay="peer-both"]');
    const role = row.locator('[data-grant="role"]');
    assert.equal(await role.inputValue(), "full", "a Merge starts as Full control");
    await d.select(role, "view", "Peer Master: View only");
    await page.locator("#notice", { hasText: "Saved · Peer Master" }).waitFor();
    const sent = (await rpcCalls(page, url)).filter(c => c.method === "relays.set");
    assert.deepEqual(sent.map(c => c.params), [{ masterId: "peer-both", role: "view" }]);
    assert.equal(await incoming.locator('tr[data-relay="peer-both"] [data-grant="role"]').inputValue(), "view");
    // A Master that only controls this one directly has no grant to set (direct pairing is always full).
    assert.equal(await incoming.locator('tr[data-relay="fixture-master"] .grant').count(), 0);
    await shots(page, "grant-view-only");
  },
  /** Tag limit and pass-through on the same row. */
  async "tags-and-pass-through"(page, url) {
    await rpc(page, url, "tags.set", { targetId: "ui-test", tags: ["web"] });
    const incoming = await mastersPage(page, url);
    const row = () => incoming.locator('tr[data-relay="peer-both"]');
    await row().locator('[data-grant="scope"]').selectOption("tags");
    const tags = row().locator('[data-grant="tags"]');
    await tags.waitFor();
    assert.ok(await page.locator('#grant-tags option[value="web"]').count(), "this Master's Tags are suggested");
    await tags.fill("web, db");
    await tags.press("Enter");
    await page.locator("#notice", { hasText: "Saved" }).waitFor();
    await page.waitForFunction(() => document.querySelector('tr[data-relay="peer-both"] [data-grant="tags"]')?.value === "web, db");
    await row().locator('[data-grant="passThrough"]').uncheck();
    const saves = async () => (await rpcCalls(page, url)).filter(c => c.method === "relays.set").length;
    for (let i = 0; i < 100 && (await saves()) < 2; i++) await page.waitForTimeout(50);
    await row().locator('[data-grant="scope"]').selectOption("all");
    await page.waitForFunction(() => !document.querySelector('tr[data-relay="peer-both"] [data-grant="tags"]'));
    const sent = (await rpcCalls(page, url)).filter(c => c.method === "relays.set").map(c => c.params);
    assert.deepEqual(sent, [{ masterId: "peer-both", tags: ["web", "db"] }, { masterId: "peer-both", passThrough: false }, { masterId: "peer-both", tags: null }]);
    await shots(page, "grant-tags");
  },
  /** The merging side: a View-only Merge's hosts carry the badge, and run / Shell / write are disabled with the reason. */
  async "view-only-host"(page, url) {
    await page.goto(url);
    await openHost(page, "branch-master/child-2");
    await page.locator(".badge.view-only").first().waitFor();
    assert.ok(await page.locator('#dc-tree .dc-item[data-node="node:branch-master/child-2"] .dc-tag', { hasText: "View only" }).count(), "badge in the tree");
    const shell = page.locator('.target-actions [data-action="open-shell"]');
    assert.ok(await shell.isDisabled());
    assert.equal(await shell.getAttribute("title"), "Branch Master gives this Master View only");
    await page.locator('[data-action="dc-tab"][data-tab="jobs"]').click();
    const newJob = page.locator('[data-action="toggle-new-job"]').first();
    await newJob.waitFor();
    assert.ok(await newJob.isDisabled(), "no new job");
    await page.locator('[data-action="dc-tab"][data-tab="files"]').click();
    await page.locator('[data-action="upload-pick"]').waitFor();
    await page.waitForFunction(() => document.querySelector('[data-action="upload-pick"]')?.disabled && document.querySelector('[data-action="mkdir"]')?.disabled);
    // A host this Master controls fully is untouched.
    await openHost(page, "ui-test");
    assert.ok(!(await page.locator('.target-actions [data-action="open-shell"]').isDisabled()));
    assert.equal(await page.locator('.badge.view-only').count(), 0);
    await openHost(page, "branch-master/child-2");
    await shots(page, "view-only-host");
  },
  /** A refusal that gets through (the grant changed since the last sync) reads as its sentence. */
  async "refusal-sentence"(page, url) {
    await page.goto(url);
    await openHost(page, "branch-master/child-2");
    await page.locator('[data-action="dc-tab"][data-tab="jobs"]').click();
    await page.locator('[data-action="toggle-new-job"]').first().click();
    await page.locator('#start-job [name="script"]').fill("uptime");
    await page.locator('#start-job [type="submit"]').click();
    await page.locator("#notice", { hasText: "Branch Master lets this Master reach only hosts with Tag web" }).waitFor();
  },
  /** Stage B: a host two Masters away appears on its own, nested under each Master, with its path and right. */
  async "far-hosts"(page, url, d) {
    await page.goto(url);
    await page.locator("#dc-tree .dc-row").first().waitFor();
    for (let i = 0; i < 8; i++) {
      const closed = page.locator('#dc-tree .dc-toggle[aria-expanded="false"]');
      if (!(await closed.count())) break;
      await closed.first().click();
    }
    // Nested: Branch Master's branch holds Lab Master's branch, which holds Lab's hosts.
    const lab = page.locator('#dc-tree .dc-item[data-node="master:branch-master/lab-master"]');
    await lab.waitFor();
    const depth = async node => Number(/depth-(\d)/.exec(await page.locator(`#dc-tree li:has(> .dc-item[data-node="${node}"])`).getAttribute("class"))[1]);
    assert.equal(await depth("master:branch-master/lab-master"), (await depth("master:branch-master")) + 1);
    assert.equal(await depth("node:branch-master/lab-master/lab-web"), (await depth("master:branch-master/lab-master")) + 1);
    assert.equal(d.count, 0, "nothing to do to see a host two hops away");
    await page.locator('#dc-tree .dc-item[data-node="node:branch-master/lab-master/lab-db"]').click();
    await page.locator(".live-label", { hasText: "Path: UI fixture (no real service) → Branch Master → Lab Master" }).waitFor();
    await page.locator(".meta-row small", { hasText: "View only (from Lab Master)" }).waitFor();
    assert.ok(await page.locator('.target-actions [data-action="open-shell"]').isDisabled());
    await shots(page, "far-host");
    // A Master on the way that doesn't answer is named.
    await page.locator('#dc-tree .dc-item[data-node="node:branch-master/lab-master/lab-web"]').click();
    await page.locator('[data-action="dc-tab"][data-tab="jobs"]').click();
    await page.locator('[data-action="toggle-new-job"]').first().click();
    await page.locator('#start-job [name="script"]').fill("uptime");
    await page.locator('#start-job [type="submit"]').click();
    await page.locator("#notice", { hasText: "Lab Master didn’t answer in time" }).waitFor();
    // Masters page: hosts left out for distance are counted.
    await page.locator('.nav[data-page="masters"]').click();
    await page.locator("#beyond-note", { hasText: "3 more hosts are more than 4 hops away" }).waitFor();
  },
  /** An older merged Master is marked, with what updating it would give. */
  async "outdated-master"(page, url) {
    await page.goto(url);
    await page.locator('.nav[data-page="masters"]').click();
    const branch = page.locator("tr", { hasText: "Branch Master" }).first();
    await branch.locator(".badge", { hasText: /^Outdated$/ }).waitFor();
    assert.match(await branch.textContent(), /update it to see the hosts it reaches through its own Merges/);
    await shots(page, "outdated-master");
  },
};
const scenarios = { "core-task": ["masters-page"], "tags-and-pass-through": ["masters-page"], "view-only-host": ["masters-page", "view-only-merge"], "refusal-sentence": ["masters-page", "merge-refuses"],
  "far-hosts": ["masters-page", "multi-hop"], "outdated-master": ["masters-page", "outdated-merge"] };

(async () => {
  const browser = await chromium.launch({ headless: true }); const results = [];
  try {
    for (const [name, flags] of Object.entries(scenarios)) {
      const fixture = await startFixture(flags);
      try {
        const page = await browser.newPage({ viewport: { width: 1180, height: 900 }, locale: "en-US" });
        page.on("dialog", dialog => dialog.accept());
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
    fs.writeFileSync(path.join(artifacts, "team-sharing-results.json"), JSON.stringify({ ok: true, gate: MAX_DECISIONS, results }, null, 2) + "\n");
    console.log(JSON.stringify({ ok: true }));
  } finally { await browser.close(); }
})().catch(e => { console.error(e); process.exitCode = 1; });
