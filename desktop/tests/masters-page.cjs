// Masters page split by direction ("Controlled from here" / "This Master is controlled by") on the fixture's
// masters-page scenario: an unmerged Master gets Merge with no search, a Master that reaches this one directly and
// through a Merge is one row with both badges, and Revoke / Disable merging / Allow again still work from the rows.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");
const { startFixture } = require("./decisions.cjs");
const artifacts = path.resolve(__dirname, "../../artifacts/add-host-ui"); fs.mkdirSync(artifacts, { recursive: true });

(async () => {
  const browser = await chromium.launch({ headless: true }), fixture = await startFixture(["masters-page"]);
  try {
    const page = await browser.newPage({ viewport: { width: 1180, height: 900 } });
    page.on("dialog", dialog => dialog.accept());
    const sent = async () => (await (await page.request.get(fixture.url + "/calls")).json()).filter(c => c.command === "rpc").map(c => c.method + " " + JSON.stringify(c.params || {}));
    await page.goto(fixture.url);
    await page.locator('nav [data-page="masters"], [data-page="masters"].nav').first().click();
    await page.locator("h1", { hasText: /^Masters$/ }).waitFor();

    // Outgoing: the merged Master and the unmerged one, with no "Find Masters" and no fleet-wide probe.
    const outgoing = page.locator("section", { has: page.locator("h2", { hasText: /^Controlled from here$/ }) });
    const hq = outgoing.locator("tr", { hasText: "HQ Master" }), branch = outgoing.locator("tr", { hasText: "Branch Master" });
    await hq.locator('[data-action="merge-master"]').waitFor();
    await branch.locator('[data-action="sync-merge"]').waitFor();
    await branch.locator('[data-action="remove-merge"]').waitFor();
    assert.equal(await outgoing.locator("tr", { hasText: "UI test target" }).count(), 0, "a host that is not a Master is not listed");
    assert.equal(await page.getByRole("button", { name: /Find Masters/ }).count(), 0);
    assert.equal(await page.locator('[data-action="discover-masters"]').count(), 0);
    assert.ok(!(await sent()).some(c => c.startsWith("merges.discover")), "no fleet-wide Master probe");

    // Incoming: one row per Master, joined on the Master ID.
    const incoming = page.locator("section", { has: page.locator("h2", { hasText: /^This Master is controlled by$/ }) });
    await incoming.locator('[data-action="accept-enrollment"]', { hasText: "Allow another Master…" }).waitFor();
    const peer = incoming.locator("tbody tr", { hasText: "peer-both" });
    assert.equal(await peer.count(), 1, "one row for a Master that is both direct and merged");
    await peer.locator(".badge", { hasText: /^Direct$/i }).waitFor();
    await peer.locator(".badge", { hasText: /^Through merge$/i }).waitFor();
    assert.equal(await incoming.locator("tbody tr", { hasText: "fixture-master" }).count(), 1);
    const relayOnly = incoming.locator("tbody tr", { hasText: "peer-relay" });
    assert.equal(await relayOnly.locator(".badge", { hasText: /^Direct$/i }).count(), 0);
    for (const width of [1180, 820]) {
      await page.setViewportSize({ width, height: 900 }); await page.screenshot({ path: path.join(artifacts, "masters-page-" + width + ".png"), fullPage: true });
      // Wide action cells must not widen the page (they wrap inside the table instead).
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), "no sideways scroll at " + width + " px");
    }
    await page.setViewportSize({ width: 1180, height: 900 });

    // Actions from the rows.
    await hq.locator('[data-action="merge-master"]').click();
    await hq.locator('[data-action="remove-merge"]').waitFor();
    assert.ok((await sent()).includes('merges.add {"targetId":"hq-master"}'));
    await peer.locator('[data-action="revoke-relay"]').click();
    await peer.locator('[data-action="allow-relay"]').waitFor();
    assert.ok((await sent()).includes('relays.revoke {"masterId":"peer-both"}'));
    await relayOnly.locator('[data-action="allow-relay"]').click();
    await relayOnly.locator('[data-action="revoke-relay"]').waitFor();
    assert.ok((await sent()).includes('relays.allow {"masterId":"peer-relay"}'));
    await peer.locator('[data-action="revoke-master"]').click();
    await peer.locator('[data-action="revoke-master"]').waitFor({ state: "detached" });
    assert.ok((await sent()).includes('masters.revoke {"masterId":"peer-both"}'));
    assert.ok(!(await sent()).some(c => c.startsWith("merges.discover")), "still no fleet-wide Master probe");
    await page.screenshot({ path: path.join(artifacts, "masters-page-after-actions-1180.png"), fullPage: true });
    console.log(JSON.stringify({ ok: true }));
  } finally { fixture.stop(); await browser.close(); }
})().catch(e => { console.error(e); process.exitCode = 1; });
