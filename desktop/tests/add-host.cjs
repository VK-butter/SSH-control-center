// Walks the one-column add flow ("+ Add host") on the fixture and gates on the decision count (≤ 2).
// Also checks: the review card (nothing saved before Finish, edits saved), the attention + retry path, step marking and focus, plain errors in place, one check per pasted value, the address as the
// default name, "Add host here…" on a folder, unfinished requests, and no "target" noun on screen. With no SSH answer the OS is
// unknown: both installers are saved, the first one used wins, and the other one's code says it was replaced.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");
const { decisions, startFixture } = require("./decisions.cjs");
const artifacts = path.resolve(__dirname, "../../artifacts/add-host-ui"); fs.mkdirSync(artifacts, { recursive: true });
const ADDRESS = "192.0.2.31", MAX_DECISIONS = 2;
const calls = async (page, url) => (await (await page.request.get(url + "/calls")).json());
const rpcCalls = (sent, method) => sent.filter(c => c.command === "rpc" && c.method === method);
const codeFor = async (page, url, query = "") => (await (await page.request.get(url + "/pairing-code" + query)).json()).code;
const column = page => page.locator("#add-host");
const step = (page, n) => column(page).locator('.add-step[data-step="' + n + '"]');

/** Exactly one step is current: it has aria-current="step", and only its number is acid. */
async function assertCurrent(page, n) {
  await step(page, n).and(page.locator('[aria-current="step"]')).waitFor();
  assert.equal(await column(page).locator('[aria-current="step"]').count(), 1, "one current step");
  // One evaluate per read: the column can re-render between a locator's query and its evaluateAll (detached → "").
  const { acid, colors } = await page.evaluate(() => {
    const probe = document.createElement("i"); probe.style.color = "var(--acid)"; document.body.append(probe); const acid = getComputedStyle(probe).color; probe.remove();
    return { acid, colors: [...document.querySelectorAll("#add-host .add-step .add-n")].map(el => getComputedStyle(el).color) };
  });
  assert.deepEqual(colors.map(c => c === acid), [1, 2, 3, 4].map(i => i === n), "acid only on step " + n + ": " + colors.join(" | "));
  // DESIGN.md rule 8: done/later steps are dimmed with colour tokens, never opacity (text stays ≥ --lilac contrast).
  const faded = await page.evaluate(() => [...document.querySelectorAll("#add-host .add-step, #add-host .add-step *")].filter(el => !el.matches(":disabled") && parseFloat(getComputedStyle(el).opacity) < 1).length);
  assert.equal(faded, 0, "no step dimmed with opacity");
}
async function assertNoTargetNoun(page, where) {
  const text = await column(page).innerText();
  assert.doesNotMatch(text, /\btargets?\b/i, "no on-screen 'target' in " + where);
}
const focusedId = page => page.evaluate(() => document.activeElement?.id || document.activeElement?.tagName);

/** Steps 1–3 as a Newcomer does them: type the address, Save installer…, paste the code. Ends on the review card. */
async function addOneHost(page, url, d, { folderId } = {}) {
  await assertCurrent(page, 1);
  assert.equal(await focusedId(page), "add-address", "focus starts in the address box");
  if (folderId) assert.equal(await column(page).locator('[name="folder"]').inputValue(), folderId, "folder pre-selected");
  await assertNoTargetNoun(page, "step 1");
  assert.equal(await column(page).locator('[name="os"]').isVisible(), false, "no OS choice on the main path");
  await column(page).getByText("Advanced").click();
  await assertNoTargetNoun(page, "step 1 with Advanced open");
  await d.fill(page.locator("#add-address"), ADDRESS, "address");
  // A fresh Windows machine has no SSH server yet: a calm note, and the flow goes on.
  await page.locator("#add-address-check").filter({ hasText: "the installer turns SSH on" }).waitFor();
  await page.getByRole("button", { name: "Save installer…" }).click();
  await assertCurrent(page, 3);
  assert.equal(await focusedId(page), "add-code", "focus moves to the code box");
  // OS unknown: both installers, named in one line.
  await step(page, 2).filter({ hasText: /Open sshcc-client-\S+\.exe \(Windows\) or sshcc-client-\S+\.deb \(Ubuntu\) on the new host, whichever matches it, and click Approve/ }).waitFor();
  assert.equal(await step(page, 2).locator(".add-n").innerText(), "✓");
  assert.equal(await page.locator('#add-host button[type="submit"]').count(), 0, "no submit button in steps 2–3");
  await assertNoTargetNoun(page, "step 3");
  // On the new host: open the installer, Approve, copy the code (simulated by the fixture).
  const code = await codeFor(page, url);
  await d.fill(page.locator("#add-code"), code, "pairing code");
  // A good code is confirmed on step 3 for a moment (host name included) before the review card opens.
  await page.locator("#add-code-check.good").filter({ hasText: /Code accepted · this is \S+/ }).waitFor();
  assert.equal(await step(page, 3).getAttribute("aria-current"), "step", "still on step 3 while the confirmation shows");
  const codeBox = () => page.evaluate(() => { const r = document.querySelector('#add-host .add-step[data-step="3"] textarea').getBoundingClientRect(); return [Math.round(r.top + scrollY), Math.round(r.height)]; });
  const before = await codeBox();
  await page.locator("#finish-pairing").waitFor();
  // The pasted code stays in place, read only: step 3 does not collapse or move.
  const pasted = step(page, 3).locator("textarea");
  assert.equal(await pasted.inputValue(), code); assert.ok(await pasted.evaluate(el => el.readOnly), "read only once done");
  const after = await codeBox(); assert.deepEqual(after, before, "the code box keeps its place and size: " + before + " → " + after);
  await assertCurrent(page, 4);
  const card = step(page, 4);
  assert.match(await card.innerText(), /SHA256:/, "review card shows the host key fingerprint");
  assert.equal(await card.locator('[name="label"]').inputValue(), ADDRESS, "shown-as name defaults to the address");
  assert.equal(rpcCalls(await calls(page, url), "pairing.complete").length, 0, "nothing saved before Finish");
  assert.equal((await rpcCall(page, url, "targets.list")).filter(t => t.routes[0].address === ADDRESS).length, 0, "no host saved before Finish");
}
const rpcCall = async (page, url, method, params = {}) => (await (await page.request.post(url + "/rpc", { data: { command: "rpc", args: { method, params } } })).json());
const newHostId = async (page, url) => (await rpcCall(page, url, "targets.list")).find(t => t.routes[0]?.address === ADDRESS)?.id;
/** Stands in for "Add host…" on a datacenter (same action and data as that menu item). */
const addFromSite = (page, site) => page.evaluate(id => { const b = document.createElement("button"); b.dataset.action = "dc-add"; b.dataset.site = id; document.body.append(b); b.click(); b.remove(); }, site);
/** Step 4: Finish completes the pairing; the result shows in the same card. */
async function finish(page, result = ".ready-message") {
  await step(page, 4).getByRole("button", { name: "Finish" }).click();
  await step(page, 4).locator(result).waitFor();
  await assertCurrent(page, 4);
}

async function shots(page, name) {
  for (const width of [1180, 820]) { await page.setViewportSize({ width, height: 900 }); await page.screenshot({ path: path.join(artifacts, "add-host-" + name + "-" + width + ".png"), fullPage: true }); }
  await page.setViewportSize({ width: 1180, height: 900 });
}

const checks = {
  /** Core task: one Windows host from an empty Master; the name is left empty. */
  async "empty"(page, url, d) {
    await page.goto(url);
    const add = page.locator('[data-action="add-host"]');
    assert.equal(await add.getAttribute("aria-haspopup"), null, "no dropdown");
    await add.click();
    await addOneHost(page, url, d);
    await assertNoTargetNoun(page, "step 4 review");
    await page.locator('[data-action="dc-toggle"][data-node="master:local"]').click();
    assert.equal(await page.locator("#dc-tree .dc-item").filter({ hasText: ADDRESS }).count(), 0, "the tree gains no host before Finish");
    await shots(page, "review");
    await finish(page);
    await assertNoTargetNoun(page, "step 4 result");
    const sent = await calls(page, url);
    assert.equal(rpcCalls(sent, "pairing.create").length, 1);
    assert.equal(rpcCalls(sent, "pairing.create")[0].params.address, ADDRESS);
    assert.equal(rpcCalls(sent, "pairing.create")[0].params.label, ADDRESS, "empty name is sent as the address");
    assert.equal(rpcCalls(sent, "pairing.create")[0].params.os, "unknown", "no SSH answer: the OS is never asked");
    assert.equal(sent.filter(c => c.command === "save_setup").length, 1);
    assert.equal(rpcCalls(sent, "pairing.preview").length, 1);
    assert.equal(rpcCalls(sent, "pairing.complete").length, 1);
    await page.locator("#dc-tree .dc-item").filter({ hasText: ADDRESS }).waitFor();
    await step(page, 4).getByRole("button", { name: "Open this host" }).click();
    await page.waitForFunction(() => !document.querySelector("#add-host"));
    await page.locator(".page-heading h1").filter({ hasText: ADDRESS }).waitFor();
  },
  /** Name and folder changed on the review card are what get saved. */
  async "review-edits"(page, url, d) {
    const office = await rpcCall(page, url, "folders.create", { name: "Office" });
    const lab = await rpcCall(page, url, "folders.create", { name: "Lab" });
    await page.goto(url); await page.locator('[data-action="add-host"]').click();
    await addOneHost(page, url, d);
    const card = step(page, 4);
    await card.locator('[name="label"]').fill("Accounting PC");
    await card.locator('[name="folder"]').selectOption(lab.id);
    await finish(page);
    const sent = await calls(page, url);
    assert.equal(rpcCalls(sent, "pairing.complete")[0].params.label, "Accounting PC");
    const assigned = rpcCalls(sent, "folders.assign");
    assert.equal(assigned.at(-1).params.folderId, lab.id);
    const saved = (await rpcCall(page, url, "targets.list")).find(t => t.routes[0].address === ADDRESS);
    assert.equal(saved.label, "Accounting PC");
    assert.equal((await rpcCall(page, url, "folders.list")).members[saved.id], lab.id);
    assert.ok(office.id);
  },
  /** The connection check fails: failures in plain words and the address-only retry, with no new installer. */
  async "attention-retry"(page, url, d) {
    await page.goto(url); await page.locator('[data-action="add-host"]').click();
    await addOneHost(page, url, d);
    await finish(page, "#retry-pairing");
    const card = step(page, 4);
    await card.filter({ hasText: "Installed, but the connection check failed" }).waitFor();
    await card.locator("li").first().waitFor();
    await shots(page, "attention");
    await card.locator('#retry-pairing [name="address"]').fill("192.0.2.32");
    await card.getByRole("button", { name: "Check again" }).click();
    await card.locator(".ready-message").waitFor();
    const sent = await calls(page, url);
    assert.equal(rpcCalls(sent, "pairing.status").filter(c => c.params.routes?.[0]?.address === "192.0.2.32").length, 1);
    assert.equal(sent.filter(c => c.command === "save_setup").length, 1, "no new installer");
  },
  /** The new host is a Master: Merged with no extra click; its hosts show in the tree; Two-way works from the card. */
  async "empty+new-host-master"(page, url, d) {
    // Two-way is offered only on Windows Masters today.
    await page.addInitScript(() => Object.defineProperty(navigator, "userAgent", { get: () => "Mozilla/5.0 (Windows NT 10.0; Win64; x64)" }));
    await page.goto(url); await page.locator('[data-action="add-host"]').click();
    await addOneHost(page, url, d);
    await finish(page);
    const card = step(page, 4), master = card.locator(".add-master");
    await master.filter({ hasText: "This host is a Master · 3 hosts merged" }).waitFor();
    assert.match(await master.innerText(), /see and control its hosts from here/, "Merge gloss");
    assert.doesNotMatch(await column(page).innerText(), /is also a Master/, "no old offer");
    assert.equal(await card.locator('[data-action="merge-master"]').count(), 0, "no Merge click needed");
    const sent = await calls(page, url), id = await newHostId(page, url);
    assert.deepEqual(rpcCalls(sent, "merges.discover").map(c => c.params), [{ targetId: id }], "only the new host is asked");
    assert.deepEqual(rpcCalls(sent, "merges.add").map(c => c.params), [{ targetId: id }]);
    assert.equal(rpcCalls(sent, "datacenters.assign").length, 0, "not filed anywhere");
    // Its hosts are in the tree.
    await page.locator('[data-action="dc-toggle"][data-node="master:' + id + '"]').click();
    await page.locator("#dc-tree .dc-item").filter({ hasText: "Child 1 of " + id }).waitFor();
    await shots(page, "master-merged");
    // Two-way is optional and goes through today's confirm dialog.
    await master.getByRole("button", { name: "Two-way" }).click();
    await page.locator("#two-way-dialog[open]").waitFor();
    await page.locator("#two-way-dialog").getByRole("button", { name: "Install and enable two-way" }).click();
    await master.locator(".badge", { hasText: /^Two-way$/ }).waitFor();
    assert.equal((await calls(page, url)).filter(c => c.command === "merge_two_way" && c.targetId === id).length, 1);
    assert.equal(await master.getByRole("button", { name: "Two-way" }).count(), 0, "no Two-way once enabled");
  },
  /** Opened from a datacenter: a Master is filed under it. */
  async "datacenter+new-host-master"(page, url, d) {
    const site = await rpcCall(page, url, "datacenters.create", { name: "Bangkok" });
    await page.goto(url); await addFromSite(page, site.id);
    await addOneHost(page, url, d);
    await finish(page);
    await step(page, 4).locator(".add-master").filter({ hasText: "hosts merged" }).waitFor();
    const id = await newHostId(page, url);
    assert.deepEqual(rpcCalls(await calls(page, url), "datacenters.assign").map(c => c.params), [{ master: id, datacenterId: site.id }]);
  },
  /** Opened from a datacenter, but the new host is not a Master: not filed, no error, no extra message. */
  async "datacenter-not-master"(page, url, d) {
    const site = await rpcCall(page, url, "datacenters.create", { name: "Bangkok" });
    await page.goto(url); await addFromSite(page, site.id);
    await addOneHost(page, url, d);
    await finish(page);
    await page.waitForTimeout(300);
    const sent = await calls(page, url);
    assert.equal(rpcCalls(sent, "merges.discover").length, 1);
    assert.equal(rpcCalls(sent, "merges.add").length, 0);
    assert.equal(rpcCalls(sent, "datacenters.assign").length, 0, "a non-Master is not filed");
    assert.equal(await step(page, 4).locator(".add-master, .callout.bad").count(), 0, "no Master line, no error");
    assert.doesNotMatch(await step(page, 4).innerText(), /not a Master/);
  },
  /** The Merge fails: plain words in the card, the host stays added, Merge is offered on the Masters page. */
  async "merge-fails"(page, url, d) {
    await page.goto(url); await page.locator('[data-action="add-host"]').click();
    await addOneHost(page, url, d);
    await finish(page);
    const master = step(page, 4).locator(".add-master");
    await master.filter({ hasText: "This host is a Master, but it could not be merged" }).waitFor();
    assert.doesNotMatch(await master.innerText(), /E_MERGE/, "plain words, no error code");
    await shots(page, "merge-failed");
    assert.ok(await newHostId(page, url), "the host stays added");
    await master.getByRole("button", { name: "Open Masters" }).click();
    const outgoing = page.locator("section", { has: page.locator("h2", { hasText: /^Controlled from here$/ }) });
    await outgoing.locator("tr", { hasText: ADDRESS }).locator('[data-action="merge-master"]').waitFor();
  },
  /** Wrong codes: a plain sentence in place, the code stays, one request per distinct value. */
  async "wrong-codes"(page, url) {
    await page.goto(url); await page.locator('[data-action="add-host"]').click();
    await page.locator("#add-address").fill(ADDRESS);
    await page.getByRole("button", { name: "Save installer…" }).click();
    await assertCurrent(page, 3);
    const box = page.locator("#add-code"), check = page.locator("#add-code-check");
    const other = await codeFor(page, url, "?master=other");
    await box.fill(other);
    await check.filter({ hasText: "different Master" }).waitFor();
    await box.fill(other); await page.waitForTimeout(800);
    assert.equal(await box.inputValue(), other, "code is not cleared on error");
    await box.fill("SSHCC1.fixture.damaged");
    await check.filter({ hasText: "damaged or incomplete" }).waitFor();
    assert.doesNotMatch(await check.innerText(), /E_PAIRING/, "plain words, no error code");
    assert.equal(await page.locator('#add-host button[type="submit"]').count(), 0);
    const sent = rpcCalls(await calls(page, url), "pairing.preview");
    assert.equal(sent.filter(c => c.params.code === other).length, 1, "same value checked once");
    assert.equal(rpcCalls(await calls(page, url), "pairing.complete").length, 0, "checking saves nothing");
    await assertCurrent(page, 3);
    await shots(page, "wrong-code");
    // Leave it unfinished, then come back: the request is listed with Resume / Cancel.
    await column(page).getByRole("button", { name: "Close" }).click();
    await page.locator('[data-action="add-host"]').click();
    const row = column(page).locator(".add-unfinished tr").filter({ hasText: ADDRESS });
    assert.equal(await row.count(), 1, "a sibling pair is one unfinished row");
    assert.match(await row.innerText(), /Windows or Ubuntu/);
    await row.getByRole("button", { name: "Resume" }).click();
    await assertCurrent(page, 3);
    page.once("dialog", dialog => dialog.accept());
    await column(page).locator(".add-unfinished").getByRole("button", { name: "Cancel" }).click();
    await page.waitForFunction(() => !document.querySelector(".add-unfinished"));
    assert.equal(rpcCalls(await calls(page, url), "pairing.cancel").length, 2, "cancelling the row cancels both installers' requests");
    assert.deepEqual(await rpcCall(page, url, "pairing.status"), []);
    await assertCurrent(page, 1);
  },
  /** "Add host here…" on a folder pre-selects and applies it; context menus say "Add host…". */
  async "folder"(page, url, d) {
    const folder = await (await page.request.post(url + "/rpc", { data: { command: "rpc", args: { method: "folders.create", params: { name: "Office" } } } })).json();
    await page.goto(url);
    await page.locator('.dc-item[data-node="master:local"]').click({ button: "right" });
    const items = await page.locator("#dc-menu .ctx-item").allTextContents();
    assert.ok(items.includes("Add host…"), "master menu offers Add host…");
    assert.ok(!items.some(text => /target|Master host/i.test(text)), "no separate Target/Master items: " + items);
    await page.keyboard.press("Escape");
    await page.locator('[data-action="dc-toggle"][data-node="master:local"]').click();
    await page.locator('.dc-item[data-node="folder:' + folder.id + '"]').click({ button: "right" });
    await page.getByRole("menuitem", { name: "Add host here…" }).click();
    await addOneHost(page, url, d, { folderId: folder.id });
    assert.equal(await step(page, 4).locator('[name="folder"]').inputValue(), folder.id, "review card keeps the folder");
    await finish(page);
    const assigned = rpcCalls(await calls(page, url), "folders.assign");
    assert.equal(assigned.length, 1); assert.equal(assigned[0].params.folderId, folder.id);
  },
  /** Step 1 live check: detected OS, duplicates of a host or a request, and late answers ignored. */
  async "live-check"(page, url) {
    await page.goto(url); await page.locator('[data-action="add-host"]').click();
    const box = page.locator("#add-address"), line = page.locator("#add-address-check"), os = page.locator('#create-pairing [name="os"]');
    const save = page.getByRole("button", { name: "Save installer…" });
    // An answer for an earlier value that arrives late is never shown.
    await box.fill("192.0.2.99"); await page.waitForTimeout(600);
    await box.fill("192.0.2.40");
    await line.filter({ hasText: "the installer turns SSH on" }).waitFor();
    await page.waitForTimeout(1500);
    assert.match(await line.innerText(), /the installer turns SSH on/, "late answer ignored");
    assert.equal(await os.inputValue(), "", "the OS choice under Advanced stays on Detect");
    assert.equal(rpcCalls(await calls(page, url), "pairing.check").length, 2, "one check per pause");
    // A known OS fills the OS choice.
    await box.fill("192.0.2.32");
    await line.filter({ hasText: "SSH answers · Ubuntu or Debian" }).waitFor();
    await box.fill("192.0.2.33");
    await line.filter({ hasText: "SSH answers · Windows" }).waitFor();
    assert.ok(await save.isEnabled());
    await shots(page, "live-check-os");
    // An existing host: "Already added as …", Save installer disabled, a link to that host.
    await box.fill("192.0.2.20");
    await line.filter({ hasText: "Already added as UI test target" }).waitFor();
    assert.ok(await save.isDisabled(), "duplicate disables Save installer");
    await shots(page, "live-check-duplicate");
    await box.fill("192.0.2.33");
    await line.filter({ hasText: "SSH answers" }).waitFor();
    assert.ok(await save.isEnabled(), "enabled again for a new address");
    // An open request for the address: same message, with Resume.
    await page.request.post(url + "/rpc", { data: { command: "rpc", args: { method: "pairing.create", params: { label: "Lab box", tenant: "lab", os: "linux", address: "192.0.2.50", port: 22 } } } });
    await box.fill("192.0.2.50");
    await line.filter({ hasText: "Already added as Lab box" }).waitFor();
    assert.ok(await save.isDisabled());
    await line.getByRole("button", { name: "Resume it" }).click();
    await column(page).locator(".facts").filter({ hasText: "Lab box" }).waitFor();
    // Back to a fresh form, then follow the link to the existing host.
    page.once("dialog", dialog => dialog.accept());
    await column(page).getByRole("button", { name: "Cancel request" }).click();
    await assertCurrent(page, 1);
    await page.locator("#add-address").fill("192.0.2.20");
    await line.getByRole("button", { name: "Open it" }).click();
    await page.waitForFunction(() => !document.querySelector("#add-host"));
  },
  /** A detected OS makes one request and one installer. */
  async "detected-os"(page, url) {
    await page.goto(url); await page.locator('[data-action="add-host"]').click();
    await page.locator("#add-address").fill("192.0.2.32");
    await page.locator("#add-address-check").filter({ hasText: "SSH answers · Ubuntu or Debian" }).waitFor();
    await page.getByRole("button", { name: "Save installer…" }).click();
    await assertCurrent(page, 3);
    await step(page, 2).filter({ hasText: /Open sshcc-client-\S+\.deb on the new host and click Approve/ }).waitFor();
    const sent = await calls(page, url);
    assert.equal(rpcCalls(sent, "pairing.create")[0].params.os, "linux");
    assert.equal((await rpcCall(page, url, "pairing.status")).length, 1, "one request");
  },
  /** Save installer… pressed before the (slow) check answered: the check runs first and its OS is used. */
  async "save-before-check"(page, url) {
    await page.goto(url); await page.locator('[data-action="add-host"]').click();
    await page.locator("#add-address").fill("192.0.2.99");
    await page.getByRole("button", { name: "Save installer…" }).click();
    await assertCurrent(page, 3);
    assert.equal(rpcCalls(await calls(page, url), "pairing.create")[0].params.os, "linux", "the detected OS is used");
    assert.equal((await rpcCall(page, url, "pairing.status")).length, 1, "one request");
  },
  /** Save installer… pressed before the (slow) check answered for a duplicate: still refused. */
  async "save-before-check-duplicate"(page, url) {
    await rpcCall(page, url, "pairing.create", { label: "Lab box", tenant: "lab", os: "linux", address: "192.0.2.99", port: 22 });
    await page.goto(url); await page.locator('[data-action="add-host"]').click();
    await page.locator("#add-address").fill("192.0.2.99");
    const save = page.getByRole("button", { name: "Save installer…" });
    await save.click();
    await page.locator("#add-address-check").filter({ hasText: "Already added as Lab box" }).waitFor();
    assert.equal(rpcCalls(await calls(page, url), "pairing.create").length, 1, "no second request");
    assert.ok(await save.isDisabled(), "Save installer stays disabled");
    await assertCurrent(page, 1);
  },
  /** The other installer was used first: its code says so in plain words, in place. */
  async "replaced"(page, url) {
    await page.goto(url); await page.locator('[data-action="add-host"]').click();
    await page.locator("#add-address").fill(ADDRESS);
    await page.getByRole("button", { name: "Save installer…" }).click();
    await assertCurrent(page, 3);
    const requests = await rpcCall(page, url, "pairing.status");
    const [windows, linux] = ["windows", "linux"].map(os => requests.find(row => row.os === os));
    assert.equal(windows.sibling, linux.requestId);
    const linuxCode = await codeFor(page, url, "?requestId=" + linux.requestId);
    // The Windows installer was used and its code pasted already (e.g. in another window).
    await rpcCall(page, url, "pairing.complete", { code: await codeFor(page, url, "?requestId=" + windows.requestId) });
    await page.locator("#add-code").fill(linuxCode);
    await page.locator("#add-code-check").filter({ hasText: "This installer was replaced by the other one you already used" }).waitFor();
    await assertCurrent(page, 3);
    await shots(page, "replaced");
  },
  /** Reduced motion: steps never animate. */
  async "reduced-motion"(page, url) {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.goto(url); await page.locator('[data-action="add-host"]').click();
    await assertCurrent(page, 1);
    const motion = await page.evaluate(() => [...document.querySelectorAll("#add-host .add-step, #add-host .add-step *")].filter(el => { const s = getComputedStyle(el); return s.animationName !== "none" || s.transitionDuration.split(",").some(v => parseFloat(v) > 0); }).length);
    assert.equal(motion, 0, "nothing animates");
  },
};
const scenarios = { "empty": ["empty"], "empty+new-host-master": ["empty", "new-host-master"], "datacenter+new-host-master": ["empty", "new-host-master"], "datacenter-not-master": ["empty"], "merge-fails": ["empty", "new-host-master", "merge-fails"], "review-edits": ["empty"], "attention-retry": ["empty", "needs-attention"], "wrong-codes": ["empty"], "folder": ["empty"], "live-check": [], "detected-os": ["empty"], "save-before-check": ["empty"], "save-before-check-duplicate": ["empty"], "replaced": ["empty"], "reduced-motion": ["empty"] };

(async () => {
  const browser = await chromium.launch({ headless: true }); const results = [];
  try {
    for (const [name, flags] of Object.entries(scenarios)) {
      const fixture = await startFixture(flags);
      try {
        const page = await browser.newPage({ viewport: { width: 1180, height: 900 } });
        const d = decisions();
        await checks[name](page, fixture.url, d);
        if (d.count) {
          await shots(page, name);
          console.log("[" + name + "] decisions: " + d.count + "\n" + d.summary());
          assert.ok(d.count <= MAX_DECISIONS, name + ": " + d.count + " decisions, target ≤ " + MAX_DECISIONS);
        }
        console.log("ok " + name);
        results.push({ scenario: name, decisions: d.count, log: d.log });
        await page.close();
      } finally { fixture.stop(); }
    }
    fs.writeFileSync(path.join(artifacts, "add-host-results.json"), JSON.stringify({ ok: true, gate: MAX_DECISIONS, results }, null, 2) + "\n");
    console.log(JSON.stringify({ ok: true, decisions: results[0].decisions }));
  } finally { await browser.close(); }
})().catch(e => { console.error(e); process.exitCode = 1; });
