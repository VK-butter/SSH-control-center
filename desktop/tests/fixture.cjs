// Isolated browser fixture. Never connects to a service or a real SSH Target.
// Scenario flags (comma-separated, env SSHCC_FIXTURE_SCENARIO or POST /scenario {"flags":[…]}):
//   empty            start as a Master with no hosts, folders, Merges or relays
//   new-host-master  every host paired through the fixture is itself a Master (merges.discover/fleet.health say so)
//   merge-fails      merges.add / merges.sync fail (the remote Master does not answer)
//   needs-attention  completing a pairing fails the connection check; an address retry (pairing.status routes) passes
//   masters-page     adds two hosts that are Masters (one merged, one not) and a Master that controls this one both
//                    directly and through a Merge, plus one that only merged this Master (merging disabled)
//   backed-up        this Master has a last backup date
//   restored         this Master was restored from a backup and its keys were not changed yet
//   rotation-trouble Change keys: the first attempt on ui-test fails (the Owner is not logged in); the next one works
//   group-offline    a group run can't reach hq-master (No route to host)
//   group-slow       group jobs keep running until cancelled (retried jobs finish as usual)
//   legacy-host      adds old-box, a host registered without the SSH Control Center service (v1)
//   two-way-merge    with masters-page: the Merge with branch-master is two-way (it shows as a relay here too)
//   view-only-merge  with masters-page: Branch Master gives this Master View only (its hosts carry access "view")
//   merge-refuses    with masters-page: a job on branch-master/child-2 is refused as if Branch Master's Tag limit had
//                    changed since the last sync (E_MERGE_ROLE)
//   multi-hop        with masters-page: Branch Master also passes on Lab Master (a Master it merged) and Lab's two hosts,
//                    one of which Lab Master gives View only; 3 more hosts are more than 4 hops away
//   outdated-merge   with masters-page: Branch Master runs an older version (no relayVersion)
//   six-digit        pairing.fetch answers like 0.5.0 installers: "waiting" on the first look, then the 6 digits once the
//                    request was approved (GET /pairing-code); without it, fetch answers "old" (the paste box, as before)
//   old-disk         monitor.stats reports only / (a host from before entry 8); otherwise three partitions, /var fullest
//   outdated-hosts   ui-test runs 0.4.0 (this Master 0.5.0); with masters-page, hq-master too and branch-master/child-1
//                    (updated by Branch Master); targets.upgrade succeeds (hq-master fails with upgrade-fails)
//   upgrade-fails    with outdated-hosts and masters-page: updating hq-master times out
//   no-templates     with outdated-hosts: this Master has no installer templates (a development build)
//   alarms-firing    a red disk alarm on ui-test, a yellow memory alarm (acknowledged later in tests), ui-test has 3
//                    OS updates pending; with masters-page, hq-master is unreachable (red)
//   alarm-new-red    a red alarm appears after the window's first look (for the notice)
//   activity-history Activity has history: jobs today, yesterday and 3 days ago (one group run), audit lines, a running
//                    job that finishes on the next refresh, and lab-pc, whose recorded job's host can't be reached
//   activity-warning the audit log can't be written (Activity shows its yellow warning)
// Backups: save_backup "writes" to /fixture/Backups and keeps the file in memory (across scenario resets), so a test can
// back up, POST /scenario {"flags":["empty"]} and restore it. POST /next-backup {"kind":"good"|"newer"|"damaged"|"other"} makes the
// next open_backup return that kind of file instead (once).
// Activity: every audited request the page sends is written to the fixture's audit log (channel "desktop"), and every
// job it starts to Recent Jobs, as the Master does. GET /last-csv returns the last save_csv {name, csv}.
// Test-only endpoints: GET /calls (every request the page sent), POST /scenario (reset state, clears /calls),
// GET /pairing-code[?requestId=…][&master=other] (the code the new host would show after Approve; default: the
// newest request awaiting approval; master=other gives a code made for another Master).
const http = require("node:http");
const fs = require("node:fs");
const path = require("node:path");
const crypto = require("node:crypto");
const ui = path.join(__dirname, "..", "ui");
const MASTER_ID = "fixture-master", MASTER_LABEL = "UI fixture Master";
const calls = [];
let flags, s, content, firstStart, savedBackup = null, nextBackup = "", lastCsv = null;
/** All mutable fixture state; `reset` rebuilds it for a scenario. */
function reset(list) {
  flags = new Set(list.filter(Boolean)); calls.length = 0; content = "Example text\n"; firstStart = true;
  const empty = flags.has("empty");
  s = {
    targets: empty ? [] : [{ schemaVersion: 2, id: "ui-test", nodeId: "ui-test-node", label: "UI test target", tenant: "test-only", os: "linux", user: "sshcc-bridge", routes: [{ kind: "lan", address: "192.0.2.20", port: 22 }], hostKeys: ["ssh-ed25519 TEST-FIXTURE-ONLY"], bridgePath: "/usr/bin/sshcc", runnerInstalled: true }],
    jobs: [], pairings: [], folders: [], folderMembers: {}, tags: [], groupRuns: [], datacenters: [], datacenterMembers: {}, merges: [], masters: empty ? [] : [{ id: MASTER_ID, label: MASTER_LABEL, publicKey: "TEST", revoked: false }], relays: [], masterHosts: new Set(),
    alarm: { rules: alarmDefaults(), interval: 5, alarms: [], updates: {}, looked: 0 },
    activity: { keepDays: 14, since: stamp(Date.now() - 20 * DAY), audit: [], jobs: [] },
    backup: { lastBackupAt: flags.has("backed-up") || flags.has("restored") ? "2026-10-20T09:30:00Z" : null, restoredAt: flags.has("restored") ? "2026-10-24T08:00:00Z" : null, keysChangedAt: null }, rotation: null
  };
  if (flags.has("activity-history")) seedActivity();
  if (flags.has("alarms-firing")) {
    const since = stamp(Date.now() - 42 * 60000);
    s.alarm.alarms.push({ id: "default-disk:ui-test", rule: "default-disk", host: "ui-test", label: "UI test target", condition: "disk", level: "red", value: 93.5, since, acknowledged: false, active: true },
      { id: "mem-yellow:ui-test", rule: "mem-yellow", host: "ui-test", label: "UI test target", condition: "memory", level: "yellow", value: 82, since, acknowledged: false, active: true });
    s.alarm.rules.push({ id: "mem-yellow", scope: { tag: "web" }, condition: "memory", threshold: 80, containers: [], level: "yellow" });
    s.alarm.updates["ui-test"] = { count: 3, packages: ["openssl", "curl", "libssl3"], error: null, checkedAt: stamp(Date.now() - 3 * 86400000), checkNow: false };
  }
  if (flags.has("masters-page")) {
    const host = (id, label, address) => ({ schemaVersion: 2, id, nodeId: id + "-node", label, tenant: "test-only", os: "linux", user: "sshcc-bridge", routes: [{ kind: "lan", address, port: 22 }], hostKeys: ["ssh-ed25519 TEST-FIXTURE-ONLY"], bridgePath: "/usr/bin/sshcc", runnerInstalled: true });
    s.targets.push(host("hq-master", "HQ Master", "192.0.2.41"), host("branch-master", "Branch Master", "192.0.2.42"));
    s.masterHosts = new Set(["hq-master", "branch-master"]);
    s.merges.push({ targetId: "branch-master", label: "Branch Master", status: "merged", error: null, syncedAt: now(), children: children("branch-master"), reverse: null,
      grant: flags.has("view-only-merge") ? { role: "view", tags: null, passThrough: true } : { role: "full", tags: null, passThrough: true }, relayVersion: flags.has("outdated-merge") ? null : 2 });
    if (flags.has("multi-hop")) {
      const lab = { id: "11111111-1111-4111-8111-111111111111", label: "Lab Master" };
      s.merges[0].children.push({ id: "lab-master", label: "Lab Master", tenant: "lab", os: "linux", nodeId: lab.id },
        { id: "lab-master/lab-web", label: "Lab web", tenant: "lab", os: "linux", nodeId: crypto.randomUUID(), hops: 2, masters: [lab] },
        { id: "lab-master/lab-db", label: "Lab db", tenant: "lab", os: "linux", nodeId: crypto.randomUUID(), hops: 2, masters: [lab], access: "view", accessFrom: "Lab Master" });
      s.merges[0].beyond = 3;
    }
    s.masters.push({ id: "peer-both", label: "Peer Master", publicKey: "TEST", revoked: false });
    const grant = { role: "full", tags: null, passThrough: true };
    s.relays.push({ masterId: "peer-both", label: "Peer Master", lastUsedAt: now(), revoked: false, ...grant }, { masterId: "peer-relay", label: "Relay-only Master", lastUsedAt: now(), revoked: true, ...grant });
    if (flags.has("legacy-host")) s.targets.push({ ...host("old-box", "Old box", "192.0.2.43"), runnerInstalled: false });
    if (flags.has("two-way-merge")) {
      s.merges[0].reverse = { status: "two-way", error: null };
      s.relays.push({ masterId: "branch-master-node", label: "Branch Master", lastUsedAt: now(), revoked: false });
    }
  }
}
const now = () => new Date().toISOString();
const DAY = 86400000, THAI = 7 * 3600000;
/** ISO 8601 in Thailand time, as the Master writes it. */
const stamp = ms => new Date(ms + THAI).toISOString().slice(0, 19) + "+07:00";
const thaiDate = ms => new Date(ms + THAI).toISOString().slice(0, 10);
const thaiMidnight = day => Date.parse(day + "T00:00:00+07:00");
const sha = () => crypto.createHash("sha256").update(content).digest("hex");
const slug = text => String(text || "").toLowerCase().replace(/[^a-z0-9_-]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 48) || "host";
const need = (value, name) => { if (value === undefined || value === null || value === "") throw new Error("E_ARGUMENT: missing " + name); return value; };
const pairingView = p => ({ requestId: p.requestId, targetId: p.targetId, label: p.label, tenant: p.tenant, os: p.os, state: p.state, createdAt: p.createdAt, updatedAt: p.updatedAt,
  envelope: { requestId: p.requestId, targetLabel: p.label, bundle: { targetId: p.targetId, tenant: p.tenant, os: p.os, user: p.user, routes: p.routes, master: { id: MASTER_ID, label: MASTER_LABEL } } }, diagnostics: p.diagnostics || null, sibling: p.sibling || null });
// Like the Master: a request cancelled because its sibling was answered first says so.
const pairing = id => s.pairings.find(p => p.requestId === id) || (() => { throw new Error(s.pairings.some(p => p.sibling === id && p.state !== "awaiting_approval")
  ? "E_PAIRING_REPLACED: unknown setup request on this Master; the other installer of the pair was used first" : "E_PAIRING_REQUEST: unknown setup request"); })();
const isMasterHost = id => s.masterHosts.has(id);
const children = id => [1, 2, 3].map(n => ({ id: "child-" + n, label: "Child " + n + " of " + id, tenant: "lab", os: n === 1 ? "windows" : "linux", nodeId: crypto.randomUUID() }));
/** A Merge's hosts, listed by targets.list as `<master>/<child>` (as the real Master does). */
const relayedTargets = () => s.merges.flatMap(m => (m.children || []).map(c => ({ schemaVersion: 2, id: m.targetId + "/" + c.id, nodeId: c.nodeId, label: c.label, tenant: c.tenant, os: c.os, relayed: true, via: m.targetId, viaLabel: m.label, routes: [], hostKeys: [], runnerInstalled: true,
  path: [m.label, ...(c.masters || []).map(x => x.label)], hops: c.hops || 1,
  access: m.grant?.role === "view" || c.access === "view" ? "view" : "full", accessFrom: m.grant?.role === "view" ? m.label : c.accessFrom || null })));
const healthRow = t => ({ targetId: t.id, online: true, checkedAt: now(), error: null, failures: 0, isMaster: isMasterHost(t.id), version: "test" });
// After the helpers: the masters-page scenario uses `now` and `children`.
/** The code the new host shows after Approve: `SSHCC1.fixture.<masterId>.<requestId>`. */
const pairingCode = (requestId, master = MASTER_ID) => "SSHCC1.fixture." + master + "." + requestId;
/** Validation shared by preview and complete (as on the real Master): the request the code answers. */
function codeRequest(code) {
  const m = /^SSHCC1\.fixture\.([^.]+)\.([^.]+)$/.exec(String(need(code, "code")).trim());
  if (!m) throw new Error("E_PAIRING_CODE: damaged pairing code");
  if (m[1] !== MASTER_ID) throw new Error("E_PAIRING_MASTER: code belongs to another Master");
  return pairing(m[2]);
}
const hostKeySha256 = p => "SHA256:" + crypto.createHash("sha256").update("FIXTURE-" + p.targetId).digest("base64").replace(/=+$/, "");
/** Read only: the review card's facts; saves nothing. */
function previewPairing({ code }) {
  const p = codeRequest(code);
  return { requestId: p.requestId, targetId: p.targetId, label: p.label, hostName: "fixture-" + p.os + "-pc", os: p.os, hostKeySha256: hostKeySha256(p) };
}
function completePairing({ code, label }) {
  const p = codeRequest(code);
  if (p.state !== "awaiting_approval") return pairingView(p);
  if (label) p.label = label;
  const target = { schemaVersion: 2, id: p.targetId, nodeId: crypto.randomUUID(), label: p.label, tenant: p.tenant, os: p.os, user: p.user, routes: p.routes, hostKeys: ["ssh-ed25519 FIXTURE-" + p.targetId], bridgePath: p.os === "windows" ? "C:\\Program Files\\SSHCC\\sshcc.exe" : "/usr/bin/sshcc", runnerInstalled: true };
  s.targets = s.targets.filter(t => t.id !== p.targetId).concat(target);
  if (flags.has("new-host-master")) s.masterHosts.add(p.targetId);
  if (p.sibling) s.pairings = s.pairings.filter(row => !(row.requestId === p.sibling && row.state === "awaiting_approval"));
  const failed = flags.has("needs-attention");
  Object.assign(p, { state: failed ? "needs_attention" : "ready", updatedAt: now(), diagnostics: failed
    ? { targetId: p.targetId, ok: false, stages: [{ name: "connect", ok: false, message: "No SSH answer at " + p.routes[0].address + ":" + p.routes[0].port }] }
    : { targetId: p.targetId, ok: true, stages: [{ name: "fixture", ok: true, message: "Isolated UI test only" }] } });
  return pairingView(p);
}
function mergeAdd(targetId) {
  const t = s.targets.find(row => row.id === targetId) || (() => { throw new Error("E_TARGET: unknown target " + targetId); })();
  if (!isMasterHost(targetId)) throw new Error("E_MERGE: " + targetId + " is not a Master");
  if (flags.has("merge-fails")) throw new Error("E_MERGE: remote Master " + targetId + " did not answer");
  const previous = s.merges.find(row => row.targetId === targetId);
  const merge = { targetId, label: t.label, status: "merged", error: null, syncedAt: now(), children: previous?.children || children(targetId), reverse: previous?.reverse || null };
  s.merges = s.merges.filter(row => row.targetId !== targetId).concat(merge);
  return merge;
}
// `pairing.check` stand-in: SSH answers only at the addresses below; every other address does not answer
// (a fresh Windows machine). SLOW_ADDRESS answers after 1.5 s, to test that late answers are ignored.
const BANNERS = { "192.0.2.33": "SSH-2.0-OpenSSH_for_Windows_9.5", "192.0.2.32": "SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.5", "192.0.2.99": "SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.5" };
const SLOW_ADDRESS = "192.0.2.99";
async function checkAddress(address, port, own) {
  if (address === SLOW_ADDRESS) await new Promise(done => setTimeout(done, 1500));
  const same = r => r.address.toLowerCase() === address.toLowerCase() && Number(r.port || 22) === port;
  const mine = p => own && (p.requestId === own || p.sibling === own); // a request's own sibling is not "already added"
  const target = s.targets.find(t => t.routes.some(same)), request = !target && s.pairings.find(p => !mine(p) && p.state === "awaiting_approval" && p.routes.some(same));
  const banner = port === 22 && BANNERS[address] || null;
  return { address, port, reachable: Boolean(banner), banner, os: banner ? (banner.includes("Windows") ? "windows" : "linux") : null,
    alreadyAs: target?.label || request?.label || null, alreadyTargetId: target?.id || null, alreadyRequestId: request?.requestId || null };
}
/* Backup files are base64 JSON: the passphrase and a copy of the fleet. The real format is tested in Rust. */
const FLEET_KEYS = ["targets", "folders", "folderMembers", "datacenters", "datacenterMembers", "merges", "masters", "relays", "tags"];
const isEmpty = () => !s.targets.length && !s.merges.length && !s.folders.length;
function makeBackup(passphrase) {
  if ([...String(passphrase || "")].length < 12) throw new Error("E_BACKUP_PASSPHRASE_SHORT: the passphrase must be at least 12 characters");
  const fleet = Object.fromEntries(FLEET_KEYS.map(key => [key, structuredClone(s[key])]));
  return Buffer.from(JSON.stringify({ fixtureBackup: 1, formatVersion: 1, passphrase, createdAt: now(), fleet })).toString("base64");
}
function openBackup(file, passphrase) {
  let backup;
  try { backup = JSON.parse(Buffer.from(String(file), "base64").toString("utf8")); } catch { backup = null; }
  if (!backup?.fixtureBackup) throw new Error("E_BACKUP_NOT_A_BACKUP: this file is not an SSH Control Center backup");
  if (backup.formatVersion > 1) throw new Error("E_BACKUP_NEWER: this backup comes from a newer Master (format " + backup.formatVersion + "); update this Master first");
  if (backup.damaged || backup.passphrase !== passphrase) throw new Error("E_BACKUP_BAD_PASSPHRASE_OR_DAMAGED: the passphrase is wrong or the file is damaged");
  return backup;
}
const backupSummary = b => ({ formatVersion: 1, createdAt: b.createdAt, nodeId: "fixture", label: MASTER_LABEL, os: "linux", hosts: b.fleet.targets.length, merges: b.fleet.merges.length, folders: b.fleet.folders.length });
function restoreBackup({ file, passphrase }) {
  if (!isEmpty()) throw new Error("E_RESTORE_NOT_EMPTY: this Master already has hosts; restore only works on a fresh install");
  const backup = openBackup(file, passphrase);
  Object.assign(s, structuredClone(backup.fleet));
  s.backup = { lastBackupAt: backup.createdAt, restoredAt: now(), keysChangedAt: null }; s.rotation = null;
  // As the Master does: two-way Merges come back one-way, and the Masters that reach this one are listed.
  const twoWayAgain = s.merges.filter(m => m.reverse?.status === "two-way").map(m => ({ targetId: m.targetId, label: m.label }));
  for (const merge of s.merges) merge.reverse = null;
  const reached = new Map();
  for (const m of s.masters.filter(m => !m.revoked && m.id !== MASTER_ID)) reached.set(m.id, { masterId: m.id, label: m.label, ways: ["direct"] });
  for (const r of s.relays.filter(r => !r.revoked)) { const row = reached.get(r.masterId) || { masterId: r.masterId, label: r.label, ways: [] }; row.ways.push("merge"); reached.set(r.masterId, row); }
  return { ...backupSummary(backup), restartRequired: true, reachedBy: [...reached.values()], twoWayAgain, checkPaths: [] };
}
/* Change keys: every direct host starts on the old key; each keys.rotation_status call moves the next one. */
function rotationStatus() {
  const r = s.rotation;
  if (!r) return { active: false };
  const next = r.hosts.find(h => h.state === "pending");
  if (next && r.started) {
    if (flags.has("rotation-trouble") && next.targetId === "ui-test" && !next.failed) { next.failed = true; next.error = "E_OWNER_NOT_LOGGED_IN: Owner must be logged in"; }
    else { next.state = "moved"; next.error = null; }
  }
  r.started = true;
  const moved = r.hosts.filter(h => h.state === "moved").length, oldKey = r.hosts.filter(h => h.state === "pending").length;
  if (moved === r.hosts.length && !r.finishedAt) { r.finishedAt = now(); s.backup.keysChangedAt = r.finishedAt; }
  const merged = s.merges.flatMap(m => (m.children || []).map(c => ({ label: c.label, master: m.label })));
  return { active: !r.finishedAt, startedAt: r.startedAt, finishedAt: r.finishedAt || null, total: r.hosts.length, moved, oldKey, hosts: r.hosts.map(({ failed, ...h }) => ({ ...h })), merged };
}
/* Tags, as the Master keeps them: rows of {targetId, key, tag}; matched without case, first spelling kept. */
function tagList() {
  const byKey = new Map(), hosts = {};
  for (const row of [...s.tags].sort((a, b) => a.targetId.localeCompare(b.targetId))) {
    if (!byKey.has(row.key)) byKey.set(row.key, { tag: row.tag, hosts: [] });
    byKey.get(row.key).hosts.push(row.targetId); (hosts[row.targetId] ||= []).push(row.tag);
  }
  return { tags: [...byKey.entries()].sort(([a], [b]) => a < b ? -1 : 1).map(([, v]) => v), hosts };
}
function setTags({ targetId, tags }) {
  if (![...s.targets, ...relayedTargets()].some(t => t.id === targetId)) throw new Error("E_TARGET: unknown host " + targetId);
  const rows = [];
  for (const raw of tags || []) {
    const tag = String(raw).trim(), key = tag.toLowerCase();
    if (!tag || [...tag].length > 32) throw new Error("E_TAG_LENGTH: a Tag has 1-32 characters");
    if (/[,\u0000-\u001f]/.test(tag)) throw new Error("E_TAG_CHARS: a Tag can't contain a comma");
    if (rows.some(row => row.key === key)) continue;
    const first = s.tags.find(row => row.key === key && row.targetId !== targetId);
    rows.push({ targetId, key, tag: first ? first.tag : tag });
  }
  if (rows.length > 20) throw new Error("E_TAG_LIMIT: a host has at most 20 Tags");
  s.tags = s.tags.filter(row => row.targetId !== targetId).concat(rows);
  return { targetId, tags: rows.map(row => row.tag) };
}
/* Group runs: hosts chosen by Tag or folder; each groups.status call moves started hosts on (queued → running → succeeded). */
function groupHosts(selector) {
  const all = [...s.targets, ...relayedTargets()];
  const ids = selector?.tag ? s.tags.filter(row => row.key === String(selector.tag).trim().toLowerCase()).map(row => row.targetId)
    : selector?.folder ? Object.entries(s.folderMembers).filter(([, folder]) => folder === selector.folder).map(([id]) => id) : null;
  if (!ids) throw new Error("E_ARGUMENT: choose a Tag or a folder");
  const rows = all.filter(t => ids.includes(t.id)).sort((a, b) => a.id.localeCompare(b.id)).map(t => ({ targetId: t.id, label: t.label, os: t.os, runner: t.runnerInstalled !== false }));
  return { hosts: rows.filter(r => r.runner).map(({ runner, ...r }) => r), skipped: rows.filter(r => !r.runner).map(({ runner, ...r }) => ({ ...r, reason: "E_RUNNER_NOT_INSTALLED: no service" })) };
}
function startGroup(p) {
  const { hosts, skipped } = groupHosts(p.selector);
  if (!hosts.length) throw new Error("E_GROUP_EMPTY: no host in this group can run a job");
  for (const h of hosts) if (!String(p.scripts?.[h.os] || "").trim()) throw new Error("E_GROUP_SCRIPT: this group has " + (h.os === "windows" ? "Windows" : "Linux") + " hosts; write a script for them too");
  const run = { id: crypto.randomUUID(), createdAt: now(), selector: p.selector, scripts: { linux: p.scripts.linux || null, windows: p.scripts.windows || null }, asAdmin: Boolean(p.asAdmin), env: p.env || {}, done: false,
    hosts: [...hosts.map(h => flags.has("group-offline") && h.targetId === "hq-master"
      ? { ...h, jobId: null, state: "unreachable", error: "ALL_ROUTES_FAILED: lan 192.0.2.41:22: No route to host", status: null, exitCode: null }
      : { ...h, jobId: crypto.randomUUID(), state: "started", error: null, status: "queued", exitCode: null, cwd: typeof p.cwd === "object" ? p.cwd[h.os] : p.cwd }),
      ...skipped.map(h => ({ ...h, state: "skipped", error: h.reason }))] };
  s.groupRuns.unshift(run); s.groupRuns = s.groupRuns.slice(0, 20);
  return structuredClone(run);
}
function groupStatus(id) {
  const run = s.groupRuns.find(r => r.id === id) || (() => { throw new Error("E_NOT_FOUND: this group run is no longer kept"); })();
  for (const h of run.hosts.filter(h => h.state === "started")) {
    if (flags.has("group-slow") && !h.retried && ["queued", "running"].includes(h.status)) { h.status = "running"; continue; }
    if (h.status === "queued") h.status = "running";
    else if (h.status === "running") { h.status = "succeeded"; h.exitCode = 0; }
  }
  run.done = run.hosts.every(h => h.state !== "started" || ["succeeded", "failed", "cancelled", "interrupted"].includes(h.status));
  return structuredClone(run);
}
/* Target updates (entry 8): versions from the scenario; an update "finishes" at once (one status step per call). */
const LATEST = "0.5.0";
function updateCheck() {
  const old = new Set(flags.has("outdated-hosts") ? ["ui-test", "hq-master", "branch-master/child-1"] : []);
  for (const id of s.upgraded || []) old.delete(id);
  return { latest: LATEST, hosts: [...s.targets, ...relayedTargets()].map(t => {
    const outdated = old.has(t.id), by = t.relayed ? "by:" + t.viaLabel : null;
    const reason = !outdated ? "current" : by || (flags.has("no-templates") ? "E_UPGRADE_TEMPLATES" : null);
    return { targetId: t.id, label: t.label, os: t.os, version: outdated ? "0.4.0" : LATEST, latest: LATEST, outdated, canUpdate: reason === null, reason };
  }) };
}
function upgradeOne(id) {
  const row = updateCheck().hosts.find(r => r.targetId === id);
  if (!row?.canUpdate) throw new Error(row?.reason === "E_UPGRADE_TEMPLATES" ? "E_UPGRADE_TEMPLATES: this Master has no installer to send" : "E_UPGRADE_CURRENT: nothing to update");
  if (flags.has("upgrade-fails") && id === "hq-master") throw new Error("E_UPGRADE_TIMEOUT: hq-master didn't come back with 0.5.0 within 3 minutes");
  (s.upgraded = s.upgraded || []).push(id);
  return { targetId: id, version: LATEST, from: "0.4.0" };
}
function upgradeStatus() {
  const run = s.upgradeRun;
  if (!run) return { hosts: [], done: true };
  const next = run.hosts.find(r => !["done", "failed"].includes(r.state));
  if (next) {
    const steps = ["waiting", "uploading", "installing", "checking"], at = steps.indexOf(next.state);
    if (at < steps.length - 1) next.state = steps[at + 1];
    else try { upgradeOne(next.targetId); next.state = "done"; } catch (error) { next.state = "failed"; next.error = error.message; }
  }
  run.done = run.hosts.every(r => ["done", "failed"].includes(r.state));
  return structuredClone(run);
}
/* Health alarms (entry 7): the three default rules; alarms come from the scenario flags, not from evaluating rules. */
function alarmDefaults() {
  return [["default-unreachable", "unreachable", null], ["default-disk", "disk", 90], ["default-memory", "memory", 90]]
    .map(([id, condition, threshold]) => ({ id, scope: { all: true }, condition, threshold, containers: [], level: "red" }));
}
function alarmList() {
  s.alarm.looked++;
  if (flags.has("alarm-new-red") && s.alarm.looked > 1 && !s.alarm.alarms.some(a => a.id === "default-unreachable:ui-test"))
    s.alarm.alarms.push({ id: "default-unreachable:ui-test", rule: "default-unreachable", host: "ui-test", label: "UI test target", condition: "unreachable", level: "red", value: "No route to host", since: stamp(Date.now()), acknowledged: false, active: true });
  if (flags.has("alarms-firing") && flags.has("masters-page") && !s.alarm.alarms.some(a => a.host === "hq-master"))
    s.alarm.alarms.push({ id: "default-unreachable:hq-master", rule: "default-unreachable", host: "hq-master", label: "HQ Master", condition: "unreachable", level: "red", value: "No route to host", since: stamp(Date.now() - 600000), acknowledged: false, active: true });
  const alarms = [...s.alarm.alarms].sort((a, b) => (b.level === "red") - (a.level === "red") || String(b.since).localeCompare(String(a.since)));
  const count = level => alarms.filter(a => a.level === level && !a.acknowledged).length;
  return { alarms: structuredClone(alarms), red: count("red"), yellow: count("yellow") };
}
function alarmRule(p) {
  const scope = p.scope || {};
  if (!(scope.all || scope.folder || (scope.tag && String(scope.tag).trim()) || (scope.hosts || []).length)) throw new Error("E_ALARM_RULE: choose every host, a folder, a Tag or hosts");
  if (!["disk", "memory", "unreachable", "container", "updates"].includes(p.condition)) throw new Error("E_ALARM_RULE: the condition is disk, memory, unreachable, container or updates");
  if (["disk", "memory"].includes(p.condition) && !(p.threshold >= 1 && p.threshold <= 99)) throw new Error("E_ALARM_RULE: a percentage from 1 to 99");
  if (!["red", "yellow"].includes(p.level)) throw new Error("E_ALARM_RULE: the level is yellow or red");
  const rule = { id: p.id || crypto.randomUUID(), scope: scope.all ? { all: true } : scope.folder ? { folder: scope.folder } : scope.tag ? { tag: String(scope.tag).trim() } : { hosts: scope.hosts },
    condition: p.condition, threshold: ["disk", "memory"].includes(p.condition) ? p.threshold : null, containers: p.condition === "container" ? p.containers || [] : [], level: p.level,
    ...(p.condition === "disk" && p.partition ? { partition: p.partition } : {}) };
  s.alarm.rules = s.alarm.rules.filter(r => r.id !== rule.id).concat(rule);
  return rule;
}
/* Activity, like the Master: the audited methods, Recent Jobs rows and the filters (Thai calendar days). */
const AUDITED = new Set(["alarms.rule_set", "alarms.rule_delete", "alarms.settings", "alarms.ack", "alarms.check_updates", "jobs.start", "jobs.cancel", "groups.run", "groups.cancel", "groups.retry", "targets.import", "targets.update", "targets.import_legacy", "targets.remove",
  "enrollment.export", "enrollment.accept", "masters.revoke", "merges.add", "merges.sync", "merges.pair_request", "merges.pair_complete", "merges.remove", "relays.revoke", "relays.allow",
  "pairing.create", "pairing.complete", "pairing.cancel", "tags.set", "folders.create", "folders.rename", "folders.delete", "folders.assign",
  "datacenters.create", "datacenters.rename", "datacenters.delete", "datacenters.assign", "files.write", "files.mkdir", "files.remove", "containers.action", "terminal.open",
  "backup.create", "backup.restore", "keys.rotate", "activity.settings"]);
const audited = (method, p) => AUDITED.has(method) || (method === "files.read" && p.download === true) || (method === "files.upload" && p.done === true);
const OBJECT_KEYS = ["targetId", "master", "jobId", "runId", "path", "container", "action", "id", "name", "folderId", "datacenterId", "masterId", "keepDays"];
function auditObject(method, p) {
  const parts = OBJECT_KEYS.map(key => p[key]).filter(v => (typeof v === "string" && v) || typeof v === "number").map(String);
  if (method === "groups.run") { if (p.selector?.tag) parts.push("tag " + p.selector.tag); if (p.selector?.folder) parts.push("folder " + p.selector.folder); }
  if (method === "tags.set") parts.push((p.tags || []).join(", "));
  if (method === "targets.import" || method === "targets.update") { const id = (p.registration || p.target)?.id; if (id) parts.push(id); }
  return parts.join(" · ");
}
function audit(method, p, result, channel = "desktop", at = Date.now()) {
  if (flags.has("activity-warning")) return;
  s.activity.audit.push({ time: stamp(at), action: method, object: auditObject(method, p), result, channel, user: "fixture-owner" });
}
function logJob(row, at = Date.now()) {
  const job = { jobId: row.jobId, targetId: row.targetId, label: row.label || null, groupId: row.groupId || null, selector: row.selector || null, script: String(row.script || "").slice(0, 4096),
    cwd: row.cwd || "/", shell: row.shell || null, asAdmin: Boolean(row.asAdmin), envNames: Object.keys(row.env || {}), channel: row.channel || "desktop", sentAt: stamp(at), sentMs: at,
    status: row.status || "queued", exitCode: row.exitCode ?? null, startedAt: row.startedAt || null, finishedAt: row.finishedAt || null, error: row.error || null };
  s.activity.jobs = s.activity.jobs.filter(j => j.jobId !== job.jobId).concat(job);
  return job;
}
function seedActivity() {
  const t = Date.now(), today = thaiMidnight(thaiDate(t)), at = (day, h, m) => today - day * DAY + (h * 60 + m) * 60000;
  s.targets.push({ schemaVersion: 2, id: "lab-pc", nodeId: "lab-pc-node", label: "Lab PC", tenant: "test-only", os: "linux", user: "sshcc-bridge", routes: [{ kind: "lan", address: "192.0.2.99", port: 22 }], hostKeys: ["ssh-ed25519 TEST-FIXTURE-ONLY"], bridgePath: "/usr/bin/sshcc", runnerInstalled: true });
  s.tags.push({ targetId: "ui-test", key: "web", tag: "web" }, { targetId: "lab-pc", key: "web", tag: "web" });
  const job = (id, target, script, day, h, m, status, exitCode, extra = {}) => {
    const sent = at(day, h, m), done = ["succeeded", "failed"].includes(status);
    logJob({ jobId: id, targetId: target, script, status, exitCode, startedAt: new Date(sent + 1000).toISOString(), finishedAt: done ? new Date(sent + 9000).toISOString() : null, ...extra }, sent);
    if (target === "ui-test") s.jobs.push({ id, masterId: MASTER_ID, script, cwd: extra.cwd || "/", status, createdAt: new Date(sent).toISOString(), startedAt: new Date(sent + 1000).toISOString(), exitCode, live: extra.live });
  };
  job("job-old", "ui-test", "apt-get update", 3, 10, 0, "failed", 100);
  job("job-yesterday", "ui-test", "systemctl restart nginx\nsystemctl status nginx", 1, 14, 3, "succeeded", 0, { cwd: "/etc/nginx", env: { TOKEN: "x" } });
  job("job-lab", "lab-pc", "df -h", 1, 23, 50, "running", null, { channel: "cli" });
  const selector = { tag: "web" };
  job("job-g1", "ui-test", "uptime", 0, 0, 20, "succeeded", 0, { groupId: "run-web", selector, label: "UI test target" });
  job("job-g2", "lab-pc", "uptime", 0, 0, 20, "unreachable", null, { groupId: "run-web", selector, label: "Lab PC", error: "ALL_ROUTES_FAILED: lan 192.0.2.99:22: No route to host" });
  job("job-live", "ui-test", "sleep 5; echo done", 0, 0, 40, "running", null, { live: true });
  const line = (day, h, m, action, p, result = "ok", channel = "desktop") => audit(action, p, result, channel, at(day, h, m));
  line(3, 10, 0, "jobs.start", { targetId: "ui-test", jobId: "job-old" });
  line(1, 9, 15, "tags.set", { targetId: "ui-test", tags: ["web"] });
  line(1, 14, 3, "jobs.start", { targetId: "ui-test", jobId: "job-yesterday" });
  line(1, 16, 30, "files.remove", { targetId: "ui-test", path: "/tmp/cache" }, "E_PERMISSION", "merge: Branch Master");
  line(1, 23, 50, "jobs.start", { targetId: "lab-pc", jobId: "job-lab" }, "ok", "cli");
  line(0, 0, 20, "groups.run", { selector }, "ok", "mcp");
  line(0, 0, 40, "terminal.open", { targetId: "ui-test" });
}
function activityFilter(p) {
  const today = thaiDate(Date.now());
  let from = -Infinity, to = Infinity;
  const day = d => thaiMidnight(d);
  if (p.range === "today") { from = day(today); to = from + DAY; }
  else if (p.range === "yesterday") { to = day(today); from = to - DAY; }
  else if (/^\d+d$/.test(p.range || "")) { to = day(today) + DAY; from = to - parseInt(p.range, 10) * DAY; }
  else if (p.from || p.to) { if (p.from) from = day(p.from); to = day(p.to || today) + DAY; }
  return { from, to, hosts: Array.isArray(p.hosts) ? new Set(p.hosts) : null, text: String(p.text || "").trim().toLowerCase(), status: p.status || "all" };
}
const FINISHED = ["succeeded", "failed", "cancelled", "interrupted", "unreachable"];
const statusMatches = (wanted, status) => wanted === "succeeded" ? status === "succeeded" : wanted === "failed" ? ["failed", "cancelled", "interrupted", "unreachable"].includes(status)
  : wanted === "running" ? !FINISHED.includes(status) : true;
function auditList(p) {
  const f = activityFilter(p);
  return s.activity.audit.filter(line => {
    const at = Date.parse(line.time); if (at < f.from || at >= f.to) return false;
    const host = line.object.split(" · ")[0];
    const byHost = f.hosts?.has(host), byText = f.text && (line.action.toLowerCase().includes(f.text) || line.object.toLowerCase().includes(f.text));
    return (!f.hosts && !f.text) || byHost || byText;
  }).reverse().map(line => ({ ...line }));
}
function jobList(p) {
  const f = activityFilter(p), out = [], groups = new Map();
  const rows = s.activity.jobs.filter(j => j.sentMs >= f.from && j.sentMs < f.to).sort((a, b) => b.sentMs - a.sentMs);
  const matches = j => (!f.hosts || f.hosts.has(j.targetId)) && statusMatches(f.status, j.status);
  const view = ({ sentMs, ...j }) => ({ ...j });
  for (const j of rows) {
    if (!j.groupId) { if (matches(j)) out.push({ kind: "job", job: view(j) }); continue; }
    if (!groups.has(j.groupId)) { const g = { kind: "group", groupId: j.groupId, selector: j.selector, channel: j.channel, hosts: [], hit: false }; groups.set(j.groupId, g); out.push(g); }
    const g = groups.get(j.groupId); g.hosts.push(view(j)); if (matches(j)) g.hit = true;
  }
  return out.filter(e => e.kind !== "group" || e.hit).map(e => { if (e.kind !== "group") return e; const { hit, ...g } = e; g.hosts.reverse(); g.sentAt = g.hosts[0].sentAt; return g; });
}
/** One refresh: lab-pc never answers; the live job finishes; other unfinished jobs move one step. */
function refreshActivity(p) {
  for (const entry of jobList(p)) for (const shown of entry.kind === "group" ? entry.hosts : [entry.job]) {
    if (FINISHED.includes(shown.status)) continue;
    const row = s.activity.jobs.find(j => j.jobId === shown.jobId);
    if (row.targetId === "lab-pc") { row.error = "ALL_ROUTES_FAILED: lan 192.0.2.99:22: No route to host"; continue; }
    const job = s.jobs.find(j => j.id === row.jobId) || s.groupRuns.flatMap(r => r.hosts).find(h => h.jobId === row.jobId);
    if (job?.live) { job.status = "succeeded"; job.exitCode = 0; job.live = false; }
    if (job) Object.assign(row, { status: job.status, exitCode: job.exitCode ?? null, error: null });
  }
}
const cell = v => { const t = v == null ? "" : String(v); return /[",\r\n]/.test(t) ? '"' + t.replace(/"/g, '""') + '"' : t; };
const csv = (head, rows) => "﻿" + [head, ...rows].map(r => r.map(cell).join(",")).join("\r\n") + "\r\n";
const thaiText = v => v && !Number.isNaN(Date.parse(v)) ? stamp(Date.parse(v)) : (v ?? "");
function activityCsv(p) {
  if (p.view === "audit") return csv(["time", "action", "object", "result", "channel", "user"], auditList(p).map(l => [l.time, l.action, l.object, l.result, l.channel, l.user]));
  const jobs = jobList(p).flatMap(e => e.kind === "group" ? e.hosts : [e.job]);
  return csv(["time", "host", "group", "status", "exit_code", "started", "finished", "cwd", "as_admin", "channel", "job_id", "script"],
    jobs.map(j => [j.sentAt, j.targetId, j.groupId, j.status, j.exitCode, thaiText(j.startedAt), thaiText(j.finishedAt), j.cwd, j.asAdmin, j.channel, j.jobId, j.script]));
}
const activityStatus = () => ({ keepDays: s.activity.keepDays, since: s.activity.since, warning: flags.has("activity-warning") });
function rpc(method, p = {}) {
  switch (method) {
    case "monitor.stats": {
      // Counters advance each call so the window can draw rates.
      s.ticks = (s.ticks || 0) + 1;
      const G = 1024 ** 3, t = s.ticks, part = (path, fs, total, used) => ({ path, fs, total: total * G, free: (total - used) * G });
      const disks = [part("/", "ext4", 100, 41), part("/var", "xfs", 200, 176), part("/boot/efi", "vfat", 1, 0.1)];
      return { os: "linux", at: Date.now(), uptimeSeconds: 86400 + t * 5, cpu: { total: 1000 * t, idle: 700 * t, count: 4 }, memory: { total: 16 * G, available: 9 * G },
        swap: { total: 2 * G, free: 2 * G }, load: [0.4, 0.3, 0.2], disk: disks[0], ...(flags.has("old-disk") ? {} : { disks }), net: { rx: 1e6 * t, tx: 4e5 * t } };
    }
    case "targets.update_check": return updateCheck();
    case "targets.upgrade": return upgradeOne(p.targetId);
    case "targets.upgrade_all": s.upgradeRun = { done: false, hosts: updateCheck().hosts.filter(r => r.canUpdate).map(r => ({ targetId: r.targetId, state: "waiting", error: null })) }; return { started: true };
    case "targets.upgrade_status": return upgradeStatus();
    case "alarms.list": return alarmList();
    case "alarms.rules": return { rules: structuredClone(s.alarm.rules), interval: s.alarm.interval };
    case "alarms.rule_set": return alarmRule(p);
    case "alarms.rule_delete": s.alarm.rules = s.alarm.rules.filter(r => r.id !== p.id); s.alarm.alarms = s.alarm.alarms.filter(a => a.rule !== p.id); return { id: p.id, deleted: true };
    case "alarms.settings": if (![5, 15, 30].includes(p.interval)) throw new Error("E_ALARM_INTERVAL: check every 5, 15 or 30 minutes"); s.alarm.interval = p.interval; return { interval: p.interval };
    case "alarms.ack": { const a = s.alarm.alarms.find(row => row.id === p.id); a.acknowledged = true; return a; }
    case "alarms.updates": return structuredClone(s.alarm.updates);
    case "alarms.host": return { alarms: alarmList().alarms.filter(a => a.host === p.targetId), updates: s.alarm.updates[p.targetId] || null };
    case "alarms.check_updates": {
      // The check "finishes" at once: ui-test has one package left, any other host none.
      for (const t of [...s.targets, ...relayedTargets()].filter(t => p.targetId === "all" || t.id === p.targetId))
        s.alarm.updates[t.id] = t.access === "view" ? { count: null, packages: [], error: "view-only", checkedAt: stamp(Date.now()), checkNow: false }
          : { count: t.id === "ui-test" ? 1 : 0, packages: t.id === "ui-test" ? ["openssl"] : [], error: null, checkedAt: stamp(Date.now() + 1000), checkNow: false };
      return { queued: p.targetId };
    }
    case "activity.status": return activityStatus();
    case "activity.settings": {
      const days = Number(p.keepDays);
      if (!Number.isInteger(days) || days < 1 || days > 3650) throw new Error("E_ACTIVITY_DAYS: keep 1-3650 days");
      s.activity.keepDays = days; return activityStatus();
    }
    case "activity.audit": return { lines: auditList(p), status: activityStatus() };
    case "activity.jobs": if (p.refresh) refreshActivity(p); return { jobs: jobList(p), status: activityStatus() };
    case "activity.csv": return { csv: activityCsv(p) };
    case "groups.preview": return groupHosts(p.selector);
    case "groups.run": return startGroup(p);
    case "groups.status": return p.runId ? groupStatus(p.runId) : { runs: structuredClone(s.groupRuns) };
    case "groups.cancel": {
      const run = s.groupRuns.find(r => r.id === p.runId);
      for (const h of run.hosts.filter(h => (!p.targetId || h.targetId === p.targetId) && h.state === "started" && ["queued", "running"].includes(h.status))) h.status = "cancelled";
      return structuredClone(run);
    }
    // Retry: hosts that failed get new jobs; the unreachable one is back online by then.
    case "groups.retry": {
      const run = s.groupRuns.find(r => r.id === p.runId);
      for (const h of run.hosts.filter(h => h.state === "unreachable" || ["failed", "interrupted", "cancelled"].includes(h.status)))
        Object.assign(h, { state: "started", jobId: crypto.randomUUID(), error: null, status: "queued", exitCode: null, retried: true });
      run.done = false;
      return structuredClone(run);
    }
    case "tags.list": return tagList();
    case "tags.set": return setTags(p);
    case "keys.rotate":
      if (!s.rotation || s.rotation.finishedAt) s.rotation = { startedAt: now(), finishedAt: null, started: false, hosts: s.targets.filter(t => !t.relayed).map(t => ({ targetId: t.id, label: t.label, state: "pending", error: null })) };
      // The slow host goes first, so a check can see it fail before the rest finish.
      s.rotation.hosts.sort((a, b) => (b.targetId === "ui-test") - (a.targetId === "ui-test"));
      return rotationStatus();
    case "keys.rotation_status": return rotationStatus();
    case "system.status": return { label: "UI fixture (no real service)", nodeId: "fixture", roles: ["master", "target"], version: "test", uptimeSeconds: 120 };
    case "backup.status": return { ...s.backup, backupBeforeKeys: Boolean(s.backup.keysChangedAt && (!s.backup.lastBackupAt || s.backup.lastBackupAt < s.backup.keysChangedAt)), empty: isEmpty(), owner: true };
    case "backup.create": { const file = makeBackup(p.passphrase); s.backup.lastBackupAt = new Date(Date.now() + 1).toISOString(); return { file }; }
    case "backup.inspect": return backupSummary(openBackup(p.file, p.passphrase));
    case "backup.restore": return restoreBackup(p);
    case "targets.list": return [...s.targets, ...relayedTargets()];
    case "targets.doctor": return { targetId: p.targetId, ok: true, stages: [{ name: "fixture", ok: true, message: "Isolated UI test only" }] };
    case "targets.update": { const t = s.targets.find(row => row.id === p.target?.id); Object.assign(t, p.target); return t; }
    case "jobs.list": return s.jobs;
    case "jobs.start": {
      if (flags.has("multi-hop") && p.targetId === "branch-master/lab-master/lab-web") throw new Error("E_RELAY_TIMEOUT: Lab Master didn't answer in time");
      if (flags.has("merge-refuses") && p.targetId === "branch-master/child-2") throw new Error("E_MERGE_ROLE: Branch Master gives this Master only hosts with Tag web");
      let job = s.jobs.find(row => row.id === p.jobId);
      if (!job) { job = { id: p.jobId, masterId: MASTER_ID, script: p.script, cwd: p.cwd, shell: p.shell, status: "running", createdAt: now(), startedAt: now(), exitCode: null }; s.jobs.unshift(job); }
      if (firstStart) { firstStart = false; const error = new Error("Simulated timeout AFTER acceptance. Retry same form."); error.status = 504; throw error; }
      return job;
    }
    case "jobs.status":
      if (p.targetId === "lab-pc") throw new Error("E_ROUTE: ALL_ROUTES_FAILED: lan 192.0.2.99:22: No route to host");
      return s.jobs.find(job => job.id === p.jobId);
    case "jobs.logs": { const log = p.stream === "stderr" ? "Fixture stderr\n" : "Fixture output <script>escaped</script>\n"; return { content: log.slice(p.offset || 0), nextOffset: log.length, eof: true }; }
    case "jobs.cancel": { const job = s.jobs.find(row => row.id === p.jobId); job.status = "cancelled"; return job; }
    case "files.list": return [{ name: "example.txt", path: "/tmp/example.txt", isDirectory: false, isSymlink: false, size: Buffer.byteLength(content) }];
    case "files.read": return { contentBase64: Buffer.from(content).toString("base64"), size: Buffer.byteLength(content), sha256: sha(), nextOffset: Buffer.byteLength(content), eof: true };
    case "files.write":
      if (p.expectedSha256 !== sha()) throw new Error("optimistic write precondition missing");
      content = Buffer.from(p.contentBase64, "base64").toString("utf8"); return { sha256: sha() };
    case "enrollment.export": return { schemaVersion: 2, ...p, master: { id: MASTER_ID, label: "Fixture", publicKey: "TEST PUBLIC KEY" } };
    case "fleet.health": return s.targets.map(healthRow);
    // Pairing (Master side). A request is "approved" when the test reads its code from GET /pairing-code.
    case "pairing.create": {
      const label = need(p.label, "label"), address = need(p.address, "address"), port = Number(p.port || 22);
      const targetId = p.targetId || slug(label) + "-" + crypto.randomBytes(2).toString("hex");
      const routes = p.routes || [{ kind: p.routeKind || "lan", address, port }];
      const make = os => ({ requestId: crypto.randomUUID(), targetId, label, tenant: p.tenant || "lab", os, user: p.user || "sshcc-bridge", routes, state: "awaiting_approval", createdAt: now(), updatedAt: now() });
      if (need(p.os, "os") !== "unknown") { const row = make(p.os); s.pairings.unshift(row); return pairingView(row); }
      // Unknown OS: a Windows and an Ubuntu request for the same host, linked as siblings.
      const pair = [make("windows"), make("linux")]; pair[0].sibling = pair[1].requestId; pair[1].sibling = pair[0].requestId;
      s.pairings.unshift(...pair); return pair.map(pairingView);
    }
    case "pairing.status":
      if (!p.requestId) return s.pairings.map(pairingView);
      if (p.routes) { const row = pairing(p.requestId), t = s.targets.find(item => item.id === row.targetId); if (t) t.routes = p.routes; row.routes = p.routes; row.state = "ready"; row.updatedAt = now(); }
      return pairingView(pairing(p.requestId));
    case "pairing.preview": return previewPairing(p);
    case "pairing.fetch": {
      const row = pairing(need(p.requestId, "requestId"));
      if (!flags.has("six-digit")) return { state: "old", requestId: row.requestId };
      if (row.state !== "awaiting_approval") return { state: "done", requestId: row.requestId };
      if (!row.approved) return { state: "waiting", requestId: row.requestId };
      const code = pairingCode(row.requestId);
      const n = parseInt(crypto.createHash("sha256").update(code).digest("hex").slice(0, 8), 16) % 1000000, digits = String(n).padStart(6, "0");
      return { state: "ready", requestId: row.requestId, digits: digits.slice(0, 3) + " " + digits.slice(3), preview: previewPairing({ code }) };
    }
    case "pairing.confirm": {
      const row = pairing(need(p.requestId, "requestId"));
      if (p.same !== true) { s.pairings = s.pairings.filter(item => item !== row && item.requestId !== row.sibling); throw new Error("E_PAIRING_MISMATCH: the numbers differ, so this pairing was cancelled"); }
      return completePairing({ code: pairingCode(row.requestId), label: p.label });
    }
    case "pairing.complete": return completePairing(p);
    case "pairing.check": return checkAddress(need(p.address, "address").trim(), Number(p.port || 22), p.requestId);
    case "pairing.cancel": { const row = pairing(need(p.requestId, "requestId")); s.pairings = s.pairings.filter(item => item !== row); return { requestId: row.requestId, cancelled: true }; }
    // Folders and datacenters (tree organisation only).
    case "folders.list": return { folders: s.folders, members: s.folderMembers };
    case "folders.create": { const folder = { id: crypto.randomUUID(), master: p.master || "local", name: need(p.name, "name") }; s.folders.push(folder); return folder; }
    case "folders.rename": { const folder = s.folders.find(row => row.id === p.id); folder.name = need(p.name, "name"); return { id: p.id, name: p.name }; }
    case "folders.delete": s.folders = s.folders.filter(row => row.id !== p.id); for (const [k, v] of Object.entries(s.folderMembers)) if (v === p.id) delete s.folderMembers[k]; return { id: p.id, deleted: true };
    case "folders.assign":
      if (p.folderId) { if (!s.folders.some(row => row.id === p.folderId)) throw new Error("E_ARGUMENT: unknown folder"); s.folderMembers[need(p.targetId, "targetId")] = p.folderId; }
      else delete s.folderMembers[p.targetId];
      return { targetId: p.targetId, folderId: p.folderId || null };
    case "datacenters.list": return { datacenters: s.datacenters, members: s.datacenterMembers };
    case "datacenters.create": { const site = { id: crypto.randomUUID(), name: need(p.name, "name") }; s.datacenters.push(site); return site; }
    case "datacenters.assign": if (p.datacenterId) s.datacenterMembers[p.master] = p.datacenterId; else delete s.datacenterMembers[p.master]; return { master: p.master, datacenterId: p.datacenterId || null };
    // Merges (this Master controls another Master's hosts) and relays (other Masters that merged this one).
    case "merges.list": return s.merges;
    case "merges.discover": return s.targets.filter(t => !p.targetId || t.id === p.targetId).map(t => ({ targetId: t.id, label: t.label, isMaster: isMasterHost(t.id), version: "test" }));
    case "merges.add": case "merges.sync": return mergeAdd(need(p.targetId, "targetId"));
    case "merges.remove": s.merges = s.merges.filter(row => row.targetId !== p.targetId); return { targetId: p.targetId, removed: true };
    case "relays.list": return s.relays;
    case "relays.set": {
      const relay = s.relays.find(row => row.masterId === p.masterId) || (() => { throw new Error("unknown merged Master"); })();
      if ("role" in p) { if (!["full", "view"].includes(p.role)) throw new Error("E_ARGUMENT: role is full or view"); relay.role = p.role; }
      if ("tags" in p) relay.tags = p.tags === null ? null : [...new Map(p.tags.map(tag => [tag.trim().toLowerCase(), tag.trim()])).values()];
      if ("passThrough" in p) relay.passThrough = Boolean(p.passThrough);
      return { ...relay };
    }
    case "relays.revoke": case "relays.allow": { const relay = s.relays.find(row => row.masterId === p.masterId); relay.revoked = method === "relays.revoke"; return relay; }
    // Allowed Masters (Masters that may control this host directly).
    case "masters.list": return s.masters;
    case "masters.revoke": { const master = s.masters.find(row => row.id === p.masterId); master.revoked = true; return master; }
    case "enrollment.accept": { const master = { id: need(p.bundle?.master?.id, "bundle.master.id"), label: p.bundle.master.label || p.bundle.master.id, publicKey: p.bundle.master.publicKey || "TEST", revoked: false }; s.masters = s.masters.filter(row => row.id !== master.id).concat(master); return master; }
    default: throw new Error("Unsupported fixture method: " + method);
  }
}
/** One page request, recorded as the Master records it: audited if it changes something, jobs it starts kept. */
function recorded(method, p) {
  let result;
  try { result = rpc(method, p); }
  catch (error) { if (audited(method, p)) audit(method, p, (/^E_[A-Z_]+/.exec(error.message) || ["E_OPERATION"])[0]); throw error; }
  if (audited(method, p)) audit(method, p, "ok");
  if (method === "jobs.start") logJob({ jobId: result.id, targetId: p.targetId, script: p.script, cwd: p.cwd, shell: p.shell, asAdmin: p.asAdmin, env: p.env, status: result.status });
  if (method === "groups.run" || method === "groups.retry") for (const h of result.hosts.filter(h => h.state === "started" || h.state === "unreachable"))
    if (h.state === "unreachable" || !s.activity.jobs.some(j => j.jobId === h.jobId))
      logJob({ jobId: h.jobId || crypto.randomUUID(), targetId: h.targetId, label: h.label, groupId: result.id, selector: result.selector, script: result.scripts[h.os], cwd: h.cwd, asAdmin: result.asAdmin, env: result.env, status: h.state === "unreachable" ? "unreachable" : h.status, error: h.error });
  if (method === "groups.status" && p.runId) for (const h of result.hosts) { const row = s.activity.jobs.find(j => j.jobId === h.jobId); if (row) Object.assign(row, { status: h.status, exitCode: h.exitCode }); }
  return result;
}
/** Native (Tauri) commands other than "rpc". */
function command(name, args) {
  if (name === "open_shell") return !args.ownerSessionId && process.env.SSHCC_FIXTURE_SINGLE_SESSION !== "1"
    ? { needsSession: true, sessions: [{ id: "3", account: "fixture-owner", state: "active" }, { id: "7", account: "fixture-owner", state: "online" }] }
    : { opened: true, pid: 999, targetId: args.targetId, ownerSessionId: args.ownerSessionId || "3" };
  if (name === "open_json" || name === "save_json") return null;
  // Stands in for the native save dialog: always "saves" to a fixed folder, never writes a file.
  // A sibling pair asks for a folder once and "writes" both installers there.
  if (name === "save_setup") {
    const row = pairing(need(args.requestId, "requestId")), folder = "/fixture/Downloads";
    const file = r => ({ path: folder + "/sshcc-client-" + r.targetId + "." + (r.os === "windows" ? "exe" : "deb"), sha256: "0".repeat(64), size: 1, requestId: r.requestId, os: r.os });
    if (!row.sibling) return file(row);
    const files = [row, pairing(row.sibling)].sort((a, b) => (a.os === "windows" ? -1 : 1) - (b.os === "windows" ? -1 : 1)).map(file);
    return { folder, paths: files.map(f => f.path), files };
  }
  // Two-way: the other Master enrolls this one and merges it back; it shows as a relay here.
  if (name === "merge_two_way") {
    const merge = s.merges.find(row => row.targetId === need(args.targetId, "targetId")), t = s.targets.find(row => row.id === args.targetId);
    merge.reverse = { status: "two-way", error: null };
    s.relays = s.relays.filter(row => row.masterId !== t.nodeId).concat({ masterId: t.nodeId, label: t.label, lastUsedAt: now(), revoked: false });
    return { status: "two-way", children: s.targets.length };
  }
  // Stands in for the native save dialog: "saves" to /fixture/Backups under the default name.
  if (name === "save_backup") {
    const file = rpc("backup.create", { passphrase: args.passphrase }).file, path = "/fixture/Backups/" + need(args.name, "name");
    savedBackup = { name: args.name, file }; return path;
  }
  // Stands in for the native open dialog: the last saved backup, or the kind asked for with POST /next-backup.
  if (name === "open_backup") {
    const kind = nextBackup; nextBackup = "";
    if (kind === "other") return { name: "notes.sshccbak", file: Buffer.from("not a backup").toString("base64") };
    // "good": a backup of this fixture's default fleet, passphrase "fixture passphrase".
    if (kind) { const base = JSON.parse(Buffer.from(makeBackup("fixture passphrase"), "base64").toString("utf8")); if (kind === "newer") base.formatVersion = 2; if (kind === "damaged") base.damaged = true; return { name: kind + ".sshccbak", file: Buffer.from(JSON.stringify(base)).toString("base64") }; }
    return savedBackup ? { ...savedBackup } : null;
  }
  if (name === "save_upgrade") return "/fixture/Downloads/" + (args.os === "windows" ? "sshcc-update.exe" : "sshcc_0.5.0.deb");
  if (name === "save_csv") { lastCsv = { name: args.name, csv: rpc("activity.csv", args.params || {}).csv }; return "/fixture/Downloads/" + args.name; }
  if (name === "rpc") return recorded(args.method, args.params || {});
  throw new Error("Unsupported fixture command: " + name);
}
const bootstrap = 'try{if(localStorage.getItem("sshcc.sideHosts")===null)localStorage.setItem("sshcc.sideHosts","open")}catch{};window.__TAURI__={core:{invoke:async(command,args)=>{const r=await fetch("/rpc",{method:"POST",headers:{"content-type":"application/json"},body:JSON.stringify({command,args})});const data=await r.json();if(!r.ok)throw new Error(data.error);return data;}}};';
const json = (res, value) => { res.setHeader("content-type", "application/json"); res.end(JSON.stringify(value === undefined ? null : value)); };
const body = async req => { let text = ""; for await (const chunk of req) text += chunk; return text ? JSON.parse(text) : {}; };
const server = http.createServer(async (req, res) => {
  try {
    const url = new URL(req.url, "http://fixture");
    if (url.pathname === "/calls") return json(res, calls);
    if (url.pathname === "/last-csv") return json(res, lastCsv);
    if (url.pathname === "/scenario" && req.method === "POST") { reset((await body(req)).flags || []); return json(res, { flags: [...flags] }); }
    if (url.pathname === "/next-backup" && req.method === "POST") { nextBackup = (await body(req)).kind || ""; return json(res, { kind: nextBackup }); }
    if (url.pathname === "/pairing-code") { const row = url.searchParams.get("requestId") ? pairing(url.searchParams.get("requestId")) : s.pairings.find(p => p.state === "awaiting_approval") || pairing(null); row.approved = true; return json(res, { code: pairingCode(row.requestId, url.searchParams.get("master") === "other" ? "other-master" : MASTER_ID) }); }
    if (url.pathname === "/rpc" && req.method === "POST") {
      const { command: name, args = {} } = await body(req);
      calls.push({ command: name, ...args });
      return json(res, await command(name, args));
    }
    const files = { "/": "index.html", "/app.css": "app.css", "/app.js": "app.js", "/strings.js": "strings.js", "/sky.js": "sky.js", "/sky.css": "sky.css" };
    if (url.pathname === "/fixture.js") { res.setHeader("content-type", "application/javascript"); return res.end(bootstrap); }
    // Bundled fonts (Latin and the Thai faces), so screenshots show the real type.
    const font = /^\/fonts\/([a-z0-9-]+\.woff2)$/.exec(url.pathname);
    if (font && fs.existsSync(path.join(ui, "fonts", font[1]))) { res.setHeader("content-type", "font/woff2"); return res.end(fs.readFileSync(path.join(ui, "fonts", font[1]))); }
    const file = files[url.pathname]; if (!file) { res.statusCode = 404; return res.end(); }
    const types = { ".html": "text/html", ".css": "text/css", ".js": "application/javascript" };
    let page = fs.readFileSync(path.join(ui, file), "utf8");
    if (file === "index.html") page = page.replace('<script src="app.js"', '<script src="/fixture.js"></script><script src="app.js"');
    res.setHeader("content-type", types[path.extname(file)] + "; charset=utf-8"); res.end(page);
  } catch (error) { res.statusCode = error.status || 400; json(res, { error: error.message }); }
});
// After every helper is defined (the Activity seed uses them).
reset((process.env.SSHCC_FIXTURE_SCENARIO || "").split(","));
server.listen(Number(process.env.SSHCC_UI_TEST_PORT || 42197), "127.0.0.1", () => console.log("SSHCC isolated UI fixture: http://127.0.0.1:" + server.address().port));
