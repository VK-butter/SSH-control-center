// Language EN | ไทย on the fixture (plain Chromium): the default follows the browser (OS) language, one click on ไทย
// switches the open page (1 decision) and survives a reload, switching keeps the page, the selected host and typed
// input, every page and dialog in Thai has no untranslated English (visible text plus aria-label, title and
// placeholder; Master, Tag, Merge, SSH, fixture values and other technical words are allowed), a mapped error shows
// the Thai sentence with the Master's English under it while an unmapped one shows English alone, <html lang>
// follows the switch, and every EN string key has a TH key. Thai screenshots at 1180px and 820px.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");
const { decisions, startFixture } = require("./decisions.cjs");
const artifacts = path.resolve(__dirname, "../../artifacts/language-ui"); fs.mkdirSync(artifacts, { recursive: true });
const WINDOWS_UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0 Safari/537.36";

// Words allowed in Thai: proper names that stay English (wording table), English glosses in parentheses, and
// technical words that are the same in Thai IT use. Everything that comes from the fixture is added at run time.
const ALLOWED = new Set(`Master Masters Tag Merge SSH SSHCC Audit log CSV Approve Owner SSH Control Center CONTROL HOST KEY EN
  AI ID OS IP CPU JSON CLI MCP LAN Tailscale Windows Server Ubuntu Debian Linux Desktop ProxyJump hostKeys identityFile
  PowerShell Bash Shell stdout stderr UTF SHA B KB MB GB TB GiB KiB Node Host key host Bridge session Session Terminal
  Repository WebView tray named pipe Unix socket Services SSHControlCenter Cursor Claude Code Codex docker podman path
  client server Service Refresh MEM Enter Ctrl C v swap root array object OpenSSH sshcc bridge authorized keys Ctrl e g`.split(/\s+/));

const rpc = async (page, url, method, params = {}) => (await (await page.request.post(url + "/rpc", { data: { command: "rpc", args: { method, params } } })).json());
const scenario = (page, url, flags) => page.request.post(url + "/scenario", { data: { flags } });
const codeFor = async (page, url, query = "") => (await (await page.request.get(url + "/pairing-code" + query)).json()).code;
/** Every word in the fixture's own data (host names, IDs, labels…): shown as is in both languages. */
async function fixtureWords(page, url) {
  const values = [];
  const walk = value => { if (typeof value === "string") values.push(value); else if (value && typeof value === "object") Object.values(value).forEach(walk); };
  for (const [method, params] of [["system.status"], ["targets.list"], ["merges.list"], ["masters.list"], ["relays.list"], ["folders.list"], ["files.list", { targetId: "ui-test", path: "~" }], ["targets.doctor", { targetId: "ui-test" }], ["pairing.status"]])
    walk(await rpc(page, url, method, params || {}).catch(() => null));
  return values.flatMap(value => value.match(/[A-Za-z][A-Za-z'’]*/g) || []);
}
/** Latin words on screen that are not allowed, with where they are. Skips code, logs, raw service text and the
 *  English detail line under translated errors. Opens every <details> first so folded text is checked too. */
async function untranslated(page, extra = []) {
  await page.evaluate(() => document.querySelectorAll("details").forEach(details => { details.open = true; }));
  return page.evaluate(allowed => {
    const ok = new Set(allowed), skip = "code, pre, textarea, kbd, script, style, svg, .mono, .copy-row, [data-raw], .error-detail, #app-version";
    const found = new Map();
    // Tokens with digits (ED25519, SSHCC1, v1) are identifiers, not words.
    const note = (text, where) => { for (const word of String(text).match(/[A-Za-z][A-Za-z0-9'’]*/g) || []) if (!/\d/.test(word) && !ok.has(word)) found.set(word, where); };
    const shown = element => element.checkVisibility({ visibilityProperty: true });
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    while (walker.nextNode()) {
      const element = walker.currentNode.parentElement;
      if (element && !element.closest(skip) && shown(element)) note(walker.currentNode.textContent, element.outerHTML.slice(0, 140));
    }
    for (const element of document.querySelectorAll("[aria-label], [title], [placeholder]"))
      if (!element.closest(skip) && shown(element)) for (const name of ["aria-label", "title", "placeholder"]) if (element.hasAttribute(name)) note(element.getAttribute(name), name + " on " + element.outerHTML.slice(0, 120));
    for (const option of document.querySelectorAll("option")) if (!option.closest(skip) && shown(option.parentElement)) note(option.textContent, "option " + option.outerHTML);
    return [...found].map(([word, where]) => word + "  ←  " + where);
  }, [...ALLOWED, ...extra]);
}
async function assertThai(page, where, extra) {
  const words = await untranslated(page, extra);
  assert.deepEqual(words, [], "untranslated English on " + where + ":\n" + words.join("\n"));
  console.log("ok thai " + where);
}
async function shots(page, name) {
  for (const width of [1180, 820]) {
    await page.setViewportSize({ width, height: 900 });
    await page.evaluate(() => document.fonts.ready);
    await page.screenshot({ path: path.join(artifacts, name + "-" + width + ".png"), fullPage: true });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), "no sideways scroll on " + name + " at " + width + " px");
  }
  await page.setViewportSize({ width: 1180, height: 900 });
}
// The first render after load replaces the main panel, so wait for it before clicking anything.
const ready = async page => { await page.locator("#service-label").filter({ hasNotText: /Connecting|กำลังเชื่อมต่อ/ }).waitFor(); await page.locator("#dc-panel").waitFor(); };
const open = async (page, url, options = {}) => { await page.goto(url); await ready(page); return page; };

(async () => {
  const browser = await chromium.launch({ headless: true }), fixture = await startFixture([]), url = fixture.url;
  const contexts = [];
  const newPage = async options => { const context = await browser.newContext({ viewport: { width: 1180, height: 900 }, ...options }); contexts.push(context); const page = await context.newPage(); page.on("dialog", dialog => dialog.accept()); return page; };
  try {
    // 1. The first run follows the OS language (the webview reports it as the browser language).
    for (const [locale, expected, add] of [["th-TH", "th", "+ เพิ่มเครื่อง"], ["en-US", "en", "+ Add host"]]) {
      const page = await open(await newPage({ locale }), url);
      assert.equal(await page.evaluate(() => document.documentElement.lang), expected, locale + " opens in " + expected);
      assert.equal(await page.locator(".add-host").textContent(), add);
      assert.equal(await page.locator('[data-lang="' + expected + '"]').getAttribute("aria-pressed"), "true", "the active language is marked");
    }
    console.log("ok default");

    // 2. One click switches, the choice is remembered, and it overrides the OS default.
    const page = await open(await newPage({ locale: "en-US" }), url), d = decisions();
    await d.choose(page.locator('.side-foot [data-lang="th"]'), "language ไทย");
    assert.equal(d.count, 1, "switching to Thai is 1 decision");
    assert.equal(await page.evaluate(() => document.documentElement.lang), "th");
    await page.locator(".add-host", { hasText: "+ เพิ่มเครื่อง" }).waitFor();
    await page.locator('[data-page="masters"]', { hasText: "เครื่องหลัก" }).waitFor();
    await page.reload(); await ready(page);
    assert.equal(await page.evaluate(() => document.documentElement.lang), "th", "the choice survives a reload (en-US browser)");
    await page.locator('.side-foot [data-lang="en"]').click();
    await page.locator(".add-host", { hasText: "+ Add host" }).waitFor();
    console.log("ok switch " + JSON.stringify(d.log));

    // 3. Switching keeps the page, the selected host, the open tab and what was typed.
    await page.locator(".add-host").click();
    await page.locator("#add-address").fill("192.0.2.31");
    await page.locator("#add-address-check", { hasText: "the installer turns SSH on" }).waitFor();
    await page.locator('.side-foot [data-lang="th"]').click();
    assert.equal(await page.locator("#add-address").inputValue(), "192.0.2.31", "typed address kept");
    await page.locator("#add-host h1", { hasText: "เพิ่มเครื่อง" }).waitFor();
    await page.locator("#add-address-check", { hasText: "ตัวติดตั้งจะเปิด SSH ให้" }).waitFor();
    await page.locator('.side-foot [data-lang="en"]').click();
    await page.locator('[data-action="enroll-close"]').click();
    await page.locator('#dc-tree .dc-toggle').first().click();
    await page.locator('#dc-tree .dc-item[data-node="node:ui-test"]').click();
    await page.locator('[data-action="dc-tab"][data-tab="files"]').click();
    await page.locator("#file-filter").fill("exam");
    await page.locator('.side-foot [data-lang="th"]').click();
    await page.locator(".page-heading h1", { hasText: "UI test target" }).waitFor();
    await page.locator('[data-action="dc-tab"][data-tab="files"][aria-selected="true"]', { hasText: "ไฟล์" }).waitFor();
    assert.equal(await page.locator("#file-filter").inputValue(), "exam", "typed filter kept");
    console.log("ok keeps state");

    // 4. Every page and dialog in Thai.
    const th = await open(await newPage({ locale: "th-TH", userAgent: WINDOWS_UA }), url);
    const words = await fixtureWords(th, url);
    await th.locator(".inv-summary").waitFor();
    await assertThai(th, "inventory", words);
    await shots(th, "inventory");
    await th.locator('#dc-tree .dc-toggle').first().click();
    await th.locator('#dc-tree .dc-item[data-node="master:local"]').click({ button: "right" });
    await th.locator("#dc-menu").waitFor(); await assertThai(th, "Master menu", words); await th.keyboard.press("Escape");
    await th.locator('#dc-tree .dc-item[data-node="node:ui-test"]').click({ button: "right" });
    await th.locator("#dc-menu").waitFor(); await assertThai(th, "host menu", words); await th.keyboard.press("Escape");
    await th.locator('#dc-tree .dc-item[data-node="master:local"]').click();
    await th.locator(".stat-grid").waitFor(); await assertThai(th, "current Master page", words);
    await th.locator('#dc-tree .dc-item[data-node="node:ui-test"]').click();
    await th.locator('[data-action="diagnose"]').first().click();
    await th.locator(".check-panel.is-ok").waitFor();
    await assertThai(th, "host summary + check", words);
    await shots(th, "host");
    for (const tab of ["jobs", "files", "containers", "monitoring"]) {
      await th.locator('[data-action="dc-tab"][data-tab="' + tab + '"]').click();
      await th.waitForTimeout(400);
      if (tab === "jobs") { await th.locator('[data-action="toggle-new-job"]').first().click(); await th.locator("#start-job").waitFor(); }
      // The fixture's partitions (/var, /boot/efi, ext4…) are host data, shown as is.
      await assertThai(th, "host " + tab, [...words, "var", "boot", "efi", "ext", "xfs", "vfat"]);
    }
    await th.locator('[data-action="dc-tab"][data-tab="files"]').click();
    await th.locator(".row-more").first().click();
    await th.locator("#dc-menu").waitFor(); await assertThai(th, "file menu", words);
    await th.locator('#dc-menu [data-action="file-delete"]').click();
    await th.locator("#delete-dialog[open]").waitFor();
    await assertThai(th, "delete dialog", words);
    await th.locator('[data-action="close-delete"]').click();
    await th.locator('.target-actions [data-action="open-shell"]').click();
    await th.locator("#shell-dialog[open]").waitFor();
    // Account and state come from the OS session list (fixture-owner · active / online): shown as reported.
    await assertThai(th, "session dialog", [...words, "fixture", "owner", "active", "online"]);
    await th.locator('[data-action="close-shell-dialog"]').click();
    await th.locator('[data-page="service"]').click();
    await th.locator(".stat-grid").waitFor(); await assertThai(th, "service", words);
    await shots(th, "service");

    await scenario(th, url, ["masters-page"]);
    await th.locator('[data-page="masters"]').click();
    await th.locator("h2", { hasText: "ถูกสั่งจากเครื่องนี้" }).waitFor();
    await th.locator("h2", { hasText: "เครื่องนี้ถูกสั่งโดย" }).waitFor();
    const mastersWords = [...words, ...await fixtureWords(th, url)];
    await assertThai(th, "Masters", mastersWords);
    await shots(th, "masters");
    await th.locator('[data-action="open-two-way"]').first().click();
    await th.locator("#two-way-dialog[open]").waitFor();
    await th.locator("#two-way-dialog h2", { hasText: "Branch Master" }).waitFor();
    await assertThai(th, "two-way dialog", mastersWords);
    await th.locator('[data-action="close-two-way-dialog"]').click();
    // Team sharing (entry 6): the grant controls with a Tag limit, a View-only host, and a refusal.
    await th.locator('tr[data-relay="peer-both"] [data-grant="scope"]').selectOption("tags");
    await th.locator('tr[data-relay="peer-both"] [data-grant="tags"]').waitFor();
    await assertThai(th, "Masters: grant controls", [...mastersWords, "web", "db"]);
    await shots(th, "grant-controls");
    await scenario(th, url, ["masters-page", "view-only-merge"]); await th.reload(); await ready(th);
    const shareWords = [...words, ...await fixtureWords(th, url)];
    for (let i = 0; i < 4; i++) { const closed = th.locator('#dc-tree .dc-toggle[aria-expanded="false"]'); if (!(await closed.count())) break; await closed.first().click(); }
    await th.locator('#dc-tree .dc-item[data-node="node:branch-master/child-2"]').click();
    await th.locator(".badge.view-only").first().waitFor();
    await assertThai(th, "View-only host", shareWords);
    await shots(th, "view-only-host");
    await scenario(th, url, ["masters-page", "merge-refuses"]); await th.reload(); await ready(th);
    for (let i = 0; i < 4; i++) { const closed = th.locator('#dc-tree .dc-toggle[aria-expanded="false"]'); if (!(await closed.count())) break; await closed.first().click(); }
    await th.locator('#dc-tree .dc-item[data-node="node:branch-master/child-2"]').click();
    await th.locator('[data-action="dc-tab"][data-tab="jobs"]').click();
    await th.locator('[data-action="toggle-new-job"]').first().click();
    await th.locator('#start-job [name="script"]').fill("uptime");
    await th.locator('#start-job [type="submit"]').click();
    await th.locator("#notice", { hasText: "Tag web" }).waitFor();
    await assertThai(th, "Merge Role refusal", [...shareWords, "web"]);
    // Multi-hop: a far host's path and right, the too-far note, a silent Master, and an outdated one.
    await scenario(th, url, ["masters-page", "multi-hop"]); await th.reload(); await ready(th);
    const hopWords = [...words, ...await fixtureWords(th, url), "Lab", "web", "db"];
    for (let i = 0; i < 8; i++) { const closed = th.locator('#dc-tree .dc-toggle[aria-expanded="false"]'); if (!(await closed.count())) break; await closed.first().click(); }
    await th.locator('#dc-tree .dc-item[data-node="node:branch-master/lab-master/lab-db"]').click();
    await th.locator(".live-label", { hasText: "เส้นทาง:" }).waitFor();
    await assertThai(th, "far host", hopWords);
    await shots(th, "far-host");
    await th.locator('#dc-tree .dc-item[data-node="node:branch-master/lab-master/lab-web"]').click();
    await th.locator('[data-action="dc-tab"][data-tab="jobs"]').click();
    await th.locator('[data-action="toggle-new-job"]').first().click();
    await th.locator('#start-job [name="script"]').fill("uptime");
    await th.locator('#start-job [type="submit"]').click();
    await th.locator("#notice", { hasText: "ไม่ตอบในเวลาที่กำหนด" }).waitFor();
    await assertThai(th, "a Master on the way didn't answer", hopWords);
    await th.locator('.nav[data-page="masters"]').click();
    await th.locator("#beyond-note").waitFor();
    await assertThai(th, "hosts beyond 4 hops", hopWords);
    await scenario(th, url, ["masters-page", "outdated-merge"]); await th.reload(); await ready(th);
    await th.locator('.nav[data-page="masters"]').click();
    await th.locator(".badge", { hasText: "รุ่นเก่า" }).waitFor();
    await assertThai(th, "outdated Master", [...words, ...await fixtureWords(th, url)]);
    // Target updates (entry 8): badges, host-page controls, the Inventory panel with a failed host.
    await scenario(th, url, ["masters-page", "outdated-hosts", "upgrade-fails"]); await th.reload(); await ready(th);
    const upWords = [...words, ...await fixtureWords(th, url)];
    await th.locator("#upgrade-panel").waitFor();
    await assertThai(th, "update panel", upWords);
    await th.locator('[data-action="upgrade-all"]').click();
    await th.locator('#upgrade-panel tr[data-upgrade="hq-master"] .badge', { hasText: "ไม่สำเร็จ" }).waitFor({ timeout: 30000 });
    await th.locator("#upgrade-panel", { hasText: "อัปเดตแล้ว" }).waitFor({ timeout: 30000 });
    await assertThai(th, "update progress", upWords);
    await shots(th, "update-all");
    for (let i = 0; i < 4; i++) { const closed = th.locator('#dc-tree .dc-toggle[aria-expanded="false"]'); if (!(await closed.count())) break; await closed.first().click(); }
    await th.locator('#dc-tree .dc-item[data-node="node:branch-master/child-1"]').click();
    await th.locator(".upgrade-badge").waitFor();
    await assertThai(th, "merged host needs update", upWords);
    // 6-digit compare (entry 8): waiting for Approve, then the digits.
    await scenario(th, url, ["empty", "six-digit"]); await th.reload(); await ready(th);
    await th.locator('[data-action="add-host"]').click();
    await th.locator("#add-address").fill("192.0.2.77");
    await th.locator('#create-pairing button[type="submit"]').click();
    await th.locator("#add-waiting").waitFor({ timeout: 15000 });
    await assertThai(th, "waiting for Approve", [...words, "exe", "deb", "Downloads", "fixture", "client", "linux", "windows", "pc", ...await fixtureWords(th, url)]);
    await th.request.get(url + "/pairing-code");
    await th.locator("#add-compare").waitFor({ timeout: 15000 });
    await assertThai(th, "compare the digits", [...words, "exe", "deb", "Downloads", "fixture", "client", "linux", "windows", "pc", ...await fixtureWords(th, url)]);
    await shots(th, "six-digit");
    // Partitions (entry 8): the Monitoring table, an older host's hint, and the alarm rule's partition field.
    await scenario(th, url, []); await th.reload(); await ready(th);
    const partWords = [...words, ...await fixtureWords(th, url), "var", "boot", "efi", "ext", "xfs", "vfat"];
    for (let i = 0; i < 4; i++) { const closed = th.locator('#dc-tree .dc-toggle[aria-expanded="false"]'); if (!(await closed.count())) break; await closed.first().click(); }
    await th.locator('#dc-tree .dc-item[data-node="node:ui-test"]').click();
    await th.locator('[data-action="dc-tab"][data-tab="monitoring"]').click();
    await th.locator("#partitions").waitFor();
    await assertThai(th, "partitions", partWords);
    await shots(th, "partitions");
    await th.locator('.nav[data-page="alarms"]').click();
    await th.locator('[data-action="alarm-tab"][data-tab="rules"]').click();
    await th.locator('[data-action="alarm-rule-new"]').click();
    await th.locator('#alarm-rule [name="partition"]').waitFor();
    await assertThai(th, "disk rule with a partition", partWords);
    await scenario(th, url, ["old-disk"]); await th.reload(); await ready(th);
    for (let i = 0; i < 4; i++) { const closed = th.locator('#dc-tree .dc-toggle[aria-expanded="false"]'); if (!(await closed.count())) break; await closed.first().click(); }
    await th.locator('#dc-tree .dc-item[data-node="node:ui-test"]').click();
    await th.locator('[data-action="dc-tab"][data-tab="monitoring"]').click();
    await th.locator("#partitions-hint").waitFor();
    await assertThai(th, "older host: update for partitions", partWords);
    // Health alarms (entry 7): the Active tab, the rule editor, and the host page's alarms and OS updates.
    await scenario(th, url, ["masters-page", "alarms-firing"]); await th.reload(); await ready(th);
    const alarmWords = [...words, ...await fixtureWords(th, url), "web", "db", "openssl", "curl", "libssl"];
    await th.locator('.nav[data-page="alarms"]').click();
    await th.locator("#alarm-list tbody tr").first().waitFor();
    await assertThai(th, "Alarms: active", alarmWords);
    await shots(th, "alarms-active");
    await th.locator('[data-action="alarm-tab"][data-tab="rules"]').click();
    await th.locator('[data-action="alarm-rule-new"]').click();
    await th.locator('#alarm-rule [name="scope"]').selectOption("hosts");
    await th.locator('#alarm-rule [name="condition"]').selectOption("container");
    await assertThai(th, "Alarms: rule editor", alarmWords);
    await shots(th, "alarms-rules");
    for (let i = 0; i < 4; i++) { const closed = th.locator('#dc-tree .dc-toggle[aria-expanded="false"]'); if (!(await closed.count())) break; await closed.first().click(); }
    await th.locator('#dc-tree .dc-item[data-node="node:ui-test"]').click();
    await th.locator("#host-alarms #updates-badge").waitFor();
    await th.locator("#host-alarms summary").click();
    await assertThai(th, "host alarms and updates", alarmWords);
    await shots(th, "host-alarms");
    await scenario(th, url, ["masters-page"]); await th.reload(); await ready(th);
    await th.locator('.nav[data-page="masters"]').click();
    await th.locator("h2", { hasText: "เครื่องนี้ถูกสั่งโดย" }).waitFor();

    // The add flow, step by step, from an empty Master.
    await scenario(th, url, ["empty"]);
    await th.reload(); await ready(th);
    await th.locator(".add-host").click();
    await th.locator("#add-address").fill("192.0.2.31");
    await th.locator("#add-address-check", { hasText: "ตัวติดตั้งจะเปิด SSH ให้" }).waitFor();
    await assertThai(th, "add step 1", words);
    await shots(th, "add-step-1");
    await th.locator('#create-pairing button[type="submit"]').click();
    // The saved installers' file names and folder come from the (fixture's) save dialog.
    words.push("exe", "deb", "Downloads", "fixture", "client", "windows", "pc"); // …and the host name the new host reports (fixture-windows-pc)
    await th.locator('#add-host [aria-current="step"][data-step="3"]').waitFor();
    await assertThai(th, "add step 3", [...words, ...await fixtureWords(th, url)]); // the installer name ends in the random target ID
    // 5. Errors: a mapped one is the Thai sentence plus the Master's English underneath.
    await th.locator("#add-code").fill(await codeFor(th, url, "?master=other"));
    const check = th.locator("#add-code-check.bad");
    await check.filter({ hasText: "รหัสนี้ทำมาสำหรับ Master อื่น" }).waitFor();
    assert.match(await check.locator(".error-detail").textContent(), /another Master/, "the English detail sits under the Thai sentence");
    assert.doesNotMatch(await check.textContent(), /E_PAIRING/, "no error code on screen");
    await assertThai(th, "add step 3 with an error", [...words, ...await fixtureWords(th, url)]);
    await th.locator("#add-code").fill(await codeFor(th, url));
    await th.locator("#finish-pairing").waitFor();
    await assertThai(th, "add step 4 review", [...words, ...await fixtureWords(th, url), "SHA256", "fixture"]);
    await th.locator('#finish-pairing button[type="submit"]').click();
    await th.locator(".ready-message").waitFor();
    await assertThai(th, "add step 4 result", [...words, ...await fixtureWords(th, url), "SHA256", "fixture"]);
    console.log("ok mapped error");

    // An unmapped error shows the English text alone, with no detail line.
    await scenario(th, url, ["masters-page", "merge-fails"]);
    await th.locator('[data-page="masters"]').click();
    await th.locator('[data-action="sync-merge"]').first().click();
    const notice = th.locator("#notice.error");
    await notice.waitFor();
    assert.match(await notice.textContent(), /[A-Za-z]{4,}/, "unmapped error stays English");
    assert.equal(await notice.locator(".error-detail").count(), 0, "no detail line for an unmapped error");
    console.log("ok unmapped error");

    // Tags (roadmap entry 4): the host's Tag row, with a Tag and a refused one.
    await scenario(th, url, []); await th.reload(); await ready(th);
    await rpc(th, url, "tags.set", { targetId: "ui-test", tags: ["web"] });
    await th.reload(); await ready(th);
    await th.locator('#dc-tree .dc-toggle').first().click();
    await th.locator('#dc-tree .dc-item[data-node="node:ui-test"]').click();
    await th.locator("#tag-add [name=tag]").fill("a,b"); await th.locator("#tag-add [name=tag]").press("Enter");
    await th.locator("#host-tags .add-check.bad").waitFor();
    await assertThai(th, "host Tags", [...words, "web"]);
    await shots(th, "host-tags");
    await th.locator('#dc-tree .dc-item[data-node="master:local"]').click();
    await th.locator('#dc-panel tr[data-host="ui-test"] .tag-chip').waitFor();
    await assertThai(th, "host table with Tags", [...words, "web"]);
    await th.locator('#dc-tree .dc-item[data-node="node:ui-test"]').click({ button: "right" });
    await th.locator(`#dc-menu [data-action="ctx-tag"]`).waitFor(); await assertThai(th, "host menu with Tag…", [...words, "web"]); await th.keyboard.press("Escape");
    // A group run by Tag: the Run on form, the confirmation and the results.
    await th.locator('#dc-tree .dc-item[data-node="node:ui-test"]').click();
    await th.locator('[data-action="dc-tab"][data-tab="jobs"]').click();
    await th.locator('[data-action="toggle-new-job"]').first().click();
    await th.locator('#start-job [name="runOn"]').selectOption("tag:web");
    await assertThai(th, "Run on a group", [...words, "web"]);
    await th.locator('#start-job [name="script"]').fill("uptime");
    await th.locator('#start-job button[type="submit"]').click();
    await th.locator("#group-dialog[open]").waitFor();
    await assertThai(th, "group confirmation", [...words, "web"]);
    await th.locator('#confirm-group button[type="submit"]').click();
    await th.locator("#group-run .group-summary").waitFor();
    await assertThai(th, "group run results", [...words, "web"]);
    await shots(th, "group-run");
    await th.locator('[data-action="group-close"]').click();
    // A mixed Linux + Windows group (masters-page has a merged Windows host), then Recent group runs.
    await scenario(th, url, ["masters-page"]);
    await rpc(th, url, "tags.set", { targetId: "ui-test", tags: ["mix"] });
    await rpc(th, url, "tags.set", { targetId: "branch-master/child-1", tags: ["mix"] });
    await th.reload(); await ready(th);
    const mixWords = [...words, ...await fixtureWords(th, url), "mix"];
    await th.locator('#dc-tree .dc-toggle').first().click();
    await th.locator('#dc-tree .dc-item[data-node="node:ui-test"]').click();
    await th.locator('[data-action="dc-tab"][data-tab="jobs"]').click();
    await th.locator('[data-action="toggle-new-job"]').first().click();
    await th.locator('#start-job [name="runOn"]').selectOption("tag:mix");
    await th.locator('#start-job [name="scriptLinux"]').waitFor();
    await assertThai(th, "mixed group form", mixWords);
    await th.locator('#start-job [name="scriptLinux"]').fill("uptime"); await th.locator('#start-job [name="scriptWindows"]').fill("Get-Date");
    await th.locator('#start-job button[type="submit"]').click();
    await th.locator('#confirm-group button[type="submit"]').click();
    await th.locator("#group-run .group-summary").waitFor();
    await th.locator('#dc-tree .dc-item[data-node="master:local"]').click();
    await th.locator('[data-action="dc-tab"][data-tab="activity"]').click();
    await th.locator("#activity-list tr.group-row").first().locator('[data-action="activity-toggle"]').click();
    await th.locator("#activity-list tr.group-host").first().waitFor();
    await assertThai(th, "Activity: jobs with a group run", mixWords);
    await shots(th, "activity-jobs");
    await th.locator('[data-action="activity-view"][data-view="audit"]').click();
    await th.locator("#activity-list .audit-table").waitFor();
    await assertThai(th, "Activity: audit log", mixWords);
    await shots(th, "activity-audit");
    // History, the offline host's recorded job, the warning and the keep-days setting.
    await scenario(th, url, ["activity-history", "activity-warning"]); await th.reload(); await ready(th);
    const historyWords = [...words, ...await fixtureWords(th, url), "web"];
    await th.locator('#dc-tree .dc-item[data-node="master:local"]').click();
    await th.locator("#activity-keep").waitFor();
    await assertThai(th, "Summary: keep activity, with the warning", historyWords);
    await th.locator('[data-action="dc-tab"][data-tab="activity"]').click();
    await th.locator('[data-action="activity-range"][data-range="7d"]').click();
    await th.locator("#activity-list tr.group-row").waitFor();
    await th.locator('#activity-list [data-job="job-lab"]').click();
    await th.locator("#activity-detail").waitFor();
    await assertThai(th, "Activity: an offline host's recorded job", historyWords);
    await th.locator('[data-action="activity-range"][data-range="custom"]').click();
    await th.locator("#activity-from").waitFor();
    await assertThai(th, "Activity: custom range", historyWords);
    await shots(th, "activity-history");

    // Backup and restore (roadmap entry 3): the section, both flows and their errors.
    const fileWords = [...words, "sshcc", "backup", "sshccbak", "Backups", "fixture"];
    await scenario(th, url, []); await th.reload(); await ready(th);
    await th.locator('#dc-tree .dc-item[data-node="master:local"]').click();
    await th.locator("#backup-section").waitFor();
    await th.locator('[data-action="backup-start"]').click();
    const backupForm = th.locator("#backup-pass-form");
    await backupForm.locator('[name="pass"]').fill("short"); await backupForm.locator('[name="again"]').fill("shorter");
    await assertThai(th, "backup passphrase", words);
    await backupForm.locator('[name="pass"]').fill("a long passphrase for tests"); await backupForm.locator('[name="again"]').fill("a long passphrase!");
    await assertThai(th, "backup passphrases differ", words);
    await backupForm.locator('[name="again"]').fill("a long passphrase for tests");
    await th.locator("#backup-save").click();
    await th.locator('#backup-flow .add-step[data-step="3"][aria-current="step"]').waitFor();
    await assertThai(th, "backup saved", fileWords);
    await shots(th, "backup-saved");
    await th.locator('#backup-flow [data-action="backup-close"]').first().click();
    await th.locator("#backup-section").waitFor(); await assertThai(th, "backup section", words);
    await scenario(th, url, ["empty"]); await th.reload(); await ready(th);
    await th.locator("#restore-offer").waitFor(); await assertThai(th, "inventory restore offer", words);
    await th.locator('#restore-offer [data-action="restore-start"]').click();
    await assertThai(th, "restore choose", words);
    await th.locator('[data-action="restore-choose"]').click();
    await th.locator('#restore-pass-form [name="pass"]').fill("not the passphrase");
    await th.locator('#restore-pass-form button[type="submit"]').click();
    await th.locator("#backup-flow .add-check.bad").waitFor();
    await assertThai(th, "restore wrong passphrase", fileWords);
    await th.locator('#restore-pass-form [name="pass"]').fill("a long passphrase for tests");
    await th.locator('#restore-pass-form button[type="submit"]').click();
    await th.locator("#restore-off").waitFor();
    await assertThai(th, "restore confirm", fileWords);
    await shots(th, "restore-confirm");
    await th.locator("#restore-off").check(); await th.locator("#restore-run").click();
    await th.locator('#backup-flow .add-step[data-step="4"] .add-check.good').waitFor();
    await assertThai(th, "restore done", fileWords);
    // What the new machine can't fix alone (a Master with a two-way Merge).
    await scenario(th, url, ["masters-page", "two-way-merge"]); await th.reload(); await ready(th);
    const twoWayWords = [...fileWords, ...await fixtureWords(th, url)];
    await th.locator('#dc-tree .dc-item[data-node="master:local"]').click();
    await th.locator('[data-action="backup-start"]').click();
    await backupForm.locator('[name="pass"]').fill("a long passphrase for tests"); await backupForm.locator('[name="again"]').fill("a long passphrase for tests");
    await th.locator("#backup-save").click();
    await th.locator('#backup-flow .add-step[data-step="3"][aria-current="step"]').waitFor();
    await scenario(th, url, ["empty"]); await th.reload(); await ready(th);
    await th.locator('#restore-offer [data-action="restore-start"]').click();
    await th.locator('[data-action="restore-choose"]').click();
    await th.locator('#restore-pass-form [name="pass"]').fill("a long passphrase for tests");
    await th.locator('#restore-pass-form button[type="submit"]').click();
    await th.locator("#restore-off").check(); await th.locator("#restore-run").click();
    await th.locator("#restore-follow-ups").waitFor();
    await assertThai(th, "restore follow-ups", twoWayWords);
    await shots(th, "restore-follow-ups");
    await th.locator('[data-action="keys-change"]').click();
    await th.locator("#keys-progress").waitFor();
    await assertThai(th, "change keys progress", twoWayWords);
    // Change keys later, with a host that can't change yet and merged hosts.
    await scenario(th, url, ["masters-page", "restored", "rotation-trouble"]); await th.reload(); await ready(th);
    await th.locator('#dc-tree .dc-item[data-node="master:local"]').click();
    await th.locator("#backup-section").waitFor(); await assertThai(th, "backup section, keys not changed", twoWayWords);
    await th.locator('#backup-section [data-action="keys-change"]').click();
    await th.locator('#keys-progress [data-host="ui-test"]').waitFor({ timeout: 15000 });
    await assertThai(th, "change keys trouble", twoWayWords);
    await shots(th, "keys-trouble");
    await th.locator("#keys-progress.pending").waitFor({ timeout: 20000 });
    await assertThai(th, "keys changed, back up now", twoWayWords);
    console.log("ok thai backup and restore");

    // 6. Every EN key has a TH key (and no TH key is left over).
    const parity = await th.evaluate(async () => {
      const { strings, keyPaths } = await import("/strings.js");
      const en = keyPaths(strings.en), thai = new Set(keyPaths(strings.th));
      return { missing: en.filter(key => !thai.has(key)), extra: [...thai].filter(key => !en.includes(key)), total: en.length };
    });
    assert.deepEqual(parity.missing, [], "EN keys with no TH key");
    assert.deepEqual(parity.extra, [], "TH keys with no EN key");
    console.log("ok parity " + parity.total + " keys");
    fs.writeFileSync(path.join(artifacts, "language-results.json"), JSON.stringify({ ok: true, decisions: d.count, keys: parity.total }, null, 2));
    console.log(JSON.stringify({ ok: true, decisions: d.count, keys: parity.total }));
  } finally {
    for (const context of contexts) await context.close().catch(() => {});
    await browser.close(); fixture.stop();
  }
})().catch(error => { console.error(error); process.exit(1); });
