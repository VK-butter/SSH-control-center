// Backup and restore (roadmap entry 3) on the fixture (plain Chromium). Gates the core task, back up → wipe → restore,
// at ≤ 4 decisions: set the passphrase (typed twice, counted once), choose where to save, choose the file, type the
// passphrase. "The old machine is off" and "Change keys" are confirmations, not decisions. Also checks the section on the
// current Master's Summary tab (and not on the Service page), the last-backup line, the passphrase rules and the errors.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");
const { decisions, startFixture } = require("./decisions.cjs");
const artifacts = path.resolve(__dirname, "../../artifacts/backup-ui"); fs.mkdirSync(artifacts, { recursive: true });
const MAX_DECISIONS = 4, PASSPHRASE = "a long passphrase for tests";
const calls = async (page, url) => (await (await page.request.get(url + "/calls")).json());
const flow = page => page.locator("#backup-flow");
const step = (page, n) => flow(page).locator('.add-step[data-step="' + n + '"]');
const section = page => page.locator("#backup-section");

async function assertCurrent(page, n) {
  await step(page, n).and(page.locator('[aria-current="step"]')).waitFor();
  assert.equal(await flow(page).locator('[aria-current="step"]').count(), 1, "one current step");
}
/** The current Master's page, Summary tab, from the tree. */
async function openCurrentMaster(page) {
  await page.locator('#dc-tree .dc-item[data-node="master:local"]').click();
  await section(page).waitFor();
}
async function shots(page, name) {
  for (const width of [1180, 820]) {
    await page.setViewportSize({ width, height: 900 });
    await page.screenshot({ path: path.join(artifacts, name + "-" + width + ".png"), fullPage: true });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), "no sideways scroll at " + width + " px");
  }
  await page.setViewportSize({ width: 1180, height: 900 });
}
/** Back up as a person does: Back up…, the passphrase twice, Save backup… (the save dialog). */
async function backUp(page, url, d) {
  if (!(await flow(page).count())) await section(page).getByRole("button", { name: "Back up…" }).click();
  await assertCurrent(page, 1);
  const form = page.locator("#backup-pass-form");
  await d.fill(form.locator('[name="pass"]'), PASSPHRASE, "passphrase");
  await form.locator('[name="again"]').fill(PASSPHRASE); // typing it twice counts once
  await assertCurrent(page, 2);
  await page.locator("#backup-check", { hasText: "The passphrases match" }).waitFor();
  await d.choose(page.locator("#backup-save"), "where to save (save dialog)");
  await assertCurrent(page, 3);
  const saved = (await calls(page, url)).filter(c => c.command === "save_backup");
  assert.equal(saved.length, 1, "one save_backup");
  assert.equal(saved[0].passphrase, PASSPHRASE);
  assert.equal(saved[0].title, "Save the backup of this Master");
  assert.match(saved[0].name, /^sshcc-backup-[^/\\\s]+-\d{4}-\d{2}-\d{2}\.sshccbak$/, "default file name");
  await step(page, 3).getByText(/^Saved to \/fixture\/Backups\/sshcc-backup-.*\.sshccbak$/).waitFor();
}

const nextBackup = (page, url, kind) => page.request.post(url + "/next-backup", { data: { kind } });
/** Restore as a person does: Restore from a backup…, choose the file, type its passphrase, confirm the old machine is off. */
async function restore(page, d, passphrase, { start = true } = {}) {
  if (start) await page.locator("#restore-offer").getByRole("button", { name: "Restore from a backup…" }).click();
  await assertCurrent(page, 1);
  await d.choose(step(page, 1).getByRole("button", { name: "Choose file…" }), "choose the backup file (open dialog)");
  await assertCurrent(page, 2);
  await d.fill(page.locator('#restore-pass-form [name="pass"]'), passphrase, "passphrase of the backup");
  await page.locator("#restore-pass-form").getByRole("button", { name: "Open the backup" }).click();
}

const checks = {
  /** The section, the passphrase rules and a saved backup. */
  async "back-up"(page, url, d) {
    await page.goto(url);
    await openCurrentMaster(page);
    await section(page).getByText("Never backed up").waitFor();
    // A Master with hosts can't be restored over.
    assert.ok(await section(page).getByRole("button", { name: "Restore from a backup…" }).isDisabled());
    await section(page).getByText("Restore works only on a fresh install with no hosts").waitFor();
    await section(page).getByRole("button", { name: "Back up…" }).click();
    await assertCurrent(page, 1);
    const form = page.locator("#backup-pass-form"), save = page.locator("#backup-save"), check = page.locator("#backup-check");
    assert.equal(await form.locator('[name="pass"]').evaluate(el => el === document.activeElement), true, "focus on the passphrase");
    await form.getByText("The file gives full control of every host of this Master").waitFor();
    await form.getByText("A lost passphrase can’t be recovered: without it, the file is useless").waitFor();
    assert.ok(await save.isDisabled(), "no save before a passphrase");
    await form.locator('[name="pass"]').fill("too short");
    await form.locator('[name="again"]').fill("too short");
    await check.and(page.locator(".bad")).getByText("Use at least 12 characters").waitFor();
    assert.ok(await save.isDisabled(), "a short passphrase can't be saved");
    await form.locator('[name="pass"]').fill(PASSPHRASE);
    await form.locator('[name="again"]').fill(PASSPHRASE + "x");
    await check.and(page.locator(".bad")).getByText("The two passphrases are different").waitFor();
    assert.ok(await save.isDisabled(), "different passphrases can't be saved");
    await assertCurrent(page, 1);
    await form.locator('[name="pass"]').fill(""); await form.locator('[name="again"]').fill("");
    await shots(page, "backup-passphrase");
    await backUp(page, url, d);
    await shots(page, "backup-saved");
    await step(page, 3).getByRole("button", { name: "Done" }).click();
    await section(page).locator("#backup-last", { hasText: /^Last backup: / }).waitFor();
    // Nothing on the Service page.
    await page.locator('[data-page="service"]').first().click();
    await page.locator("#main h1").first().waitFor();
    assert.equal(await section(page).count(), 0, "no backup section on the Service page");
  },
  async "backed-up"(page, url) {
    await page.goto(url);
    await openCurrentMaster(page);
    await section(page).locator("#backup-last", { hasText: /^Last backup: 20\/10\/2026/ }).waitFor();
    await shots(page, "backup-section");
  },
  /** A cancelled save dialog leaves the flow on step 2 with nothing saved. */
  async "save-cancelled"(page, url) {
    await page.route("**/rpc", async route => {
      const body = route.request().postDataJSON();
      if (body.command === "save_backup") return route.fulfill({ status: 200, contentType: "application/json", body: "null" });
      return route.continue();
    });
    await page.goto(url);
    await openCurrentMaster(page);
    await section(page).getByRole("button", { name: "Back up…" }).click();
    const form = page.locator("#backup-pass-form");
    await form.locator('[name="pass"]').fill(PASSPHRASE); await form.locator('[name="again"]').fill(PASSPHRASE);
    await page.locator("#backup-save").click();
    await assertCurrent(page, 2);
    assert.equal(await form.locator('[name="pass"]').inputValue(), PASSPHRASE, "the passphrase stays typed");
  },
  /** The core task: back up, wipe (a fresh install), restore, every host back. ≤ 4 decisions. */
  async "round-trip"(page, url, d) {
    await page.goto(url);
    await openCurrentMaster(page);
    await backUp(page, url, d);
    await page.request.post(url + "/scenario", { data: { flags: ["empty"] } }); // the new machine
    await page.reload();
    await page.locator("#restore-offer").waitFor();
    await page.locator("#restore-offer").getByRole("button", { name: "+ Add host" }).waitFor();
    await shots(page, "restore-offer");
    await restore(page, d, PASSPHRASE);
    await assertCurrent(page, 3);
    const facts = step(page, 3).locator("dl.facts");
    await facts.getByText("UI fixture Master").waitFor();
    assert.equal(await facts.locator("dt", { hasText: "Hosts" }).locator("xpath=following-sibling::dd[1]").textContent(), "1");
    await step(page, 3).getByText("Two Masters with the same keys must never run at once").waitFor();
    const run = page.locator("#restore-run");
    assert.ok(await run.isDisabled(), "no restore before the old machine is confirmed off");
    assert.ok(!(await calls(page, url)).some(c => c.method === "backup.restore"), "nothing restored yet");
    await shots(page, "restore-confirm");
    await page.locator("#restore-off").check(); // a confirmation, not a decision
    await run.click();
    await assertCurrent(page, 4);
    await step(page, 4).getByText("This machine is now UI fixture Master · 1 hosts").waitFor();
    await step(page, 4).locator("#keys-choice").getByText("Change keys (recommended)").waitFor();
    await shots(page, "restore-done");
    await step(page, 4).getByRole("button", { name: "Skip for now" }).click(); // skipping finishes the restore
    assert.equal(await flow(page).count(), 0, "Skip closes the flow");
    await page.locator("#dc-tree .dc-item[data-node='master:local']").click();
    await page.locator(".dc-main").getByText("UI test target").first().waitFor();
    assert.equal(await page.locator("#restore-offer").count(), 0);
  },
  /** Each wrong file is one plain sentence in place; a wrong passphrase keeps the file and asks again. */
  async "restore-errors"(page, url, d) {
    await page.goto(url);
    await openCurrentMaster(page);
    assert.ok(!(await section(page).getByRole("button", { name: "Restore from a backup…" }).isDisabled()), "an empty Master can restore");
    await section(page).getByRole("button", { name: "Restore from a backup…" }).click();
    const sentence = async (n, text) => step(page, n).locator(".add-check.bad", { hasText: text }).waitFor();
    await nextBackup(page, url, "other");
    await restore(page, d, "fixture passphrase", { start: false });
    await sentence(2, "This file is not an SSH Control Center backup · choose a .sshccbak file");
    for (const [kind, text] of [["newer", "This backup comes from a newer Master. Update this Master first"],
      ["damaged", "The passphrase is wrong, or the file is damaged · check the passphrase and try again"]]) {
      await nextBackup(page, url, kind);
      await step(page, 1).getByRole("button", { name: "Choose another file…" }).click();
      await page.locator("#restore-pass-form").getByRole("button", { name: "Open the backup" }).click();
      await sentence(2, text);
    }
    await nextBackup(page, url, "good");
    await step(page, 1).getByRole("button", { name: "Choose another file…" }).click();
    await page.locator('#restore-pass-form [name="pass"]').fill("not the passphrase");
    await page.locator("#restore-pass-form").getByRole("button", { name: "Open the backup" }).click();
    await sentence(2, "The passphrase is wrong");
    await step(page, 1).getByText("File: good.sshccbak").waitFor();
    await shots(page, "restore-wrong-passphrase");
    await page.locator('#restore-pass-form [name="pass"]').fill("fixture passphrase");
    await page.locator("#restore-pass-form").getByRole("button", { name: "Open the backup" }).click();
    await assertCurrent(page, 3);
    // A Master that gained hosts in the meantime refuses, in place.
    await page.route("**/rpc", async route => route.request().postDataJSON().args?.method === "backup.restore"
      ? route.fulfill({ status: 400, contentType: "application/json", body: JSON.stringify({ error: "E_RESTORE_NOT_EMPTY: this Master already has hosts; restore only works on a fresh install" }) })
      : route.continue());
    await page.locator("#restore-off").check();
    await page.locator("#restore-run").click();
    await sentence(3, "This Master already has hosts. Restore only works on a fresh install");
  },
  /** After a restore: Masters still using the old address, and the two-way Merge to enable again. */
  async "follow-ups"(page, url, d) {
    await page.goto(url);
    await openCurrentMaster(page);
    await backUp(page, url, d);
    await page.request.post(url + "/scenario", { data: { flags: ["empty"] } });
    await page.reload();
    await restore(page, d, PASSPHRASE);
    await page.locator("#restore-off").check(); await page.locator("#restore-run").click();
    const after = page.locator("#restore-follow-ups");
    await after.waitFor();
    await after.getByText("Update this Master’s address on Peer Master").waitFor();
    await after.getByText("Update this Master’s address on Branch Master").waitFor();
    assert.equal(await after.getByText("Relay-only Master").count(), 0, "a Master with merging disabled no longer reaches this one");
    await after.getByText("Two-way needs enabling again").waitFor();
    await after.locator('[data-two-way="branch-master"] [data-action="open-two-way"]').waitFor();
    await shots(page, "restore-follow-ups");
  },
  /** Change keys after a restore: a confirmation, live progress, then "back up now". */
  async "change-keys"(page, url, d) {
    await page.goto(url);
    await openCurrentMaster(page);
    await backUp(page, url, d);
    await page.request.post(url + "/scenario", { data: { flags: ["empty"] } });
    await page.reload();
    await restore(page, d, PASSPHRASE);
    await page.locator("#restore-off").check(); await page.locator("#restore-run").click();
    const before = d.count;
    await step(page, 4).getByRole("button", { name: "Change keys" }).click(); // a confirmation, not a decision
    assert.equal(d.count, before);
    assert.ok((await calls(page, url)).some(c => c.method === "keys.rotate"));
    await page.locator("#keys-progress").getByText(/hosts? still accepts? the old key/).first().waitFor();
    await page.locator("#keys-progress").getByText("Keys changed on every host").waitFor({ timeout: 15000 });
    await page.locator("#keys-progress").getByText("Old backups no longer work. Back up now").waitFor();
    await step(page, 4).getByRole("button", { name: "Back up…" }).waitFor();
    await shots(page, "keys-changed");
    await step(page, 4).getByRole("button", { name: "Open the host list" }).click();
  },
  /** Change keys later from the section: a host that can't change yet is named with the reason, merged hosts are
   *  left to their Master, and when every host moved: "Old backups no longer work. Back up now". */
  async "change-keys-later"(page, url, d) {
    await page.goto(url);
    await openCurrentMaster(page);
    await section(page).getByText("This Master was restored and still uses the old machine’s keys").waitFor();
    await section(page).getByRole("button", { name: "Change keys…" }).click();
    const progress = page.locator("#keys-progress");
    await progress.locator('[data-host="ui-test"]', { hasText: "Nobody is logged in as the Owner on UI test target" }).waitFor({ timeout: 15000 });
    await progress.getByText("Hosts still on the old key are tried again every minute, and when they come back online").waitFor();
    await progress.getByText("Child 1 of branch-master · Branch Master changes its keys").waitFor();
    await shots(page, "keys-trouble");
    await progress.getByText("Keys changed on every host").waitFor({ timeout: 20000 });
    await progress.getByText("Old backups no longer work. Back up now").waitFor();
    await section(page).locator("#backup-last", { hasText: "Not backed up since keys were changed" }).waitFor();
    assert.equal(await section(page).getByRole("button", { name: "Change keys…" }).count(), 0, "no Change keys once done");
    await shots(page, "keys-back-up-now");
    await backUp(page, url, d);
    await step(page, 3).getByRole("button", { name: "Done" }).click();
    await section(page).locator("#backup-last", { hasText: /^Last backup: / }).waitFor();
    assert.equal(await page.locator("#keys-progress").count(), 0, "back up now is gone after a backup");
  },
};
const scenarios = { "back-up": [], "backed-up": ["backed-up"], "save-cancelled": [], "round-trip": [], "restore-errors": ["empty"], "follow-ups": ["masters-page", "two-way-merge"], "change-keys": [], "change-keys-later": ["masters-page", "restored", "rotation-trouble"] };

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
    fs.writeFileSync(path.join(artifacts, "backup-results.json"), JSON.stringify({ ok: true, gate: MAX_DECISIONS, results }, null, 2) + "\n");
    console.log(JSON.stringify({ ok: true }));
  } finally { await browser.close(); }
})().catch(e => { console.error(e); process.exitCode = 1; });
