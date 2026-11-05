import { esc, table, LANGUAGES } from "./strings.js";
import { createSky } from "./sky.js";
const $ = (selector, parent = document) => parent.querySelector(selector);
const pretty = value => JSON.stringify(value, null, 2);
document.documentElement.dataset.theme = "dark";
/* Language EN | ไทย: the first run follows the OS language (the webview reports it), then the choice is remembered.
   Each Master has its own window and webview data, so this is per Master. `T` is the current table (strings.js). */
const LANGUAGE_KEY = "sshcc.language";
const savedLanguage = () => { try { const value = localStorage.getItem(LANGUAGE_KEY); return LANGUAGES.includes(value) ? value : null; } catch { return null; } };
const osLanguage = () => /^th(-|$)/i.test(navigator.language || "") ? "th" : "en";
let lang = savedLanguage() || osLanguage(), T = table(lang), addText = T.add, mastersText = T.masters;
document.documentElement.lang = lang;
const state = {
  page: "datacenter", targets: [], masters: [], service: null, serviceError: "", selected: null,
  tab: "overview", query: "", group: "", status: "", diagnostics: new Map(), jobs: [], jobsError: "",
  job: null, stream: "stdout", log: "", offset: 0, eof: true, polling: false, newJob: false,
  export: null, pendingJob: null, file: null, files: null, pairing: null, pairings: [], saved: new Map(), add: null,
  merges: [], relays: [], merging: new Set(), masterCheck: new Map(),
  view: localStorage.getItem("sshcc.fleetView") === "table" ? "table" : "graph",
  invTab: "all", health: new Map(), healthError: "", healthLoading: false, healthAt: null, healthStarted: 0, flipped: new Set(), twoWay: new Map(),
  clientLinks: [], clientLinksError: "",
  // Datacenter: tree selection, collapsed branches and on-demand container data per machine ("local" = this Master).
  dc: { treeHidden: (() => { try { return localStorage.getItem("sshcc.sideHosts") !== "open"; } catch { return true; } })(), node: "dc", tab: "summary", query: "", collapsed: new Set(), seen: new Set(), filter: "running", adding: null, editing: null, ctView: "containers",
    monInterval: Number(localStorage.getItem("sshcc.monitorInterval")) || 5000, monPaused: false },
  monitor: new Map(),
  containers: new Map(), ctLogs: null, folders: { folders: [], members: {} }, datacenters: { datacenters: [], members: {} },
  // Backup and restore: the service's status line, and the open flow (null, or {mode: "backup" | "restore", …}).
  backupStatus: null, backup: null, rotation: null,
  // Tags: {tags: [{tag, hosts}], hosts: {id: [tag]}}; tagError: the last refused change, shown under the Tag box.
  tags: { tags: [], hosts: {} }, tagError: null,
  // Group runs: the draft waiting in the confirmation, and the run shown in place of the New job form.
  groupDraft: null, groupRun: null, activity: null,
  alarms: { alarms: [], red: 0, yellow: 0 }, alarmRules: null, alarmInterval: 5, alarmTab: "active", alarmEdit: null, updates: {}, seenRed: null
};
addReset();
const native = (command, args = {}) => {
  if (!window.__TAURI__?.core?.invoke) throw new Error(T.common.openApp);
  return window.__TAURI__.core.invoke(command, args);
};
const rpc = (method, params = {}) => native("rpc", { method, params });
const target = () => state.targets.find(item => item.id === state.selected);
const isWindows = item => /win/i.test(item?.os || "");
const osName = os => ({ windows: "Windows", linux: "Linux" })[os] || os || "";
const timestamp = value => {
  if (!value) return "—";
  const date = new Date(typeof value === "number" ? value * 1000 : value);
  return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString(T.locale, { dateStyle: "short", timeStyle: "short" });
};
const formatSize = bytes => {
  if (bytes == null || Number.isNaN(Number(bytes))) return "—";
  let value = Number(bytes), unit = 0;
  const units = ["B", "KB", "MB", "GB", "TB"];
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit++; }
  return (unit && value < 10 ? value.toFixed(1) : Math.round(value)) + " " + units[unit];
};
const formatUptime = seconds => {
  const minutes = Math.floor((seconds || 0) / 60), hours = Math.floor(minutes / 60), days = Math.floor(hours / 24);
  const text = T.monitor.uptime;
  if (days) return text.d(days, hours % 24);
  if (hours) return text.h(hours, minutes % 60);
  return text.m(minutes);
};
const svg = paths => '<svg class="i" viewBox="0 0 16 16" aria-hidden="true">' + paths + "</svg>";
const icons = {
  graph: svg('<circle cx="3.5" cy="8" r="2"/><circle cx="12.5" cy="3.5" r="2"/><circle cx="12.5" cy="12.5" r="2"/><path d="M5.5 8h2.5M8 3.5v9M8 3.5h2.5M8 12.5h2.5"/>'),
  table: svg('<rect x="1.75" y="2.75" width="12.5" height="10.5" rx="1.5"/><path d="M1.75 6.25h12.5M1.75 9.75h12.5M6 6.25v7"/>'),
  shell: svg('<rect x="1.5" y="2.5" width="13" height="11" rx="1.5"/><path d="M4.5 6.5 6.5 8.25 4.5 10M8 10.25h3.5"/>'),
  both: svg('<path d="M2.5 5.5h10M10 3l2.5 2.5L10 8M13.5 10.5h-10M6 8l-2.5 2.5L6 13"/>'),
  one: svg('<path d="M2.5 8h10M10 5.5 12.5 8 10 10.5"/>'),
  plus: svg('<path d="M8 3v10M3 8h10"/>'),
  search: svg('<circle cx="7" cy="7" r="4.25"/><path d="m10.25 10.25 3.25 3.25"/>'),
  refresh: svg('<path d="M13.25 8A5.25 5.25 0 1 1 11.7 4.3"/><path d="M13.25 2.25v3.5h-3.5"/>'),
  back: svg('<path d="M13 8H3.5M7.5 4 3.5 8l4 4"/>'),
  more: svg('<circle cx="8" cy="3.5" r="1.1"/><circle cx="8" cy="8" r="1.1"/><circle cx="8" cy="12.5" r="1.1"/>'),
  up: svg('<path d="M8 13V3.5M4 7.5l4-4 4 4"/>'),
  chevron: svg('<path d="m6 3.5 4.5 4.5L6 12.5"/>'),
  folder: svg('<path d="M1.75 4.25c0-.7.55-1.25 1.25-1.25h3l1.5 1.75H13c.7 0 1.25.55 1.25 1.25v6c0 .7-.55 1.25-1.25 1.25H3c-.7 0-1.25-.55-1.25-1.25Z"/>'),
  file: svg('<path d="M4 1.75h5l3.25 3.25v8c0 .7-.55 1.25-1.25 1.25H4c-.7 0-1.25-.55-1.25-1.25V3c0-.7.55-1.25 1.25-1.25Z"/><path d="M9 1.75V5h3.25"/>'),
  copy: svg('<rect x="5.25" y="5.25" width="8.5" height="8.5" rx="1.5"/><path d="M10.75 5.25V3.5c0-.7-.55-1.25-1.25-1.25H3.5c-.7 0-1.25.55-1.25 1.25v6c0 .7.55 1.25 1.25 1.25h1.75"/>'),
  check: svg('<path d="m3.25 8.25 3 3 6.5-6.5"/>'),
  lock: svg('<rect x="3" y="7" width="10" height="7" rx="1.5"/><path d="M5.25 7V5a2.75 2.75 0 0 1 5.5 0v2"/>'),
  alert: svg('<path d="M8 2 1.5 13.5h13Z"/><path d="M8 6.5v3.25M8 11.75h.01"/>'),
  info: svg('<circle cx="8" cy="8" r="6.25"/><path d="M8 7.25v4M8 4.75h.01"/>'),
  pulse: svg('<path d="M1.5 8.5h3l1.75-4.5 3.5 8 1.75-3.5h3"/>'),
  close: svg('<path d="m4 4 8 8M12 4l-8 8"/>')
};
const chip = text => '<span class="chip">' + esc(text) + "</span>";
/** A notice; `detail` is the Master's English text under a translated error sentence (Thai only, see `explain`). */
function notify(message, error = false, detail = "") {
  const notice = $("#notice"), text = $("#notice-text");
  text.textContent = String(message); notice.classList.toggle("error", error); notice.hidden = false;
  if (detail) { const small = document.createElement("small"); small.className = "error-detail"; small.textContent = detail; text.append(small); }
}
function clearNotice() { $("#notice").hidden = true; }
const notifyError = (error, pairing = false) => { const shown = explain(error, pairing); notify(shown.text, true, shown.detail); };
async function copyText(text, message = T.common.copied) {
  try { await navigator.clipboard.writeText(String(text ?? "")); }
  catch { throw new Error(T.common.copyFailed); }
  notify(message);
}
async function openShell(targetId, ownerSessionId) {
  const item = state.targets.find(row => row.id === targetId);
  if (!item) throw new Error(T.common.notInRegistry);
  const result = await native("open_shell", { targetId, ownerSessionId: ownerSessionId || null });
  if (result.needsSession) {
    const dialog = $("#shell-dialog");
    dialog.dataset.machine = item.label || item.id; state.shellSessions = result.sessions;
    $("#shell-target").value = item.id;
    fillShellDialog();
    dialog.showModal();
    return;
  }
  notify(T.shell.opened(item.label || item.id));
}
/** The session dialog's words; also re-run when the language changes while it is open (the choice is kept). */
function fillShellDialog() {
  const dialog = $("#shell-dialog"), select = $("#shell-session"), chosen = select.value;
  $("#shell-title").textContent = T.shell.title(dialog.dataset.machine || "");
  select.innerHTML = '<option value="">' + esc(T.shell.choose) + "</option>" + (state.shellSessions || []).map(session => '<option value="' + esc(session.id) + '">' + esc(T.shell.option(session.account, session.id, session.state)) + "</option>").join("");
  select.value = chosen;
}
function badge(status) {
  const styles = { succeeded: "good", failed: "bad", interrupted: "bad", cancelled: "", running: "running", queued: "pending" };
  return '<span class="badge ' + (styles[status] || "") + '">' + esc(T.status.jobs[status] || status) + "</span>";
}
async function busy(button, work) {
  if (button?.disabled) return;
  if (button) button.disabled = true;
  clearNotice();
  try { return await work(); }
  catch (error) { notifyError(error, state.page === "enroll" || Boolean(state.dc.adding)); }
  finally { if (button?.isConnected) button.disabled = button.hasAttribute("data-blocked"); }
}
/* The switch: every string comes from `T`, so changing language is a re-render from state. Static words in
   index.html carry data-i18n keys; open dialogs are refilled; what the person typed is put back. */
const lookup = path => path.split(".").reduce((node, key) => node?.[key], T);
function applyStatic() {
  for (const element of document.querySelectorAll("[data-i18n]")) element.textContent = lookup(element.dataset.i18n);
  for (const [key, attribute] of [["i18nAria", "aria-label"], ["i18nTitle", "title"], ["i18nPlaceholder", "placeholder"]])
    for (const element of document.querySelectorAll("[data-" + key.replace(/[A-Z]/g, c => "-" + c.toLowerCase()) + "]")) element.setAttribute(attribute, lookup(element.dataset[key]));
  // Two-way dialog: the other Master's name goes where `{peer}` stands, bold in the lead sentence.
  const peer = $("#two-way-dialog").dataset.peer || "";
  for (const element of document.querySelectorAll("[data-i18n-peer]")) {
    element.replaceChildren(...lookup(element.dataset.i18nPeer).split("{peer}").flatMap((part, i) => {
      const name = document.createElement(element.tagName === "P" ? "strong" : "span");
      name.className = "two-way-peer"; name.textContent = peer;
      return i ? [name, part] : [part];
    }));
  }
  for (const button of document.querySelectorAll("[data-lang]")) button.setAttribute("aria-pressed", String(button.dataset.lang === lang));
  if ($("#shell-dialog").open) fillShellDialog();
  if ($("#delete-dialog").open) fillDeleteDialog();
  if ($("#group-dialog").open) fillGroupDialog();
}
/** Snapshot of the typed values, open disclosures and focus under `root`; the returned function puts them back
 *  after a re-render (fields are matched by form and name). */
function keepInput(root) {
  const key = field => (field.form?.id || "") + "|" + (field.name || field.id);
  const fields = element => [...element.querySelectorAll("input, textarea, select")].filter(field => !["hidden", "file"].includes(field.type));
  const values = new Map(fields(root).map(field => [key(field), field.type === "checkbox" ? field.checked : field.value]));
  const open = [...root.querySelectorAll("details")].map(details => details.open);
  const focused = root.contains(document.activeElement) && document.activeElement.matches("input, textarea, select") ? key(document.activeElement) : "";
  return () => {
    for (const field of fields(root)) {
      if (!values.has(key(field)) || field.readOnly) continue;
      if (field.type === "checkbox") field.checked = values.get(key(field)); else field.value = values.get(key(field));
      if (field.matches("[data-new-folder]")) field.form.elements.newFolder.hidden = field.value !== "__new__";
    }
    const details = root.querySelectorAll("details");
    if (details.length === open.length) details.forEach((element, i) => { element.open = open[i]; });
    if (focused) fields(root).find(field => key(field) === focused)?.focus();
    // The address check line is redrawn in the new language.
    const where = $("#create-pairing");
    if (where?.address.value.trim()) onWhereInput(where);
  };
}
function setLanguage(next) {
  if (next === lang || !LANGUAGES.includes(next)) return;
  lang = next; T = table(lang); addText = T.add; mastersText = T.masters;
  try { localStorage.setItem(LANGUAGE_KEY, lang); } catch {}
  document.documentElement.lang = lang;
  const restore = keepInput($("#main"));
  closeDcMenu(); clearNotice(); applyStatic(); render(); restore();
}
function renderHeader() {
  $("#fleet-count").textContent = String(hostCount());
  // Unacknowledged alarms: red if any is red, hidden at 0.
  const count = $("#alarm-count"), open = (state.alarms.red || 0) + (state.alarms.yellow || 0);
  if (count) { count.hidden = !open; count.textContent = String(open); count.className = "count alarm-count " + (state.alarms.red ? "is-red" : "is-yellow"); }
  // Always say which Master this window operates from; merged Masters are reached through it.
  $("#service-label").textContent = state.service ? state.service.label : T.side.unavailable;
  if (state.service?.version && $("#app-version")) $("#app-version").textContent = "v" + state.service.version;
  $("#service-dot").className = "dot " + (state.service ? "good" : "bad");
  // Folded by default (only the main menu shows); the chevron by Inventory opens Add host, the filter and the tree, and it is remembered
  $("#side-hosts").hidden = state.dc.treeHidden === true;
  $(".tree-toggle").setAttribute("aria-expanded", String(state.dc.treeHidden !== true));
  document.querySelectorAll("[data-page]").forEach(button => {
    const active = button.dataset.page === state.page;
    button.classList.toggle("active", active);
    if (active) button.setAttribute("aria-current", "page"); else button.removeAttribute("aria-current");
  });
}
async function refreshLocal() {
  try { state.service = await rpc("system.status"); state.serviceError = ""; }
  catch (error) { state.service = null; state.serviceError = String(error); }
  try {
    const links = await native("client_links");
    state.clientLinks = Array.isArray(links) ? links : [];
    state.clientLinksError = "";
  } catch (error) {
    state.clientLinks = [];
    state.clientLinksError = String(error?.message || error);
  }
  renderHeader();
}
async function refreshFleet() {
  const [rows, merges, relays, folders, datacenters, backup, rotation, tags] = await Promise.all([
    rpc("targets.list"), rpc("merges.list").catch(() => []), rpc("relays.list").catch(() => []), rpc("folders.list").catch(() => null), rpc("datacenters.list").catch(() => null),
    rpc("backup.status").catch(() => null), rpc("keys.rotation_status").catch(() => null), rpc("tags.list").catch(() => null)
  ]);
  state.backupStatus = backup; state.rotation = rotation;
  void refreshActivityStatus().then(() => { if ($("#activity-warning")) $("#activity-warning").outerHTML = renderActivityWarning(); });
  // Redraw for new update info only when it changed, and never under someone typing or adding a host.
  const before = JSON.stringify(state.upgrades || null);
  void refreshUpgrades().then(() => {
    if (state.page !== "datacenter" || JSON.stringify(state.upgrades || null) === before) return;
    $("#dc-tree").innerHTML = renderDcTree();
    if ($("#dc-panel") && !state.dc.adding && !document.activeElement?.matches?.("input, textarea, select")) $("#dc-panel").innerHTML = renderDcPanel();
  });
  void refreshAlarms().then(() => { if ($("#dc-tree")) $("#dc-tree").innerHTML = renderDcTree(); if ($("#host-alarms") && pageTarget()) $("#host-alarms").outerHTML = renderHostAlarms(pageTarget()); });
  if (tags) state.tags = tags;
  if (folders) state.folders = folders;
  if (datacenters) state.datacenters = datacenters;
  if (!Array.isArray(rows)) throw new Error(T.common.badFormat);
  state.targets = rows; state.merges = merges; state.relays = relays;
  if (state.selected && !target()) state.selected = null;
  renderHeader();
}
async function refreshMasters() {
  [state.masters, state.relays, state.merges] = await Promise.all([
    rpc("masters.list"), rpc("relays.list").catch(() => []), rpc("merges.list").catch(() => [])
  ]);
}
const HEALTH_INTERVAL = 60000;
async function refreshHealth(refresh = false) {
  if (state.healthLoading) return;
  state.healthLoading = true; state.healthStarted = Date.now();
  try {
    const rows = await rpc("fleet.health", { refresh });
    state.flipped = new Set(rows.filter(row => state.health.has(row.targetId) && state.health.get(row.targetId).online !== row.online).map(row => row.targetId));
    state.health = new Map(rows.map(row => [row.targetId, row]));
    state.healthError = ""; state.healthAt = new Date();
  } catch (error) { state.healthError = String(error?.message || error); }
  finally { state.healthLoading = false; }
  if ($("#fleet-table")) $("#fleet-table").innerHTML = renderFleetView();
  // Re-render only read-only summaries; a machine page may hold an open form.
  $("#dc-tree").innerHTML = renderDcTree();
  // The Masters page lists hosts whose health says they are Masters.
  if (state.page === "masters") $("#main").innerHTML = renderMasters();
  if (state.page === "datacenter" && state.dc.tab === "summary" && state.dc.node !== "dc" && !holdsForm() && !state.dc.adding) $("#dc-panel").innerHTML = renderDcPanel();
  if ($("#inv-summary")) $("#inv-summary").outerHTML = renderSummaryLine();
  syncSky();
}
function refreshHealthIfDue() {
  const showsHealth = state.page === "datacenter";
  if (!showsHealth || document.hidden || !document.hasFocus()) return;
  if (Date.now() - state.healthStarted >= HEALTH_INTERVAL - 1000) void refreshHealth();
}
async function navigate(page) {
  state.page = page; clearNotice(); render();
  if (page === "datacenter") {
    try { await refreshFleet(); if (state.page === page) render(); } catch (error) { notify(error, true); }
    void refreshHealth();
    void loadContainersIfShown(); void loadFilesIfShown();
  }
  if (page === "masters") {
    try { await refreshMasters(); if (state.page === page) render(); } catch (error) { notify(error, true); }
    if (!state.health.size) void refreshHealth();
  }
  if (page === "service") { await refreshLocal(); if (state.page === page) render(); }
  if (page === "alarms") { await Promise.all([refreshAlarms(), refreshAlarmRules()]); if (state.page === page) render(); }
  if (page === "enroll") { try { await refreshPairings(); if (state.page === page) render(); } catch (error) { notifyError(error, true); } }
}
function render() {
  renderHeader();
  const main = $("#main");
  $("#dc-tree").innerHTML = renderDcTree();
  if (state.page === "datacenter") main.innerHTML = '<section class="dc-main" id="dc-panel">' + renderDcPanel() + "</section>";
  if (state.page === "enroll") main.innerHTML = renderAddHost();
  if (state.page === "masters") main.innerHTML = renderMasters();
  if (state.page === "alarms") main.innerHTML = renderAlarms();
  if (state.page === "service") main.innerHTML = renderService();
  if (state.page === "enroll" || state.dc.adding) focusAddStep();
  syncSky();
  if (state.rotation?.active && $("#keys-progress") && !rotationTimer) rotationTimer = setTimeout(() => void watchRotation(), 2000);
  watchFetch();
}
function heading(title, description, actions = "") {
  return '<div class="page-heading"><div><h1>' + esc(title) + "</h1>" + (description ? "<p>" + esc(description) + "</p>" : "") + "</div>" + (actions ? '<div class="page-actions">' + actions + "</div>" : "") + "</div>";
}
function renderFleetTools() {
  return '<div id="fleet-table">' + renderFleetView() + "</div>" +
    '<details class="fleet-hint"><summary>' + T.inv.hintTitle + "</summary><div><ul>" + T.inv.hint.map(line => "<li>" + esc(line) + "</li>").join("") + "</ul></div></details>";
}
function renderFleetView() {
  if (!state.targets.length) return renderFleetRows();
  return renderGraph();
}
const stateLabel = status => T.status.hosts[status];
const stateStyles = { online: "good", offline: "bad", checking: "pending", legacy: "pending" };
function nodeState(item) {
  if (item.runnerInstalled === false) return "legacy";
  const row = state.health.get(item.id);
  if (!row) return state.healthError ? "unknown" : "checking";
  return row.online === true ? "online" : row.online === false ? "offline" : "unknown";
}
const HOT = 85;
/** Every host this window can open: the current Master plus every Target (merged Masters included). */
const hostCount = () => state.targets.length + 1;
const hostsOnline = () => state.targets.filter(item => nodeState(item) === "online").length + (state.service ? 1 : 0);
function hostRole(item) {
  const role = T.inv.role;
  if (!item) return role.current;
  if (state.merges.some(merge => merge.targetId === item.id)) return role.merged;
  return state.masterCheck.get(item.id) || state.health.get(item.id)?.isMaster ? role.master : role.target;
}
/** Every host page — target host or Master — has the same tabs. */
const hostPageTabs = (current = false) => ["summary", ...(current ? ["activity"] : []), "jobs", "files", "containers", "monitoring"].map(id => [id, T.host.tabs[id]]);
/** This Master registered as a target host of itself (needed for its own jobs, files and terminal). */
const selfTarget = () => state.service ? state.targets.find(item => !item.relayed && item.nodeId === state.service.nodeId) : null;
/** The target host behind the open page: the host itself, a merged Master, or this Master's self registration. */
function pageTarget() {
  const node = state.dc.node || "";
  if (node.startsWith("node:")) return state.targets.find(item => item.id === node.slice(5)) || null;
  if (node === "master:local") return selfTarget() || null;
  if (node.startsWith("master:")) return state.targets.find(item => item.id === node.slice(7)) || null;
  return null;
}
function resetHostState(id) {
  state.selected = id || null; state.job = null; state.jobs = []; state.jobsError = "";
  state.files = null; state.filesError = ""; state.file = null; state.pendingJob = null; state.newJob = false;
}
/** Pages whose open form or editor must not be redrawn by background refreshes. */
const holdsForm = () => Boolean(state.backup) || state.dc.node.startsWith("node:") || ["jobs", "files"].includes(state.dc.tab);
function sampledMem(id) {
  const samples = state.monitor.get(id)?.samples;
  return samples?.length > 1 ? monitorSeries(samples.slice(-2))[0]?.memory ?? null : null;
}
const memCell = item => { const mem = sampledMem(item.id); return mem == null ? '<span class="muted">—</span>' : '<span class="num">' + Math.round(mem) + "%</span>"; };
function hostSite(item) {
  if (!item) return siteOf("local")?.name || "—";
  if (state.merges.some(merge => merge.targetId === item.id)) return siteOf(item.id)?.name || "—";
  return siteOf(item.relayed ? item.id.split("/")[0] : "local")?.name || "—";
}
function hostAddress(item) {
  const route = item.routes?.[0];
  return item.relayed ? '<span class="muted">' + esc(T.inv.via(item.viaLabel || item.via)) + "</span>"
    : route ? '<code title="' + esc(route.kind || "") + '">' + esc(route.address) + (route.port && route.port !== 22 ? ":" + esc(route.port) : "") + "</code>" : "—";
}
/** A merged host this Master may only view (its Merge Role, entry 6). */
const viewOnly = item => item?.access === "view";
const accessFrom = item => item?.accessFrom || item?.viaLabel;
const accessBadge = item => viewOnly(item) ? '<span class="badge view-only" title="' + esc(T.share.viewOnlyWhy(accessFrom(item))) + '">' + T.share.viewOnly + "</span>" : "";
const terminalButton = item => item ? '<button class="small" data-action="open-shell" data-id="' + esc(item.id) + '" aria-label="' + esc(T.inv.terminalOn(item.label || item.id)) + '"' + (!item.runnerInstalled ? ' disabled title="' + esc(T.inv.installFirst) + '"' : ' title="' + esc(T.inv.openTerminal) + '"') + ">" + icons.shell + '<span class="btn-label">' + T.inv.openTerminal + "</span></button>" : "";
const hostHead = () => { const h = T.inv.head; return "<thead><tr><th>" + h.host + "</th><th>" + h.role + "</th><th>" + h.address + "</th><th>" + h.cpu + "</th><th>" + h.mem + "</th><th>" + h.status + "</th><th></th></tr></thead>"; };
/** One row per host; only the name links to its page. */
function hostRow(item) {
  if (!item) return '<tr data-host="local"><td><button class="row-button" data-action="dc-select" data-node="master:local">' + esc(state.service?.label || T.inv.thisMaster) + "</button><small>" + T.inv.thisWindow + "</small></td><td>" + T.inv.role.current + "</td>" +
    '<td><span class="muted">—</span></td><td>' + cpuCell({ id: "local" }) + "</td><td>" + memCell({ id: "local" }) + "</td><td>" + localStatus() + '</td><td class="actions">' + terminalButton(selfTarget()) + "</td></tr>";
  const check = state.diagnostics.get(item.id);
  const status = !item.runnerInstalled ? '<span class="badge pending">' + T.status.hosts.legacy + "</span>" : check ? '<span class="badge ' + (check.ok ? "good" : "bad") + '">' + (check.ok ? T.inv.connected : T.inv.problem) + "</span><small>" + esc(T.common.checkedAt(timestamp(check.checkedAt))) + "</small>" : statusBadge(item);
  return '<tr data-host="' + esc(item.id) + '"><td><button class="row-button" data-action="dc-select" data-node="node:' + esc(item.id) + '">' + esc(item.label || item.id) + "</button><small>" + esc(osName(item.os)) + "</small>" + accessBadge(item) + upgradeBadge(item) + tagChips(item.id) + "</td><td>" + esc(hostRole(item)) + "</td><td>" + hostAddress(item) + "</td><td>" + cpuCell(item) + "</td><td>" + memCell(item) + "</td><td>" + status + '</td><td class="actions">' + terminalButton(item) + "</td></tr>";
}
const localStatus = () => '<span class="badge ' + (state.service ? "good" : "bad") + '">' + (state.service ? T.status.hosts.online : T.status.unavailable) + "</span>";
/** Latest CPU % from the Monitoring tab's own samples, or null when the machine was never watched. */
function sampledCpu(id) {
  const samples = state.monitor.get(id)?.samples;
  return samples?.length > 1 ? monitorSeries(samples.slice(-2))[0]?.cpu ?? null : null;
}
const isHot = item => nodeState(item) === "online" && (sampledCpu(item.id) ?? 0) >= HOT;
const cpuCell = item => { const cpu = sampledCpu(item.id); return cpu == null ? '<span class="muted">—</span>' : '<span class="num' + (cpu >= HOT ? " hot-text" : "") + '">' + Math.round(cpu) + "%</span>"; };
const statusBucket = item => ({ online: "online", offline: "offline" })[nodeState(item)] || "other";
function statusBadge(item) {
  const status = nodeState(item), error = state.health.get(item.id)?.error;
  if (isHot(item)) return '<span class="badge hot" title="' + esc(T.inv.highLoadTitle(Math.round(sampledCpu(item.id)))) + '">' + T.inv.highLoad + "</span>";
  return '<span class="badge ' + (stateStyles[status] || "") + '"' + (error ? ' title="' + esc(error) + '"' : "") + ">" + esc(stateLabel(status)) + "</span>";
}
function renderStatusBar() {
  const pool = state.targets.filter(item => matchesFilter(item, true));
  const count = bucket => pool.filter(item => statusBucket(item) === bucket).length;
  // The current Master is listed too (first row) whenever no search or tenant filter hides it.
  const local = !state.query && state.service ? 1 : 0, f = T.inv.filters;
  const filters = [["", f.all, pool.length + local, ""], ["online", f.online, count("online") + local, "good"], ["offline", f.offline, count("offline"), "bad"], ["other", f.other, count("other"), "pending"]]
    .filter(([id, , total]) => id !== "other" || total || state.status === "other");
  const meta = state.healthError
    ? '<span class="status-meta error" title="' + esc(state.healthError) + '">' + icons.alert + T.inv.statusFailedHover + "</span>"
    : '<span class="status-meta">' + (state.healthAt ? esc(T.inv.lastChecked(timestamp(state.healthAt))) : T.inv.checkingAll) + "</span>";
  return '<div class="status-bar"><div class="status-filters" role="group" aria-label="' + esc(T.inv.filterBy) + '">' +
    filters.map(([id, label, total, dot]) => '<button class="status-filter" data-action="status-filter" data-status="' + id + '" aria-pressed="' + (state.status === id) + '">' + (dot ? '<span class="dot ' + dot + '"></span>' : "") + label + " <strong>" + total + "</strong></button>").join("") +
    "</div>" + meta + "</div>";
}
/** The one rule for the filter text: a host matches on its ID, name, tenant, merged Master, addresses, folder name or Tags. */
function hostMatches(item, text) {
  const search = String(text || "").trim().toLowerCase();
  if (!search) return true;
  const folderId = state.folders.members?.[item.id], folder = folderId && (state.folders.folders || []).find(row => row.id === folderId);
  return [item.id, item.label, item.tenant, item.viaLabel, folder?.name, ...hostTags(item.id), ...(item.routes || []).map(route => route.address)]
    .some(value => String(value || "").toLowerCase().includes(search));
}
const hostTags = id => state.tags.hosts?.[id] || [];
/** A host's Tags as chips in a table row or on a topology card (nothing when it has none). */
const tagChips = id => hostTags(id).length ? '<span class="row-tags">' + hostTags(id).map(tag => tagChip(tag)).join("") + "</span>" : "";
/** A Tag as a chip: clicking its name filters by it; `removeFrom` adds × to take it off that host. */
function tagChip(tag, removeFrom = "") {
  return '<span class="tag-chip"><button type="button" class="tag-name" data-action="tag-filter" data-tag="' + esc(tag) + '" title="' + esc(T.tags.filterBy(tag)) + '">' + esc(tag) + "</button>" +
    (removeFrom ? '<button type="button" class="tag-remove" data-action="tag-remove" data-id="' + esc(removeFrom) + '" data-tag="' + esc(tag) + '" aria-label="' + esc(T.tags.remove(tag)) + '">' + icons.close + "</button>" : "") + "</span>";
}
/** The host page's Tags: chips with ×, and a box that suggests the Tags already in use. */
function renderTagRow(item) {
  const t = T.tags, mine = hostTags(item.id), lower = new Set(mine.map(tag => tag.toLowerCase()));
  const suggestions = (state.tags.tags || []).map(row => row.tag).filter(tag => !lower.has(tag.toLowerCase()));
  const error = state.tagError?.id === item.id ? '<p class="add-check bad" role="alert">' + errorHtml(explain(state.tagError.error)) + "</p>" : "";
  return '<section class="tag-row" id="host-tags" aria-label="' + esc(t.title) + '">' + mine.map(tag => tagChip(tag, item.id)).join("") +
    '<form id="tag-add" data-id="' + esc(item.id) + '" autocomplete="off"><input name="tag" list="tag-suggestions" maxlength="64" placeholder="' + esc(t.add) + '" aria-label="' + esc(t.addLabel) + '">' +
    '<datalist id="tag-suggestions">' + suggestions.map(tag => '<option value="' + esc(tag) + '">').join("") + "</datalist></form>" + error + "</section>";
}
async function refreshTags() { state.tags = await rpc("tags.list").catch(() => state.tags); }
async function setHostTags(id, tags) {
  state.tagError = null;
  try { await rpc("tags.set", { targetId: id, tags }); }
  catch (error) { state.tagError = { id, error: String(error?.message || error) }; }
  await refreshTags();
  const restore = keepInput($("#main")); render(); restore();
  if (state.tagError) $("#tag-add [name=tag]")?.focus(); else { const box = $("#tag-add [name=tag]"); if (box) { box.value = ""; box.focus(); } }
}
/** A chip click: that Tag goes in the filter box, and the tree and host lists follow it. */
function filterByTag(tag) {
  state.query = state.dc.query = tag;
  const box = $("#dc-search"); if (box) box.value = tag;
  render();
}
function matchesFilter(item, ignoreStatus = false) {
  return (ignoreStatus || !state.status || statusBucket(item) === state.status) && hostMatches(item, state.query);
}
function isTwoWay(merge) {
  const peer = state.targets.find(item => item.id === merge?.targetId);
  return merge?.reverse?.status === "two-way" && state.relays.some(relay => relay.masterId === peer?.nodeId && !relay.revoked);
}
const noMatch = () => '<div class="empty"><h2>' + T.inv.noMatchTitle + "</h2><p>" + T.inv.noMatchText + '</p><button data-action="clear-filter">' + T.inv.clearFilters + "</button></div>";
/** Under the sky: tabs (All · Direct · Via <Master> per merged Master · Needs a look) over host rows in two columns.
 *  The sky draws the topology; this list is the way to act (open a host, its terminal, its Tags). */
const needsLook = item => isHot(item) || ["offline", "unknown", "legacy"].includes(nodeState(item));
function renderGraph() {
  const masters = dcModel(), local = masters[0], peers = masters.slice(1), peerIds = new Set(peers.map(master => master.id));
  const direct = local.machines.filter(entry => !peerIds.has(entry.item.id)).map(entry => entry.item);
  const t = T.inv.tabs, all = state.targets;
  const tabs = [["all", t.all, all.length + 1], ["direct", t.direct, direct.length],
    ...peers.map(peer => ["peer:" + peer.id, T.inv.through(peer.label), peer.all.length]), ["look", t.look, all.filter(needsLook).length]];
  if (!tabs.some(([id]) => id === state.invTab)) state.invTab = "all";
  const peer = state.invTab.startsWith("peer:") ? peers.find(row => "peer:" + row.id === state.invTab) : null;
  const pool = state.invTab === "direct" ? direct : state.invTab === "look" ? all.filter(needsLook) : peer ? [peer.item, ...peer.all.map(entry => entry.item)] : all;
  const alsoDirect = new Set(peers.flatMap(row => row.all.filter(entry => entry.alsoDirect).map(entry => entry.item.id)));
  const row = item => {
    const status = item ? nodeState(item) : state.service ? "online" : "offline", name = item ? item.label || item.id : local.label;
    const role = !item ? T.inv.cardSelf : peerIds.has(item.id) ? T.inv.peerRole(peers.find(row => row.id === item.id).all.length) : "";
    const sub = [role, item ? osName(item.os) : "", item && !item.relayed ? item.routes?.[0]?.address : "", item?.relayed ? T.inv.via(item.viaLabel || "") : ""].filter(Boolean);
    const shown = item && isHot(item) ? T.inv.highLoad : stateLabel(status) || status;
    return '<div class="inv-row is-' + (item && isHot(item) ? "hot" : status) + (item && state.flipped.has(item.id) ? " just-changed" : "") + '" data-host="' + esc(item ? item.id : "local") + '">' +
      '<span class="dot ' + ({ online: "good", offline: "bad" }[status] || "pending") + (item && isHot(item) ? " hot" : "") + '" aria-hidden="true"></span>' +
      '<div class="inv-text"><button class="row-button" data-action="dc-select" data-node="' + (item ? "node:" + esc(item.id) : "master:local") + '">' + esc(name) + "</button>" +
      "<small>" + esc(sub.join(" · ")) + (item && alsoDirect.has(item.id) && state.invTab.startsWith("peer:") ? " · " + T.inv.alsoDirectLower : "") + "</small>" + (item ? tagChips(item.id) : "") + "</div>" +
      '<span class="inv-state">' + esc(shown) + "</span>" + (item ? terminalButton(item) : '<span class="inv-shell-space"></span>') + "</div>";
  };
  const rows = pool.filter(item => matchesFilter(item));
  const showLocal = state.invTab === "all" && !state.query && (!state.status || state.status === "online");
  const meta = peer ? '<p class="inv-meta">' + esc(T.inv.online(peer.machines.filter(entry => nodeState(entry.item) === "online").length, peer.machines.length)) + " · " +
    (peer.twoWay ? T.inv.twoWay : T.inv.oneWay) + ' <button class="text-button" data-page="masters">' + T.inv.manageMerge + "</button></p>" : "";
  const body = rows.length || showLocal ? '<div class="inv-rows">' + (showLocal ? row(null) : "") + rows.map(row).join("") + "</div>"
    : '<div class="empty"><h2>' + (state.invTab === "look" ? T.inv.tabs.nothingTitle : T.inv.noDrawTitle) + "</h2><p>" + (state.invTab === "look" ? T.inv.tabs.nothingText : T.inv.noDrawText) + "</p></div>";
  return '<div class="inv-tabs" role="tablist" aria-label="' + esc(t.label) + '">' + tabs.map(([id, label, count]) =>
    '<button role="tab" class="inv-tab" data-action="inv-tab" data-tab="' + esc(id) + '" aria-selected="' + (state.invTab === id) + '">' + esc(label) + ' <span class="n">' + count + "</span></button>").join("") + "</div>" + meta + body;
}
function renderFleetRows() {
  const rows = state.targets.filter(item => matchesFilter(item));
  if (!state.targets.length && !state.service) return '<div class="empty"><h2>' + T.inv.firstTitle + "</h2><p>" + T.inv.firstText + "</p></div>";
  const showLocal = !state.query && (!state.status || state.status === "online");
  if (!rows.length && !showLocal) return noMatch();
  return '<div class="table-wrap"><table>' + hostHead() + "<tbody>" + (showLocal ? hostRow(null) : "") + rows.map(hostRow).join("") + "</tbody></table></div>";
}
function stat(label, value, sub = "") {
  return '<div class="stat"><span class="stat-label">' + esc(label) + '</span><div class="stat-value">' + value + "</div>" + (sub ? "<small>" + esc(sub) + "</small>" : "") + "</div>";
}
function healthStat(item) {
  const row = state.health.get(item.id), status = nodeState(item);
  const live = row?.online ? [row.latencyMs != null ? T.sky.ms(row.latencyMs) : "", row.onlineSince ? T.sky.facts.online + " " + formatUptime(Math.max(60, (Date.now() - Date.parse(row.onlineSince)) / 1000)) : ""] : [];
  return stat(T.host.lastStatus, '<span class="dot ' + (stateStyles[status] || "") + '"></span>' + esc(stateLabel(status)),
    [row?.checkedAt ? T.common.checkedAt(timestamp(row.checkedAt)) : "", ...live].filter(Boolean).join(" · "));
}
/** The Summary tab: the facts first, then `middle` (tags, alarms), then the details you open on purpose. */
function renderOverview(item, middle = "") {
  const t = T.host;
  if (item.relayed) {
    return '<div class="stat-grid">' + stat(t.viaMaster, esc(item.viaLabel || item.via)) + stat(t.permissions, t.usesRights) + healthStat(item) + "</div>" + middle +
      '<div class="disclosure-list"><details><summary>' + t.technical + ' <small class="muted">' + t.technicalIds + "</small></summary><div><dl class=\"facts\"><dt>" + t.masterId + "</dt><dd><code>" + esc(item.via) + "</code></dd><dt>" + t.nodeId + "</dt><dd><code>" + esc(item.nodeId) + '</code></dd></dl><p class="help mt">' + esc(t.relayedHelp(item.viaLabel || item.via)) + "</p></div></details></div>";
  }
  const routes = item.routes || [], primary = routes[0], head = t.routeHead;
  return '<div class="stat-grid">' +
    stat(t.primary, primary ? "<code>" + esc(primary.address) + ":" + esc(primary.port || 22) + "</code>" : "—", primary ? (primary.proxyJump ? t.via(primary.proxyJump.address) : t.routeKinds[primary.kind] || primary.kind || "") : "") +
    stat(t.fallback, routes.length > 1 ? String(routes.length - 1) : t.noneRoutes) +
    healthStat(item) +
    stat(t.service, item.runnerInstalled ? '<span class="dot good"></span>' + t.installed : '<span class="dot pending"></span>' + t.notInstalled) +
    "</div>" + middle +
    '<div class="disclosure-list">' +
    "<details><summary>" + t.technical + ' <small class="muted">' + t.technicalMore + '</small></summary><div><dl class="facts"><dt>' + t.nodeId + "</dt><dd><code>" + esc(item.nodeId || t.awaiting) + "</code></dd><dt>" + t.sshUser + "</dt><dd>" + esc(item.user) + "</dd><dt>" + t.bridge + "</dt><dd><code>" + esc(item.bridgePath || "—") + "</code></dd><dt>" + t.targetId + "</dt><dd><code>" + esc(item.id) + "</code></dd></dl>" +
    '<div class="table-wrap mt"><table><thead><tr><th>' + head.order + "</th><th>" + head.type + "</th><th>" + head.destination + "</th><th>" + head.jump + "</th></tr></thead><tbody>" +
    routes.map((route, i) => '<tr><td class="num">' + (i + 1) + "</td><td>" + esc(t.routeKinds[route.kind] || route.kind) + "</td><td><code>" + esc(route.address) + ":" + esc(route.port || 22) + "</code></td><td>" + (route.proxyJump?.address ? "<code>" + esc(route.proxyJump.address) + "</code>" : '<span class="muted">' + t.direct + "</span>") + "</td></tr>").join("") +
    '</tbody></table></div><h3 class="mt">' + t.pinnedKeys + '</h3><div class="output light mt"><pre>' + esc((item.hostKeys || []).join("\n") || "—") + "</pre></div></div></details>" +
    "<details><summary>" + t.edit + '</summary><form id="edit-target"><div class="form-grid"><label class="full">' + t.hostName + '<input name="label" required value="' + esc(item.label || item.id) + '"></label><label class="full">' + t.routesJson + '<textarea class="mono" name="routes" rows="7" required spellcheck="false">' + esc(pretty(routes)) + "</textarea><small>" + t.routesHelp + '</small></label></div><div class="form-actions"><button type="submit" class="primary">' + t.save + '</button></div><p class="help">' + t.tenantHelp + "</p></form></details>" +
    "<details><summary>" + t.removeTitle + '</summary><div><p class="help">' + t.removeHelp + '</p><button class="danger" data-action="remove-target">' + esc(t.remove(item.label || item.id)) + "</button></div></details>" +
    "</div>";
}
/* Connection check result, shown right under the host heading on every tab: one clear verdict, then each step. */
function renderCheckPanel(item) {
  const check = state.diagnostics.get(item.id), running = state.checking === item.id, t = T.check;
  if (!running && (!check || state.checkHidden?.has(item.id))) return "";
  const close = '<button class="icon-button" data-action="check-close" aria-label="' + esc(t.hide) + '" title="' + esc(t.hideShort) + '">' + icons.close + "</button>";
  if (running) return '<section class="check-panel is-running" role="status" aria-live="polite"><span class="check-mark" aria-hidden="true">…</span><div class="check-head"><h2>' + t.running + "</h2>" +
    "<p>" + t.runningText + "</p></div></section>";
  const stages = check.stages || [], passed = stages.filter(stage => stage.ok).length;
  const raw = stage => stage.message || (stage.executor ? (stage.executor.reason || t.notReady) : "");
  const where = stage => stage.route?.address ? stage.route.address + ":" + (stage.route.port || 22) : t.theHost;
  // Plain words for the failures people meet most; the original text stays underneath.
  const plain = stage => {
    const text = raw(stage);
    if (text === "TARGET_IDENTITY_MISMATCH" || /host key|REMOTE HOST IDENTIFICATION/i.test(text)) return t.identity;
    if (/timed out|timeout/i.test(text)) return t.timeout(where(stage));
    if (/refused/i.test(text)) return t.refused(where(stage));
    if (/could not resolve|Name or service not known|nodename nor servname/i.test(text)) return t.resolve(stage.route?.address || t.ofTheHost);
    if (/no route|unreachable/i.test(text)) return t.noRoute(where(stage));
    if (/permission denied|publickey/i.test(text)) return t.denied;
    if (/E_OWNER_NOT_LOGGED_IN/.test(text)) return explain(text).text;
    return "";
  };
  const why = stage => { const text = plain(stage), detail = raw(stage); return text ? esc(text) + (detail && detail !== text ? '<small class="check-detail">' + esc(detail) + "</small>" : "") : esc(detail); };
  const label = stage => (t.steps[stage.name] || stage.name) + (stage.route?.address ? " (" + stage.route.address + ")" : "");
  const selected = check.result?.executor?.selected || stages.find(stage => stage.name === "executor")?.executor?.selected;
  const steps = stages.map(stage => '<li class="' + (stage.ok ? "is-ok" : "is-bad") + '"><span class="step-mark" aria-hidden="true">' + (stage.ok ? "✓" : "✕") + "</span><div><strong>" + esc(label(stage)) + "</strong>" +
    (stage.ok ? "" : "<p>" + why(stage) + "</p>") + "</div></li>").join("");
  return '<section class="check-panel ' + (check.ok ? "is-ok" : "is-bad") + '" role="status" aria-live="polite"><span class="check-mark" aria-hidden="true">' + (check.ok ? "✓" : "!") + "</span>" +
    '<div class="check-head"><h2>' + (check.ok ? t.ok : t.bad) + "</h2><p>" + esc(t.passed(passed, stages.length)) + " · " + esc(timestamp(check.checkedAt)) +
    (selected?.ownerSessionId ? " · " + esc(t.session(selected.ownerSessionId)) : "") + '</p></div><div class="check-actions"><button class="small" data-action="diagnose">' + icons.refresh + t.again + "</button>" + close + "</div>" +
    '<ol class="check-steps">' + steps + "</ol>" +
    '<details class="check-raw"><summary>' + t.raw + '</summary><div><button class="small" data-action="copy-diagnostics">' + icons.copy + t.copyJson + "</button></div></details></section>";
}
function renderJobs(item) {
  const t = T.jobs, disabled = !item.runnerInstalled ? ' disabled title="' + esc(T.inv.installFirst) + '"' : "";
  return (state.groupRun ? renderGroupRun(state.groupRun) : state.newJob ? renderNewJob(item) : "") +
    '<section class="section"><div class="section-heading"><h2>' + t.title + '</h2><div class="page-actions">' +
    (state.newJob ? "" : '<button class="primary" data-action="toggle-new-job"' + disabled + ">" + icons.plus + t.newJob + "</button>") +
    '<button data-action="refresh-jobs">' + icons.refresh + T.common.refresh + '</button></div></div><div class="job-layout"><div class="job-list" id="job-list">' + renderJobRows() + '</div><div class="job-detail" id="job-detail">' + renderJobDetail() + "</div></div></section>";
}
/* Group runs (roadmap entry 4): "Run on" picks this host, every host with a Tag, or every host in a folder. A
   confirmation names the hosts; the run then shows in place of the form, one row per host, updating until done. */
function masterLabel(id) {
  if (id === "local") return state.service?.label || T.inv.thisMaster;
  return state.merges.find(merge => merge.targetId === id)?.label || id;
}
function runOnOptions() {
  const t = T.jobs, folders = state.folders.folders || [], several = new Set(folders.map(folder => folder.master)).size > 1;
  return '<option value="">' + esc(t.thisHost) + "</option>" +
    (state.tags.tags || []).map(row => '<option value="tag:' + esc(row.tag) + '">' + esc(t.withTag(row.tag)) + "</option>").join("") +
    folders.map(folder => '<option value="folder:' + esc(folder.id) + '">' + esc(t.inFolder((several || folder.master !== "local" ? masterLabel(folder.master) + " / " : "") + folder.name)) + "</option>").join("");
}
/** Owner session and Shell are per host, so they are hidden for a group (each OS uses its own shell). A group with Linux and Windows hosts gets a script box
 *  for each (and each OS's default working folder). */
async function onRunOnChange(form) {
  const value = form.elements.runOn.value;
  for (const field of form.querySelectorAll(".host-only")) field.hidden = Boolean(value);
  const preview = value ? await rpc("groups.preview", { selector: runOnSelector(form.elements.runOn) }).catch(() => null) : null;
  if (form.elements.runOn.value !== value) return; // changed again while asking
  setMixed(form, new Set((preview?.hosts || []).map(row => row.os)).size > 1);
}
function setMixed(form, mixed) {
  form.dataset.mixed = mixed ? "1" : "";
  for (const field of form.querySelectorAll('[data-script="pair"]')) field.hidden = !mixed;
  form.querySelector('[data-script="single"]').hidden = mixed;
  form.querySelector("[data-cwd]").hidden = mixed;
  form.elements.script.required = !mixed; form.elements.cwd.required = !mixed;
  form.elements.scriptLinux.required = form.elements.scriptWindows.required = mixed;
}
/** The group's selector from the Run on value, with the words shown for it. */
function runOnSelector(select) {
  const [kind, ...rest] = select.value.split(":"), value = rest.join(":"), label = select.selectedOptions[0]?.textContent || "";
  return kind === "tag" ? { tag: value, label } : { folder: value, label };
}
async function draftGroupRun(form, values, env) {
  const selector = runOnSelector(form.elements.runOn), host = target();
  const preview = await rpc("groups.preview", { selector });
  const oses = [...new Set(preview.hosts.map(row => row.os))], mixed = oses.length > 1;
  const scripts = {};
  if (mixed) Object.assign(scripts, { linux: values.scriptLinux, windows: values.scriptWindows });
  else for (const os of oses.length ? oses : [host?.os || "linux"]) scripts[os] = values.script;
  const params = { selector, scripts, cwd: mixed ? { linux: defaultCwd("linux"), windows: defaultCwd("windows") } : values.cwd, env };
  if (values.asAdmin === "on") params.asAdmin = true;
  state.groupDraft = { params, preview };
  fillGroupDialog(); $("#group-dialog").showModal();
}
/** The confirmation's words; also re-run when the language changes while it is open. */
function fillGroupDialog() {
  const draft = state.groupDraft, t = T.jobs;
  if (!draft) return;
  $("#group-title").textContent = t.confirmTitle(draft.preview.hosts.length);
  $("#group-lead").textContent = t.confirmLead(draft.params.selector.label);
  $("#group-hosts").innerHTML = draft.preview.hosts.map(row => "<li>" + esc(row.label || row.targetId) + ' <small class="muted">' + esc(osName(row.os)) + "</small></li>").join("");
  $("#group-skipped").innerHTML = draft.preview.skipped.length ? '<p class="help mt">' + esc(t.skippedTitle) + '</p><ul class="dialog-list">' + draft.preview.skipped.map(row => "<li>" + esc(row.label || row.targetId) + "</li>").join("") + "</ul>" : "";
  $('#confirm-group [type="submit"]').disabled = !draft.preview.hosts.length;
}
async function runGroup() {
  const draft = state.groupDraft;
  $("#group-dialog").close();
  if (!draft) return;
  state.groupDraft = null;
  state.groupRun = await rpc("groups.run", draft.params);
  state.newJob = false; render(); void watchGroup();
}
const groupFinished = row => ["succeeded", "failed", "cancelled", "interrupted"].includes(row.status) || ["unreachable", "skipped"].includes(row.state);
function groupRowStatus(row) {
  const t = T.jobs;
  if (row.state === "started") return badge(row.status || "queued");
  return '<span class="badge' + (row.state === "unreachable" ? " bad" : "") + '">' + esc(t.groupStates[row.state] || row.state) + "</span>";
}
function renderGroupRun(run) {
  const t = T.jobs, hosts = run.hosts || [], counted = hosts.filter(row => row.state !== "skipped");
  const ok = counted.filter(row => row.status === "succeeded").length, finished = counted.filter(groupFinished).length;
  const rows = hosts.map(row => '<tr data-host="' + esc(row.targetId) + '"><td><strong>' + esc(row.label || row.targetId) + "</strong><small>" + esc(osName(row.os)) + "</small>" +
    (row.error && row.state !== "skipped" ? '<small class="error-line">' + errorHtml(explain(row.error, false, row.label || row.targetId)) + "</small>" : "") + "</td><td>" + groupRowStatus(row) + "</td>" +
    '<td class="num">' + (row.exitCode ?? "—") + '</td><td class="actions">' +
    (row.state === "started" && !groupFinished(row) ? '<button class="small" data-action="group-cancel" data-id="' + esc(row.targetId) + '">' + t.cancelOne + "</button>" : "") +
    (row.jobId ? '<button class="small" data-action="group-log" data-id="' + esc(row.targetId) + '" data-job="' + esc(row.jobId) + '">' + t.openLog + "</button>" : "") + "</td></tr>").join("");
  const failed = counted.some(row => row.state === "unreachable" || ["failed", "interrupted", "cancelled"].includes(row.status));
  const actions = (!run.done ? '<button data-action="group-cancel">' + t.cancelAll + "</button>" : "") +
    (run.done && failed ? '<button class="primary" data-action="group-retry">' + t.retryFailed + "</button>" : "") + '<button data-action="group-close">' + t.closeGroup + "</button>";
  return '<section class="surface group-run" id="group-run" aria-live="polite"><div class="section-heading"><div><h2>' + t.groupTitle + "</h2><p>" + esc(t.groupLead(run.selector?.label || "", timestamp(run.createdAt))) + "</p></div>" +
    '<div class="page-actions">' + actions + "</div></div>" +
    '<p class="group-summary"><strong>' + esc(run.done ? t.groupSummary(ok, counted.length) : t.groupRunning(finished, counted.length)) + "</strong></p>" +
    '<div class="table-wrap"><table><thead><tr><th>' + T.inv.head.host + "</th><th>" + T.inv.head.status + "</th><th>" + t.exitCode + "</th><th></th></tr></thead><tbody>" + rows + "</tbody></table></div></section>";
}
/* Target update pack (roadmap entry 8): which hosts run an older version than this Master, updating one, and
   updating every outdated host one at a time with a progress panel. */
async function refreshUpgrades() {
  const reply = await rpc("targets.update_check").catch(() => null);
  if (reply) state.upgrades = { latest: reply.latest, hosts: Object.fromEntries(reply.hosts.map(row => [row.targetId, row])) };
}
const upgradeRow = id => state.upgrades?.hosts?.[id];
function upgradeBadge(item, short = false) {
  const row = upgradeRow(item.id), t = T.upgrade;
  if (!row?.outdated) return "";
  const by = String(row.reason || "").startsWith("by:") ? row.reason.slice(3) : "";
  const text = short ? t.needsShort : t.needs(row.version, state.upgrades.latest) + (by ? " · " + t.by(by) : "");
  return short ? '<span class="dc-tag is-update" title="' + esc(t.needs(row.version, state.upgrades.latest)) + '">' + esc(text) + "</span>" : '<span class="badge warn upgrade-badge">' + esc(text) + "</span>";
}
/** The host page's update controls: Update and Save the installer, or why not. */
function upgradeControls(item) {
  const row = upgradeRow(item.id), t = T.upgrade;
  if (!row?.outdated) return "";
  if (String(row.reason || "").startsWith("by:")) return '<small class="muted">' + esc(t.byOther(row.reason.slice(3))) + "</small>";
  const busy = state.upgrading === item.id, blocked = row.reason === "E_UPGRADE_TEMPLATES";
  return '<button class="small" data-action="host-upgrade" data-id="' + esc(item.id) + '"' + (busy || blocked ? " disabled" : "") + (blocked ? ' title="' + esc(t.noTemplates) + '"' : "") + ">" + (busy ? t.updating : t.update) + "</button>" +
    (blocked ? "" : '<button class="small" data-action="host-upgrade-file" data-id="' + esc(item.id) + '" data-os="' + esc(row.os) + '">' + t.saveFile + "</button>");
}
async function upgradeHost(id) {
  const item = state.targets.find(row => row.id === id), name = item?.label || id;
  state.upgrading = id; render();
  try {
    const done = await rpc("targets.upgrade", { targetId: id });
    notify(T.upgrade.done(name, done.version));
  } catch (error) {
    const shown = explain(error); notify(T.upgrade.failed(name), true, shown.text);
  } finally {
    state.upgrading = null; await refreshHealth(true).catch(() => {}); await refreshUpgrades(); render();
  }
}
/** Inventory: hosts this Master can update, and the run in progress. */
function renderUpgradePanel() {
  const t = T.upgrade, rows = Object.values(state.upgrades?.hosts || {}), can = rows.filter(row => row.canUpdate), run = state.upgradeRun;
  if (!can.length && !run) return "";
  const progress = run ? '<div class="table-wrap"><table class="upgrade-table"><tbody>' + run.hosts.map(row => '<tr data-upgrade="' + esc(row.targetId) + '"><td>' + esc(state.targets.find(h => h.id === row.targetId)?.label || row.targetId) + '</td><td><span class="badge ' + (row.state === "done" ? "good" : row.state === "failed" ? "bad" : "pending") + '">' + esc(t.state[row.state] || row.state) + "</span>" +
    (row.error ? '<small class="error-detail">' + esc(row.error) + "</small>" : "") + "</td></tr>").join("") + "</tbody></table></div>" +
    (run.done ? '<p class="help">' + esc(t.summary(run.hosts.filter(r => r.state === "done").length, run.hosts.length)) + "</p>" : "") : "";
  return '<section class="section upgrade-panel" id="upgrade-panel"><div class="section-heading"><div><h2>' + t.panelTitle + "</h2><p>" + esc(t.panelLead(can.length || run.hosts.length, state.upgrades?.latest || "")) + "</p></div>" +
    (run && !run.done ? "" : can.length ? '<button class="primary" data-action="upgrade-all">' + t.all + "</button>" : "") + '</div><p class="help">' + t.allLead + "</p>" + progress + "</section>";
}
let upgradeTimer = 0;
async function watchUpgrades() {
  clearTimeout(upgradeTimer);
  state.upgradeRun = await rpc("targets.upgrade_status").catch(() => state.upgradeRun);
  if ($("#upgrade-panel")) $("#upgrade-panel").outerHTML = renderUpgradePanel();
  if (state.upgradeRun && !state.upgradeRun.done) upgradeTimer = setTimeout(() => void watchUpgrades(), 2000);
  else { await refreshHealth(true).catch(() => {}); await refreshUpgrades(); if ($("#upgrade-panel")) $("#upgrade-panel").outerHTML = renderUpgradePanel(); }
}
/* Health alarms (roadmap entry 7). The service checks rules in the background; this page shows what it found, edits
   rules, and polls every 60 s while the window is open (a new red alarm shows the notice bar). */
const alarmLevel = id => { const open = state.alarms.alarms.filter(a => a.host === id && !a.acknowledged); return open.some(a => a.level === "red") ? "red" : open.length ? "yellow" : ""; };
async function refreshAlarms() {
  const [list, updates] = await Promise.all([rpc("alarms.list").catch(() => null), rpc("alarms.updates").catch(() => null)]);
  if (list) {
    const red = list.alarms.filter(a => a.level === "red" && !a.acknowledged);
    if (state.seenRed) {
      const fresh = red.find(a => !state.seenRed.has(a.id));
      if (fresh) notify(T.alarms.newRed(fresh.label || fresh.host, alarmWhat(fresh)), true);
    }
    state.seenRed = new Set(red.map(a => a.id));
    state.alarms = list;
  }
  if (updates) state.updates = updates;
  renderHeader();
}
async function refreshAlarmRules() {
  const reply = await rpc("alarms.rules").catch(() => null);
  if (reply) { state.alarmRules = reply.rules; state.alarmInterval = reply.interval; }
}
const alarmWhat = alarm => T.alarms.shown[alarm.condition]?.(alarm.value, alarm.partition) || alarm.condition;
function scopeLine(scope) {
  const t = T.alarms.scopeLine;
  if (scope.all) return t.all();
  if (scope.tag) return t.tag(scope.tag);
  if (scope.folder) return t.folder((state.folders.folders || []).find(f => f.id === scope.folder)?.name || scope.folder);
  return t.hosts((scope.hosts || []).length) + " · " + (scope.hosts || []).map(id => state.targets.find(h => h.id === id)?.label || id).join(", ");
}
const levelBadge = level => '<span class="alarm-level is-' + (level === "red" ? "red" : "yellow") + '"><span class="dot alarm-' + (level === "red" ? "red" : "yellow") + '" aria-hidden="true"></span>' + esc(T.alarms.level[level]) + "</span>";
function renderAlarms() {
  const t = T.alarms;
  const tabs = '<div class="tabs" role="tablist">' + ["active", "rules"].map(tab => '<button role="tab" class="tab ' + (state.alarmTab === tab ? "active" : "") + '" aria-selected="' + (state.alarmTab === tab) + '" data-action="alarm-tab" data-tab="' + tab + '">' + t[tab] + "</button>").join("") + "</div>";
  return heading(t.title, t.lead) + tabs + (state.alarmTab === "rules" ? renderAlarmRules() : renderActiveAlarms());
}
function renderActiveAlarms() {
  const t = T.alarms, rows = state.alarms.alarms;
  if (!rows.length) return '<div class="empty-inline" id="alarm-list">' + (state.alarmRules && !state.alarmRules.length ? t.noRules : t.none) + "</div>";
  return '<div class="table-wrap" id="alarm-list"><table class="alarm-table"><thead><tr><th>' + t.head.level + "</th><th>" + t.head.host + "</th><th>" + t.head.what + "</th><th>" + t.head.since + "</th><th></th></tr></thead><tbody>" +
    rows.map(a => '<tr class="' + (a.acknowledged ? "acknowledged" : "") + '" data-alarm="' + esc(a.id) + '"><td>' + levelBadge(a.level) + '</td><td><button class="row-button" data-action="dc-select" data-node="node:' + esc(a.host) + '">' + esc(a.label || a.host) + "</button></td><td>" + esc(alarmWhat(a)) +
      "</td><td>" + esc(a.since ? thaiTime(a.since) : "—") + '</td><td class="actions">' + (a.acknowledged ? '<span class="muted">' + t.acknowledged + "</span>" : '<button class="small" data-action="alarm-ack" data-id="' + esc(a.id) + '">' + t.acknowledge + "</button>") + "</td></tr>").join("") +
    "</tbody></table></div>";
}
function renderAlarmRules() {
  const t = T.alarms, rules = state.alarmRules || [];
  const list = rules.length ? '<div class="table-wrap"><table class="alarm-table"><thead><tr><th>' + t.head.rule + "</th><th>" + t.head.hosts + "</th><th>" + t.head.level + "</th><th></th></tr></thead><tbody>" +
    rules.map(r => '<tr data-rule="' + esc(r.id) + '"><td>' + esc(t.condition[r.condition] + (r.threshold != null ? " " + r.threshold + "%" : "") + (r.partition ? " · " + r.partition : "") + ((r.containers || []).length ? " · " + r.containers.join(", ") : "")) + "</td><td>" + esc(scopeLine(r.scope)) + "</td><td>" + levelBadge(r.level) +
      '</td><td class="actions"><button class="small" data-action="alarm-rule-edit" data-id="' + esc(r.id) + '">' + t.edit + '</button><button class="small danger" data-action="alarm-rule-delete" data-id="' + esc(r.id) + '">' + t.remove + "</button></td></tr>").join("") + "</tbody></table></div>"
    : '<div class="empty-inline">' + t.noRules + "</div>";
  const interval = '<form id="alarm-interval" class="inline-form mt"><label>' + t.interval + '<select name="interval">' + [5, 15, 30].map(n => '<option value="' + n + '"' + (state.alarmInterval === n ? " selected" : "") + ">" + t.minutes(n) + "</option>").join("") + '</select></label></form><p class="help">' + t.intervalHelp + "</p>";
  return '<section class="section">' + (state.alarmEdit ? renderRuleEditor(state.alarmEdit) : '<div class="form-actions"><button class="primary" data-action="alarm-rule-new">' + t.addRule + "</button></div>") + list + interval + "</section>";
}
function renderRuleEditor(rule) {
  const t = T.alarms, kind = rule.scope.all ? "all" : rule.scope.folder ? "folder" : rule.scope.tag != null ? "tag" : "hosts";
  const folders = (state.folders.folders || []).map(f => '<option value="' + esc(f.id) + '"' + (rule.scope.folder === f.id ? " selected" : "") + ">" + esc(f.name) + "</option>").join("");
  const chosen = new Set(rule.scope.hosts || []);
  const hosts = state.targets.filter(h => h.runnerInstalled !== false).map(h => '<label class="check-label"><input type="checkbox" name="host" value="' + esc(h.id) + '"' + (chosen.has(h.id) ? " checked" : "") + ">" + esc(h.label || h.id) + "</label>").join("");
  const pct = ["disk", "memory"].includes(rule.condition);
  return '<form id="alarm-rule" class="surface rule-editor" data-id="' + esc(rule.id || "") + '"><h2>' + (rule.id ? t.editRule : t.newRule) + '</h2><div class="form-grid">' +
    '<label>' + t.which + '<select name="scope">' + ["all", "folder", "tag", "hosts"].map(k => '<option value="' + k + '"' + (k === kind ? " selected" : "") + ">" + t.scope[k] + "</option>").join("") + "</select></label>" +
    (kind === "folder" ? '<label>' + t.folder + '<select name="folder">' + folders + "</select></label>" : "") +
    (kind === "tag" ? '<label>' + t.tag + '<input name="tag" list="alarm-tags" value="' + esc(rule.scope.tag || "") + '"><datalist id="alarm-tags">' + (state.tags.tags || []).map(r => '<option value="' + esc(r.tag) + '">').join("") + "</datalist></label>" : "") +
    (kind === "hosts" ? '<fieldset class="full host-pick"><legend>' + t.chooseHosts + "</legend>" + hosts + "</fieldset>" : "") +
    '<label>' + t.conditionLabel + '<select name="condition">' + ["disk", "memory", "unreachable", "container", "updates"].map(c => '<option value="' + c + '"' + (c === rule.condition ? " selected" : "") + ">" + t.condition[c] + "</option>").join("") + "</select></label>" +
    (pct ? '<label>' + t.threshold + '<input name="threshold" type="number" min="1" max="99" value="' + esc(rule.threshold ?? 90) + '"></label>' : "") +
    (rule.condition === "disk" ? '<label>' + t.partition + '<input name="partition" class="mono" value="' + esc(rule.partition || "") + '" placeholder="/var"><small class="help">' + t.partitionHelp + "</small></label>" : "") +
    (rule.condition === "container" ? '<label class="full">' + t.containers + '<input name="containers" value="' + esc((rule.containers || []).join(", ")) + '" placeholder="web, db"><small class="help">' + t.containersHelp + "</small></label>" : "") +
    '<label>' + t.levelLabel + '<select name="level">' + ["red", "yellow"].map(l => '<option value="' + l + '"' + (l === rule.level ? " selected" : "") + ">" + t.level[l] + "</option>").join("") + "</select></label></div>" +
    '<div class="form-actions"><button type="submit" class="primary">' + t.save + '</button><button type="button" data-action="alarm-rule-cancel">' + t.cancel + "</button></div></form>";
}
/** The editor's current values as a rule (also used to redraw it when the scope or condition changes). */
function ruleFromForm(form) {
  const v = name => form.elements[name]?.value, scope = v("scope");
  return {
    id: form.dataset.id || undefined,
    scope: scope === "all" ? { all: true } : scope === "folder" ? { folder: v("folder") || (state.folders.folders || [])[0]?.id || "" } : scope === "tag" ? { tag: v("tag") ?? "" } : { hosts: [...form.querySelectorAll('[name="host"]:checked')].map(el => el.value) },
    condition: v("condition"), threshold: v("threshold") != null ? Number(v("threshold")) : 90, level: v("level"),
    containers: (v("containers") || "").split(",").map(n => n.trim()).filter(Boolean),
    ...(v("partition")?.trim() ? { partition: v("partition").trim() } : {}),
  };
}
/** The host page's Alarms row: its alarms, its OS-update result with Check now, and a rule for just this host. */
function renderHostAlarms(item) {
  const t = T.alarms, mine = state.alarms.alarms.filter(a => a.host === item.id), up = state.updates[item.id];
  const updates = !up || (up.count == null && !up.error) ? t.updatesNever : up.error === "view-only" ? t.updatesViewOnly : up.error ? t.updatesError : up.count ? t.updatesCount(up.count) : t.updatesNone;
  const packages = up?.packages?.length ? "<details><summary>" + t.packages + '</summary><ul class="package-list">' + up.packages.map(p => "<li><code>" + esc(p) + "</code></li>").join("") + "</ul></details>" : "";
  return '<section class="section host-alarms" id="host-alarms"><div class="section-heading"><h2>' + t.hostAlarms + '</h2><button class="small" data-action="alarm-for-host" data-id="' + esc(item.id) + '">' + t.forHost + "</button></div>" +
    (mine.length ? '<ul class="alarm-lines">' + mine.map(a => "<li>" + levelBadge(a.level) + " " + esc(alarmWhat(a)) + (a.acknowledged ? ' <span class="muted">· ' + t.acknowledged + "</span>" : "") + "</li>").join("") + "</ul>" : "") +
    '<p class="updates-line"><strong>' + t.updates + '</strong> <span class="badge ' + (up?.count ? "warn" : "") + '" id="updates-badge">' + esc(state.checkingUpdates === item.id ? t.checking : updates) + "</span>" +
    (up?.checkedAt ? ' <small class="muted">' + esc(t.checkedAt(thaiTime(up.checkedAt))) + "</small>" : "") +
    (up?.error === "view-only" ? "" : ' <button class="small" data-action="check-updates" data-id="' + esc(item.id) + '"' + (state.checkingUpdates === item.id ? " disabled" : "") + ">" + t.checkNow + "</button>") + "</p>" + packages + "</section>";
}
async function checkUpdates(id) {
  state.checkingUpdates = id; render();
  await rpc("alarms.check_updates", { targetId: id });
  for (let i = 0; i < 90; i++) {
    await new Promise(resolve => setTimeout(resolve, 1000));
    const info = await rpc("alarms.host", { targetId: id }).catch(() => null);
    if (info?.updates?.checkNow === false && info.updates.checkedAt && info.updates.checkedAt !== state.updates[id]?.checkedAt) { state.updates[id] = info.updates; break; }
  }
  state.checkingUpdates = null; await refreshAlarms(); render();
}
setInterval(() => { if (!document.hidden) void refreshAlarms().then(() => { if (state.page === "alarms" && state.alarmTab === "active" && $("#alarm-list")) $("#alarm-list").outerHTML = renderActiveAlarms(); $("#dc-tree").innerHTML = renderDcTree(); }); }, 60000);
/* Activity (roadmap entry 5): the current Master's Recent Jobs and audit log. The Master filters by time range, hosts
   and status; this page turns the search box into host IDs with the same `hostMatches` as the tree. Times are always
   Thailand time, whatever the machine's time zone. */
const thaiTime = value => {
  const date = new Date(value || "");
  return Number.isNaN(date.getTime()) ? "—" : date.toLocaleString(T.locale, { timeZone: "Asia/Bangkok", day: "numeric", month: "short", hour: "2-digit", minute: "2-digit", hourCycle: "h23" });
};
/** Today in Thailand as YYYY-MM-DD, for file names and the custom range. */
const thaiDay = () => new Date(Date.now() + 7 * 3600000).toISOString().slice(0, 10);
const activityState = () => state.activity || (state.activity = { view: "jobs", search: "", range: "today", from: "", to: "", status: "all", jobs: null, lines: null, info: null, open: new Set(), detail: null });
function activityParams(view = activityState().view) {
  const a = activityState(), params = a.range === "custom" ? { from: a.from, to: a.to || thaiDay() } : { range: a.range };
  if (view === "jobs" && a.status !== "all") params.status = a.status;
  if (a.search.trim()) { params.hosts = state.targets.filter(item => hostMatches(item, a.search)).map(item => item.id); params.text = a.search.trim(); }
  return params;
}
async function loadActivity(refresh = false) {
  const a = activityState(), view = a.view;
  const reply = view === "audit" ? await rpc("activity.audit", activityParams()) : await rpc("activity.jobs", { ...activityParams(), refresh });
  if (a.view !== view) return;
  if (view === "audit") a.lines = reply.lines || []; else a.jobs = reply.jobs || [];
  a.info = reply.status || a.info;
  redrawActivity();
}
function redrawActivity() {
  if ($("#activity-list")) $("#activity-list").innerHTML = renderActivityList();
  if ($("#activity-warning")) $("#activity-warning").outerHTML = renderActivityWarning();
}
const renderActivityWarning = () => activityState().info?.warning
  ? '<div class="callout pending" id="activity-warning" role="status">' + icons.alert + "<div><strong>" + T.activity.warningTitle + "</strong><p>" + T.activity.warningText + "</p></div></div>"
  : '<div id="activity-warning" hidden></div>';
const activityHost = (id, label) => esc([...state.targets].find(item => item.id === id)?.label || label || id);
const activityBadge = status => status === "unreachable" ? '<span class="badge bad">' + T.activity.unreachable + "</span>" : badge(status);
function selectorLabel(selector) {
  const t = T.activity;
  if (selector?.tag) return t.byTag(selector.tag);
  const folder = (state.folders.folders || []).find(row => row.id === selector?.folder);
  return t.byFolder(folder?.name || selector?.folder || "");
}
function renderActivity() {
  const a = activityState(), t = T.activity;
  const views = ["jobs", "audit"].map(view => '<button data-action="activity-view" data-view="' + view + '" aria-pressed="' + (a.view === view) + '">' + (view === "jobs" ? t.jobsView : t.auditView) + "</button>").join("");
  const ranges = ["today", "yesterday", "7d", "custom"].map(range => '<button data-action="activity-range" data-range="' + range + '" aria-pressed="' + (a.range === range) + '">' + t.ranges[range] + "</button>").join("");
  const custom = a.range === "custom" ? '<label>' + t.from + '<input type="date" id="activity-from" value="' + esc(a.from) + '" max="' + thaiDay() + '"></label><label>' + t.to + '<input type="date" id="activity-to" value="' + esc(a.to || thaiDay()) + '" max="' + thaiDay() + '"></label>' : "";
  const status = a.view === "jobs" ? '<label>' + t.status + '<select id="activity-status">' + ["all", "succeeded", "failed", "running"].map(value => '<option value="' + value + '"' + (a.status === value ? " selected" : "") + ">" + t.statuses[value] + "</option>").join("") + "</select></label>" : "";
  return '<section class="section activity" id="activity">' + renderActivityWarning() +
    '<div class="activity-bar"><div class="segmented" role="group" aria-label="' + esc(t.views) + '">' + views + "</div>" +
    '<input type="search" id="activity-search" value="' + esc(a.search) + '" placeholder="' + esc(a.view === "audit" ? t.searchAudit : t.search) + '" aria-label="' + esc(a.view === "audit" ? t.searchAudit : t.search) + '">' +
    '<div class="segmented" role="group" aria-label="' + esc(t.range) + '">' + ranges + "</div>" + custom + status +
    '<button class="right" data-action="activity-export">' + t.export + "</button></div>" +
    '<div id="activity-list">' + renderActivityList() + "</div></section>";
}
function renderActivityList() {
  const a = activityState(), t = T.activity, rows = a.view === "audit" ? a.lines : a.jobs;
  if (!rows) return '<div class="empty-inline">' + T.activity.loading + "</div>";
  const since = a.info?.since ? '<p class="help activity-since">' + esc(t.since(thaiTime(a.info.since))) + "</p>" : "";
  if (!rows.length) return '<div class="empty-inline">' + t.nothing + "<br><small>" + t.nothingHint + "</small></div>" + since;
  return (a.detail ? renderActivityDetail(a.detail) : "") + (a.view === "audit" ? renderAuditTable(rows) : renderActivityJobs(rows)) + since;
}
function activityJobRow(job, cls = "") {
  return '<tr class="' + cls + '"><td>' + esc(thaiTime(job.sentAt)) + "</td><td>" + activityHost(job.targetId, job.label) + '</td><td><code class="script-line">' + esc(String(job.script || "").split("\n")[0]) + "</code></td><td>" + activityBadge(job.status) +
    "</td><td>" + (job.exitCode ?? "—") + '</td><td class="actions"><button class="small" data-action="activity-open" data-target="' + esc(job.targetId) + '" data-job="' + esc(job.jobId) + '">' + T.activity.open + "</button></td></tr>";
}
function renderActivityJobs(entries) {
  const t = T.activity, open = activityState().open;
  const body = entries.map(entry => {
    if (entry.kind !== "group") return activityJobRow(entry.job);
    // A retried host has a row per attempt; the counts use each host's latest one.
    const hosts = entry.hosts || [], latest = [...new Map(hosts.map(job => [job.targetId, job])).values()];
    const ok = latest.filter(job => job.status === "succeeded").length, shown = open.has(entry.groupId);
    return '<tr class="group-row"><td>' + esc(thaiTime(entry.sentAt)) + '</td><td colspan="2"><strong>' + esc(t.group(selectorLabel(entry.selector), latest.length)) + "</strong></td><td>" + esc(t.groupDone(ok, latest.length)) +
      '</td><td></td><td class="actions"><button class="small" data-action="activity-toggle" data-group="' + esc(entry.groupId) + '" aria-expanded="' + shown + '">' + (shown ? t.hide : t.show) + "</button></td></tr>" +
      (shown ? hosts.map(job => activityJobRow(job, "group-host")).join("") : "");
  }).join("");
  const h = t.head;
  return '<div class="table-wrap"><table class="activity-table"><thead><tr><th>' + h.time + "</th><th>" + h.host + "</th><th>" + h.script + "</th><th>" + h.status + "</th><th>" + h.exit + "</th><th></th></tr></thead><tbody>" + body + "</tbody></table></div>";
}
function channelLabel(channel) {
  const names = T.activity.channels, text = String(channel || "unknown");
  return text.startsWith("merge: ") ? names.merge(text.slice(7)) : names[text] || text;
}
function renderAuditTable(lines) {
  const t = T.activity, h = t.head;
  const body = lines.map(line => '<tr class="' + (line.result === "ok" ? "" : "failed") + '"><td>' + esc(thaiTime(line.time)) + "</td><td>" + esc(t.actions[line.action] || line.action) + '</td><td class="mono">' + esc(line.object) +
    "</td><td>" + (line.result === "ok" ? esc(t.ok) : '<code class="badge bad">' + esc(line.result) + "</code>") + "</td><td>" + esc(channelLabel(line.channel)) + "</td></tr>").join("");
  return '<div class="table-wrap"><table class="activity-table audit-table"><thead><tr><th>' + h.time + "</th><th>" + h.action + "</th><th>" + h.object + "</th><th>" + h.result + "</th><th>" + h.channel + "</th></tr></thead><tbody>" + body + "</tbody></table></div>";
}
/** A recorded job whose host can't be reached: what the Master wrote down, and why the output can't be read. */
function renderActivityDetail({ job, reason }) {
  const t = T.activity;
  const field = (label, value) => "<div><dt>" + label + "</dt><dd>" + value + "</dd></div>";
  return '<section class="surface activity-detail" id="activity-detail"><div class="section-heading"><h2>' + t.detailTitle + " · " + activityHost(job.targetId, job.label) + '</h2><button class="small" data-action="activity-detail-close">' + t.close + "</button></div>" +
    '<div class="callout pending">' + icons.alert + "<div><p>" + esc(t.offline) + '</p><small class="error-detail">' + esc(reason) + "</small></div></div>" +
    '<div class="output light"><pre>' + esc(job.script) + "</pre></div>" +
    '<dl class="activity-fields">' + field(T.activity.head.status, activityBadge(job.status)) + field(t.cwd, "<code>" + esc(job.cwd || "—") + "</code>") + field(t.sent, esc(thaiTime(job.sentAt))) +
    field(t.started, esc(job.startedAt ? thaiTime(job.startedAt) : "—")) + field(t.finished, esc(job.finishedAt ? thaiTime(job.finishedAt) : "—")) +
    ((job.envNames || []).length ? field(t.envNames, "<code>" + esc(job.envNames.join(", ")) + "</code>") : "") + "</dl></section>";
}
function findActivityJob(jobId) {
  for (const entry of activityState().jobs || []) {
    if (entry.kind === "group") { const job = entry.hosts.find(row => row.jobId === jobId); if (job) return job; }
    else if (entry.job.jobId === jobId) return entry.job;
  }
  return null;
}
/** Open: the host's Jobs tab with the job selected; when the host doesn't answer, the recorded details here. */
async function openActivityJob(targetId, jobId) {
  const a = activityState();
  try {
    const job = await rpc("jobs.status", { targetId, jobId });
    a.detail = null;
    await selectTarget(targetId, "jobs");
    state.job = job; state.groupRun = null; render(); await refreshSelectedJob(true);
  } catch (error) {
    a.detail = { job: findActivityJob(jobId) || { jobId, targetId }, reason: String(error?.message || error).replace(/^Error:\s*/, "") };
    redrawActivity();
  }
}
async function exportActivity() {
  const a = activityState(), t = T.activity;
  const path = await native("save_csv", { params: { ...activityParams(), view: a.view }, name: t.fileName(a.view, thaiDay()), title: T.dialogs.exportCsv });
  if (path) notify(t.exported(path));
}
/** Keep-days field on the current Master's Summary tab, next to the backup section. */
function renderActivityKeep() {
  const t = T.activity, info = activityState().info;
  return '<section class="section mt" id="activity-keep"><div class="section-heading"><div><h2>' + t.keepTitle + "</h2><p>" + t.keepLead + "</p></div></div>" + renderActivityWarning() +
    '<form id="activity-keep-form" class="inline-form"><label>' + t.keepDays + '<input name="keepDays" type="number" min="1" max="3650" required value="' + esc(info?.keepDays ?? 14) + '"></label>' +
    '<button type="submit">' + t.keepSave + "</button></form></section>";
}
async function refreshActivityStatus() { activityState().info = await rpc("activity.status").catch(() => activityState().info); }
let groupTimer = 0;
/** Polls the shown run until every host has finished; only the run's own panel is redrawn. */
async function watchGroup() {
  clearTimeout(groupTimer); groupTimer = 0;
  const shown = state.groupRun;
  if (!shown) return;
  const run = await rpc("groups.status", { runId: shown.id }).catch(() => null);
  if (state.groupRun?.id !== shown.id) return;
  if (run) state.groupRun = run;
  const panel = $("#group-run"); if (panel) panel.outerHTML = renderGroupRun(state.groupRun);
  if (!state.groupRun.done) groupTimer = setTimeout(() => void watchGroup(), 2000);
  else if (activityShown()) await loadActivity();
}
/** Open log: that host's Jobs tab with the group's job selected. */
async function openGroupLog(id, jobId) {
  await selectTarget(id, "jobs");
  state.job = await rpc("jobs.status", { targetId: id, jobId }).catch(() => ({ id: jobId }));
  state.groupRun = null; render(); await refreshSelectedJob(true);
}
/** The working folder a new job starts with: the root of the host's drive, so a quick command needs no path. */
const defaultCwd = os => /win/i.test(os || "") ? "C:\\" : "/";
function renderNewJob(item) {
  const windows = isWindows(item), t = T.jobs;
  return '<form id="start-job" class="surface new-job"><h2>' + t.newJob + '</h2><div class="form-grid">' +
    '<label class="full">' + t.runOn + '<select name="runOn">' + runOnOptions() + "</select></label>" +
    '<label class="full" data-cwd>' + t.cwd + '<input name="cwd" required class="mono" value="' + defaultCwd(item.os) + '" placeholder="' + (windows ? "C:\\projects\\my-app" : "/srv/my-app") + '"></label>' +
    '<label class="full" data-script="single">' + t.script + '<textarea name="script" required rows="6" class="mono" spellcheck="false" placeholder="' + (windows ? "Get-Location" : "pwd") + '"></textarea></label>' +
    '<label class="full" data-script="pair" hidden>' + t.scriptLinux + '<textarea name="scriptLinux" rows="4" class="mono" spellcheck="false" placeholder="pwd"></textarea></label>' +
    '<label class="full" data-script="pair" hidden>' + t.scriptWindows + '<textarea name="scriptWindows" rows="4" class="mono" spellcheck="false" placeholder="Get-Location"></textarea></label>' +
    '<p class="help full" data-script="pair" hidden>' + t.mixedHelp + "</p></div>" +
    "<details><summary>" + t.advanced + '</summary><div class="form-grid"><label class="host-only">' + t.shell + '<select name="shell"><option value="' + (windows ? "powershell" : "bash") + '">' + (windows ? "PowerShell" : "Bash") + "</option></select></label>" +
    '<label class="host-only">' + t.session + '<input name="ownerSessionId" placeholder="' + esc(t.sessionPlaceholder) + '"></label>' +
    (!windows ? '<label class="check-label full"><input type="checkbox" name="asAdmin">' + t.asAdmin + "</label>" : "") +
    '<label class="full">' + t.env + " <small>" + t.envHelp + '</small><textarea name="env" rows="2" class="mono" spellcheck="false">{}</textarea></label></div></details>' +
    '<div class="form-actions"><button type="submit" class="primary"' + (!item.runnerInstalled ? " disabled" : "") + ">" + t.start + "</button>" +
    '<button type="button" data-action="toggle-new-job">' + T.common.cancel + '</button><span class="help">' + t.help + "</span></div></form>";
}
function renderJobRows() {
  if (state.jobsError) return '<div class="error-inline">' + esc(state.jobsError) + "</div>";
  if (!state.jobs.length) return '<div class="empty-inline">' + T.jobs.empty + "<br>" + T.jobs.emptyHint + "</div>";
  return state.jobs.map(job => '<button class="job-row ' + (state.job?.id === job.id ? "active" : "") + '" data-action="select-job" data-id="' + esc(job.id) + '"><div>' + badge(job.status) + "<small>" + esc(timestamp(job.createdAt)) + "</small></div><code>" + esc(job.script) + "</code></button>").join("");
}
function renderJobDetail() {
  const job = state.job, t = T.jobs;
  if (!job) return '<div class="empty-inline">' + t.select + "<br><small>" + t.selectHint + "</small></div>";
  const running = ["running", "queued"].includes(job.status);
  return '<div class="job-head"><h3>' + t.job + ' <code title="' + esc(t.idTitle(job.id, job.masterId)) + '">' + esc(job.id.slice(0, 8)) + "</code></h3>" + badge(job.status) +
    (job.exitCode != null ? '<span class="chip">' + esc(t.exit(job.exitCode)) + "</span>" : "") +
    (running ? '<button class="danger small right" data-action="cancel-job">' + t.cancel + "</button>" : "") + "</div>" +
    '<div class="output light"><pre>' + esc(job.script) + "</pre></div>" +
    '<div class="job-meta"><span>' + icons.folder + "<code>" + esc(job.cwd) + "</code></span><span>" + esc(timestamp(job.startedAt || job.createdAt)) + "</span></div>" +
    '<div class="console"><div class="console-bar"><div class="segmented" role="group" aria-label="' + esc(t.stream) + '">' +
    ["stdout", "stderr"].map(stream => '<button data-action="log-stream" data-stream="' + stream + '" aria-pressed="' + (state.stream === stream) + '">' + stream + "</button>").join("") +
    '</div><button class="small" data-action="read-logs">' + (state.eof ? t.more : t.next) + '</button><button class="small" data-action="copy-log">' + icons.copy + T.common.copy + "</button>" +
    (running ? '<small><span class="dot good"></span>' + t.updates + "</small>" : "") +
    '</div><div class="output" id="log-output" tabindex="0" aria-label="' + esc(t.logs) + '"><pre>' + esc(state.log || t.noOutput) + "</pre></div></div>";
}
const joinPath = (folder, name) => folder.replace(/[\\/]+$/, "") + (/^[A-Za-z]:/.test(folder) ? "\\" : "/") + name;
/** Delete safety layers: right-click only, a dialog naming host and full path, typing the exact name, and the service refusing non-empty folders. */
function openDeleteDialog(path, name, isDirectory) {
  const form = $("#confirm-delete");
  form.reset(); form.dataset.path = path; form.dataset.name = name; form.dataset.dir = isDirectory ? "1" : "";
  fillDeleteDialog();
  form.querySelector('[type="submit"]').disabled = true;
  $("#delete-dialog").showModal();
  form.querySelector('[name="confirm"]').focus();
}
/** The delete dialog's words; also re-run when the language changes while it is open (what was typed stays). */
function fillDeleteDialog() {
  const form = $("#confirm-delete"), t = T.files, dir = form.dataset.dir === "1";
  const host = state.targets.find(row => row.id === state.selected);
  $("#delete-title").textContent = t.deleteTitle(dir);
  $("#delete-name").textContent = form.dataset.name || "";
  $("#delete-path").textContent = form.dataset.path || "";
  $("#delete-host").textContent = host ? host.label || host.id : T.common.thisHost;
  $("#delete-note").textContent = dir ? t.deleteDirNote : t.deleteNote;
}
function parentPath(path) {
  const value = String(path || "").trim().replace(/[\\/]+$/, "");
  if (/^[A-Za-z]:$/.test(value)) return value + "\\";
  const index = Math.max(value.lastIndexOf("/"), value.lastIndexOf("\\"));
  if (index < 0) return value || "/";
  if (index === 0) return value[0];
  const parent = value.slice(0, index);
  return /^[A-Za-z]:$/.test(parent) ? parent + "\\" : parent;
}
/** WinSCP-like browser: path box with a folder dropdown, Up, New folder, Upload; drop files to upload; right-click to delete. */
function renderFiles(item) {
  const path = state.files?.path || "~", t = T.files;
  return '<div class="section-heading"><div><h2>' + t.title + '</h2><p class="help">' + t.help + "</p></div></div>" +
    '<div class="file-bar"><button type="button" class="icon-button" data-action="parent-folder" aria-label="' + esc(t.up) + '" title="' + esc(t.up) + '"' + (state.files ? "" : " disabled") + ">" + icons.up + "</button>" +
    '<form id="browse-files" class="path-pick"><input name="path" id="file-path" aria-label="' + esc(t.path) + '" class="mono" required value="' + esc(path) + '">' +
    '<button type="button" class="icon-button" data-action="path-menu" aria-label="' + esc(t.choose) + '" title="' + esc(t.choose) + '">' + icons.chevron + "</button></form>" +
    '<div class="search file-search"><svg class="i" viewBox="0 0 16 16" aria-hidden="true"><circle cx="7" cy="7" r="4.5"/><path d="m10.5 10.5 3.5 3.5"/></svg><input id="file-filter" type="search" aria-label="' + esc(t.search) + '" placeholder="' + esc(t.searchPlaceholder) + '" value="' + esc(state.fileQuery || "") + '"' + (state.files ? "" : " disabled") + "></div>" +
    '<button type="button" data-action="mkdir"' + (state.files ? "" : " disabled") + ">" + icons.plus + t.newFolder + '</button><button type="button" class="accent" data-action="upload-pick"' + (state.files ? "" : " disabled") + ">" + icons.up + t.upload + '</button><input type="file" id="upload-input" multiple hidden></div>' +
    '<div id="upload-status">' + renderUploadStatus() + '</div><div id="files-output" class="file-drop">' + renderFileList() + '</div><div id="file-editor">' + renderFileEditor() + "</div>";
}
const fileTime = seconds => seconds ? new Date(seconds * 1000).toLocaleString(T.locale, { year: "numeric", month: "short", day: "2-digit", hour: "2-digit", minute: "2-digit" }) : "—";
function renderFileList() {
  const t = T.files;
  if (!state.files && state.filesError) return '<div class="callout pending">' + icons.alert + "<div><strong>" + t.cantOpen + "</strong><p>" + errorHtml(explain(state.filesError)) + "</p></div></div>";
  if (!state.files) return '<div class="empty-inline">' + t.reading + "</div>";
  const query = (state.fileQuery || "").trim().toLowerCase();
  const entries = [...state.files.entries].filter(entry => !query || String(entry.name).toLowerCase().includes(query)).sort((a, b) => (b.isDirectory ? 1 : 0) - (a.isDirectory ? 1 : 0) || String(a.name).localeCompare(String(b.name)));
  const up = parentPath(state.files.path), atRoot = up === state.files.path;
  const parent = atRoot ? "" : '<tr><td><button class="row-button file-name folder" data-action="open-folder" data-path="' + esc(up) + '">' + icons.up + '..</button></td><td class="num muted">—</td><td class="muted">' + t.parent + "</td><td></td></tr>";
  return '<div class="table-wrap"><table class="file-table"><thead><tr><th>' + t.head.name + "</th><th>" + t.head.size + "</th><th>" + t.head.modified + '</th><th><span class="sr-only">' + t.head.actions + "</span></th></tr></thead><tbody>" + parent +
    entries.map(entry => '<tr data-file="' + esc(entry.path) + '" data-name="' + esc(entry.name) + '" data-dir="' + (entry.isDirectory ? "1" : "") + '"><td><button class="row-button file-name' + (entry.isDirectory ? " folder" : "") + '" data-action="' + (entry.isDirectory ? "open-folder" : "open-file") + '" data-path="' + esc(entry.path) + '">' + (entry.isDirectory ? icons.folder : icons.file) + esc(entry.name) + "</button>" + (entry.isSymlink ? " " + chip(t.link) : "") +
      '</td><td class="num muted" title="' + esc(t.bytes(entry.size ?? "")) + '">' + (entry.isDirectory ? "—" : esc(formatSize(entry.size))) + '</td><td class="muted">' + esc(fileTime(entry.modified)) + '</td><td class="row-more-cell"><button type="button" class="icon-button row-more" data-action="file-more" aria-haspopup="menu" aria-label="' + esc(t.more(entry.name)) + '" title="' + esc(t.moreShort) + '">' + icons.more + "</button></td></tr>").join("") +
    (entries.length ? "" : '<tr><td colspan="4" class="muted">' + (query ? esc(t.noMatch(state.fileQuery.trim())) : t.empty) + "</td></tr>") + "</tbody></table></div>";
}
function renderFileEditor() {
  if (!state.file) return "";
  const t = T.files;
  return '<div class="file-editor surface"><div class="section-heading"><h3><code>' + esc(state.file.path) + '</code></h3><button class="icon-button" data-action="close-file" aria-label="' + esc(t.closeFile) + '" title="' + esc(t.closeFile) + '">' + icons.close + "</button></div>" +
    '<form id="save-file"><textarea name="content" aria-label="' + esc(t.content) + '" class="mono" rows="14" spellcheck="false"' + (!state.file.editable ? " readonly" : "") + ">" + esc(state.file.content) + '</textarea><div class="form-actions"><button type="submit" class="primary"' + (!state.file.editable ? " disabled" : "") + ">" + t.save + '</button><span class="help">' + (state.file.editable ? t.shaHelp : t.readOnly) + "</span></div></form></div>";
}
function pairingState(item) {
  return addText.states[item?.state] || addText.preparing;
}
/** Plain-language versions of errors people can act on (strings.js `errors`), the first matching code wins:
 *  `{text, detail}`. `pairing` adds the add flow's pairing codes. Anything unmapped is shown as is, in English.
 *  In Thai, a mapped error keeps the Master's English text (without its `E_…:` prefix) as `detail`, shown in a small
 *  line under the sentence so it can still be searched or pasted into a bug report. */
function explain(error, pairing = false, hostName = "") {
  const text = String(error?.message || error);
  const host = state.targets.find(row => row.id === state.selected), name = hostName || (host ? host.label || host.id : T.common.thisHost);
  const rows = [...(pairing ? T.errors.pairing : []), ...T.errors.common];
  const found = rows.find(([code]) => text.includes(code))?.[1];
  if (!found) return { text, detail: "" };
  return { text: typeof found === "function" ? found(name, text) : found, detail: lang === "en" ? "" : text.replace(/^(Error:\s*)?E_[A-Z_]+:\s*/, "") };
}
/** An error from `explain` (or its text) as markup: the sentence, then the English detail line when there is one. */
const errorHtml = shown => typeof shown === "string" ? esc(shown) : esc(shown.text) + (shown.detail ? '<small class="error-detail">' + esc(shown.detail) + "</small>" : "");
async function refreshPairings() {
  const rows = await rpc("pairing.status");
  if (!Array.isArray(rows)) throw new Error(T.errors.pairingRead);
  state.pairings = rows;
  if (state.pairing) state.pairing = rows.find(item => item.requestId === state.pairing.requestId) || state.pairing;
}
/** The add column's current step, from the request, whether its installer was saved and the pasted code:
 *  no request → 1; request not saved → 1 (Save installer… is in step 1); saved → 3; code previewed (nothing
 *  saved yet) → 4 "review"; after Finish (ready / needs_attention) → 4 "result". Step 2 is never current: it is
 *  done once the installer is saved. */
function addStep({ request, saved, previewed }) {
  if (request && ["ready", "needs_attention"].includes(request.state)) return { n: 4, phase: "result" };
  if (request && previewed) return { n: 4, phase: "review" };
  if (request && saved) return { n: 3, phase: "paste" };
  return { n: 1, phase: request ? "save" : "where" };
}
function addReset() { state.add = { code: "", checked: "", error: "", preview: null, focused: null, skipped: state.add?.skipped || new Set() }; }
function renderAddHost(options = {}) {
  const request = state.pairing, saved = request ? state.saved.get(request.requestId) : null;
  const preview = state.add.preview?.requestId === request?.requestId ? state.add.preview : null;
  const now = addStep({ request, saved: Boolean(saved || state.add.skipped.has(request?.requestId)), previewed: Boolean(preview) });
  const folders = (state.folders.folders || []).filter(folder => folder.master === "local");
  const folder = folders.find(row => row.id === options.folder), site = (state.datacenters.datacenters || []).find(row => row.id === options.site);
  const card = (n, body) => {
    const status = n === now.n ? "is-now" : n < now.n ? "is-done" : "is-later";
    return '<section class="add-step ' + status + '" data-step="' + n + '"' + (status === "is-now" ? ' aria-current="step" data-phase="' + now.phase + '"' : "") + '><div class="add-n">' + (status === "is-done" ? "✓" : n) + "</div>" +
      '<div class="add-body"><h2>' + addText.steps[n - 1] + "</h2>" + (n <= now.n ? body : "") + "</div></section>";
  };
  const requests = state.pairings.filter(item => item.state !== "ready");
  return '<div id="add-host" class="add-flow">' + heading(addText.title, [folder && addText.inFolder(folder.name), site && addText.inSite(site.name), addText.lead].filter(Boolean).join(" · "), '<button data-action="enroll-close">' + icons.close + addText.close + "</button>") +
    '<div class="add-column">' + card(1, addWhere(request, now, options, folders)) + card(2, addInstall(request, saved)) + card(3, addPaste(now)) + card(4, now.phase === "review" ? addReview(preview, folders) : request ? addResult(request) : "") + "</div>" +
    addUnfinished(requests) + "</div>" +
    '<details class="mt"><summary>' + addText.admin + '</summary><div><p class="help">' + addText.adminHelp + '</p><div class="page-actions"><button data-action="import-registration">' + addText.importFile + '</button><button data-action="import-v1-registry">' + addText.importV1 + '</button></div><form id="paste-registration" class="mt"><label>' + addText.registrationJson + '<textarea name="document" class="mono" rows="4" required spellcheck="false"></textarea></label><div class="form-actions"><button type="submit">' + addText.import + "</button></div></form></div></details>";
}
/** The address a pairing request was made for (its first route). */
const requestAddress = request => (request?.envelope?.bundle?.routes || [])[0]?.address || "";
/** Folder choice for step 1 and the review card; "+ New folder…" shows a name box (see the change handler). */
function folderSelect(folders, selected = "") {
  const option = (value, text) => '<option value="' + esc(value) + '"' + (value === (selected || "") ? " selected" : "") + ">" + esc(text) + "</option>";
  return '<label>' + addText.folder + '<select name="folder" data-new-folder>' + option("", addText.none) + folders.map(row => option(row.id, row.name)).join("") + option("__new__", addText.newFolder) +
    '</select><input name="newFolder" maxlength="64" placeholder="' + addText.newFolderName + '" hidden></label>';
}
/** The folder picked in a form with `folderSelect`, creating it first for "+ New folder…" ("" = none). */
async function chosenFolder(values) {
  if (values.folder !== "__new__") return values.folder;
  return values.newFolder.trim() ? (await rpc("folders.create", { name: values.newFolder.trim() })).id : "";
}
/** Step 1: the address up front; name and folder under "Optional"; today's fields under "Advanced". */
function addWhere(request, now, options, folders) {
  if (request) {
    return '<dl class="facts"><dt>' + addText.address + "</dt><dd>" + esc(requestAddress(request)) + "</dd><dt>" + addText.name + "</dt><dd>" + esc(request.label || request.targetId) + "</dd></dl>" +
      (now.phase === "save" ? '<div class="form-actions"><button class="primary" data-action="save-setup" data-focus>' + addText.save + '</button><button data-action="add-skip-save">' + addText.haveInstaller + '</button><button class="danger" data-action="pairing-cancel" data-id="' + esc(request.requestId) + '">' + addText.cancelRequest + "</button></div>" : "");
  }
  const groups = [...new Set(state.targets.map(item => item.tenant).filter(Boolean))], tenant = groups[0] || "lab";
  const option = (value, text, selected) => '<option value="' + esc(value) + '"' + (selected ? " selected" : "") + ">" + esc(text) + "</option>";
  return '<form id="create-pairing" class="add-where">' +
    '<label>' + addText.address + '<input id="add-address" class="add-big" name="address" required autocomplete="off" spellcheck="false" placeholder="' + addText.addressPlaceholder + '" value="' + (options.self ? "127.0.0.1" : "") + '" data-focus><small>' + addText.addressHelp + "</small></label>" +
    '<p class="add-check" id="add-address-check" aria-live="polite"></p>' +
    "<details><summary>" + addText.optional + '</summary><div class="form-grid"><label>' + addText.name + '<input name="label" maxlength="256" placeholder="' + addText.namePlaceholder + '" value="' + (options.self ? esc(state.service?.label || "") : "") + '"></label>' +
    folderSelect(folders, options.folder) + "</div></details>" +
    "<details><summary>" + addText.advanced + '</summary><div class="form-grid">' +
    '<label>' + addText.os + '<select name="os">' + option("", addText.osDetect) + Object.entries(addText.osNames).map(([value, text]) => option(value, text)).join("") + "</select></label>" +
    '<label>' + addText.group + '<input name="tenant" required value="' + esc(tenant) + '" list="pairing-groups" pattern="[A-Za-z0-9_\\-]+" maxlength="64"><datalist id="pairing-groups">' + groups.map(group => '<option value="' + esc(group) + '"></option>').join("") + "</datalist><small>" + addText.groupHelp + "</small></label>" +
    '<label>' + addText.hostId + '<input name="targetId" pattern="[A-Za-z0-9_\\-]+" maxlength="64" placeholder="' + addText.generated + '"></label><label>' + addText.port + '<input name="port" type="number" value="22" min="1" max="65535" required></label>' +
    '<label>' + addText.network + '<select name="routeKind">' + option("", addText.detect) + option("lan", addText.lan) + option("tailscale", addText.tailscale) + option("public", addText.publicIp) + "</select></label>" +
    '<label>' + addText.bridge + '<input name="user" value="sshcc-bridge" required></label><label class="full">' + addText.routes + '<textarea name="routes" rows="4" class="mono" placeholder="' + addText.routesPlaceholder + '"></textarea><small>' + addText.routesHelp + "</small></label></div></details>" +
    '<div class="form-actions"><button type="submit" class="primary">' + addText.save + '</button></div><p class="help">' + addText.saveHelp + "</p></form>";
}
/** Step 1 live check: `pairing.check` after a ~400 ms pause; only the answer for the current address/port is shown. */
let whereTimer = 0;
const whereKey = form => form.address.value.trim() + ":" + (Number(form.port.value) || 22);
function onWhereInput(form) {
  clearTimeout(whereTimer); setWhereCheck(form, "");
  if (form.address.value.trim()) whereTimer = setTimeout(() => void checkWhere(form), 400);
}
function setWhereCheck(form, html, tone = "", duplicate = false) {
  const line = $("#add-address-check", form), save = $('button[type="submit"]', form);
  line.innerHTML = html; line.className = "add-check" + (tone ? " " + tone : "");
  save.disabled = duplicate; save.toggleAttribute("data-blocked", duplicate); // `busy` leaves a blocked button disabled
  form.dataset.duplicate = duplicate ? "1" : "";
}
async function checkWhere(form) {
  const key = whereKey(form), address = form.address.value.trim(), port = Number(form.port.value) || 22;
  setWhereCheck(form, esc(addText.checking));
  let result;
  try { result = await rpc("pairing.check", { address, port }); } catch { result = null; }
  // Resume needs the request in the list; it may have been made elsewhere since the list was read.
  if (result?.alreadyRequestId && !state.pairings.some(item => item.requestId === result.alreadyRequestId)) await refreshPairings().catch(() => {});
  if (!form.isConnected || whereKey(form) !== key) return; // a newer value is being checked
  form.dataset.os = result?.os || ""; form.dataset.osKey = key; // what Save installer… uses when Advanced OS is "Detect"
  if (!result) return setWhereCheck(form, "");
  if (result.alreadyAs) {
    const link = result.alreadyTargetId ? '<button type="button" class="small" data-action="open-paired-target" data-id="' + esc(result.alreadyTargetId) + '">' + addText.openIt + "</button>"
      : result.alreadyRequestId ? '<button type="button" class="small" data-action="resume-pairing" data-id="' + esc(result.alreadyRequestId) + '">' + addText.resumeIt + "</button>" : "";
    return setWhereCheck(form, icons.alert + "<span>" + addText.alreadyAs + " <strong>" + esc(result.alreadyAs) + "</strong></span>" + link, "bad", true);
  }
  if (!result.reachable) return setWhereCheck(form, icons.info + "<span>" + esc(addText.noSsh) + "</span>");
  setWhereCheck(form, '<span class="dot good"></span><span>' + esc(addText.sshAnswers(addText.bannerOs[result.os])) + "</span>", "good");
}
/** Step 2: one line; done as soon as the installer is saved. The Ubuntu Server command is folded under it. */
function addInstall(request, saved) {
  const name = path => path.split(/[\\/]/).pop();
  if (saved?.paths) { // an unknown-OS pair: both installers in one folder
    const [exe, deb] = ["exe", "deb"].map(ext => name(saved.paths.find(path => path.endsWith("." + ext)) || ""));
    return "<p>" + addText.openEither(exe, deb) + '</p><p class="help wrap">' + addText.savedTo + " " + esc(saved.folder || "") + " · " + addText.bothHelp + "</p>" +
      "<details><summary>" + addText.serverHint + "</summary><div>" + copyRow(addText.serverCommand(deb)) + "</div></details>";
  }
  const file = saved?.path ? name(saved.path) : "";
  return "<p>" + (file ? addText.open(file) : addText.openAny) + "</p>" + (saved?.path ? '<p class="help wrap">' + addText.savedTo + " " + esc(saved.path) + "</p>" : "") +
    (request?.os === "linux" || request?.sibling ? "<details><summary>" + addText.serverHint + "</summary><div>" + copyRow(addText.serverCommand(file && request?.os === "linux" ? file : "FILE.deb")) + "</div></details>" : "");
}
/** Step 3: the code is checked on paste, once per distinct value; no submit button. */
function addPaste(now) {
  // Done: the pasted code stays where it was (read only, same layout) so nothing above step 4 moves.
  if (now.n > 3) return state.add.code ? '<label>' + addText.code + '<textarea class="mono" rows="3" readonly spellcheck="false">' + esc(state.add.code) + "</textarea></label>" +
    '<p class="add-check good">' + esc(addText.codeAccepted(state.add.accepted)) + '</p><p class="help">' + addText.codeHelp + "</p>" : '<p class="add-check">' + icons.check + addText.codeDone + "</p>";
  const error = state.add.error, fetched = state.add.fetch?.requestId === state.pairing?.requestId ? state.add.fetch : null;
  // Entry 8: the Master fetches the code itself and both screens show the same 6 digits; pasting stays as the
  // fallback, and is the only way with an older installer.
  // Until the first answer (or for an older installer) the paste box is shown as before.
  if (fetched && fetched.state !== "old" && !error && !state.add.code) {
    const paste = "<details" + (state.add.pasting ? " open" : "") + ' id="add-paste-instead"><summary>' + addText.pasteInstead + "</summary><div>" + pasteBox(error) + "</div></details>";
    if (fetched?.state === "ready") return '<div class="add-compare" id="add-compare"><p>' + addText.compareLead + '</p><p class="compare-digits" aria-live="polite">' + esc(fetched.digits) + "</p>" +
      '<div class="form-actions"><button class="primary" data-action="add-same" data-focus>' + addText.same + '</button><button class="danger" data-action="add-different">' + addText.different + "</button></div></div>" + paste;
    return '<p class="add-check" id="add-waiting" aria-live="polite"><span class="dot pending"></span>' + addText.waitingApprove + "</p>" + paste;
  }
  return (fetched?.state === "old" ? '<p class="help">' + addText.olderInstaller + "</p>" : "") + pasteBox(error);
}
function pasteBox(error) {
  return '<label>' + addText.code + '<textarea id="add-code" class="mono" rows="3" maxlength="65536" autocomplete="off" spellcheck="false" placeholder="SSHCC1.…" data-focus>' + esc(state.add.code) + "</textarea></label>" +
    '<p class="add-check' + (error ? " bad" : "") + '" id="add-code-check" aria-live="polite">' + (error ? errorHtml(explain(error, true)) : esc(addText.codeIdle)) + '</p><p class="help">' + addText.codeHelp + "</p>";
}
/** While step 3 waits, ask the new machine for its code every 5 s (only one loop; it stops when step 3 is left). */
let fetchTimer = 0;
function watchFetch() {
  const request = state.pairing, inPaste = (state.page === "enroll" || state.dc.adding) && $('#add-host .add-step[data-step="3"][aria-current="step"]');
  if (!request || !inPaste || fetchTimer) return;
  fetchTimer = setTimeout(async () => {
    fetchTimer = 0;
    const current = state.pairing;
    if (!current || current.requestId !== request.requestId) return;
    const reply = await rpc("pairing.fetch", { requestId: request.requestId }).catch(() => null);
    const before = JSON.stringify(state.add.fetch || null);
    if (reply && state.pairing?.requestId === request.requestId) state.add.fetch = reply;
    // Redraw on news, but never over a code being typed or pasted.
    if (JSON.stringify(state.add.fetch || null) !== before && !state.add.code) render();
    if (state.add.fetch?.state !== "ready" && state.add.fetch?.state !== "old") watchFetch();
  }, state.add.fetch ? 5000 : 800);
}
/** Save installer…: the native save dialog; once saved, step 2 is done and the flow moves to step 3. */
async function saveInstaller() {
  const request = state.pairing;
  if (!request) return;
  const saved = await native("save_setup", { requestId: request.requestId, title: T.dialogs.installerFile, folderTitle: T.dialogs.installerFolder });
  if (saved) for (const id of [request.requestId, request.sibling].filter(Boolean)) state.saved.set(id, saved);
  render();
}
/** The line under the code box: plain text, or an error from `explain` (sentence plus the English detail in Thai). */
function setCodeCheck(text, tone = "") { const line = $("#add-code-check"); if (line) { line.innerHTML = errorHtml(text); line.className = "add-check" + (tone ? " " + tone : ""); } }
let codeTimer = 0;
const CODE_ACCEPTED_MS = 700;
/** A paste is checked at once; typing waits for a pause. Each distinct value is sent once. */
function onCodeInput(box, pasted) {
  state.add.code = box.value; clearTimeout(codeTimer);
  codeTimer = setTimeout(() => void checkCode(box.value.trim()), pasted ? 0 : 500);
}
/** Previews the code (read only); a good code opens the review card, nothing is saved until Finish. */
async function checkCode(code) {
  if (!code || code === state.add.checked) return;
  state.add.checked = code; state.add.error = ""; setCodeCheck(addText.codeChecking);
  try {
    const result = await rpc("pairing.preview", { code });
    if (state.add.checked !== code) return;
    // A code for another unfinished request moves the column to that request.
    if (result.requestId !== state.pairing?.requestId) state.pairing = state.pairings.find(item => item.requestId === result.requestId) || state.pairing;
    // Hold the confirmation on step 3 for a moment so the jump to step 4 is not a surprise.
    state.add.accepted = result.hostName || result.label; setCodeCheck(addText.codeAccepted(state.add.accepted), "good");
    await new Promise(resolve => setTimeout(resolve, CODE_ACCEPTED_MS));
    if (state.add.checked !== code) return; // edited, cancelled or closed during the pause
    state.add.preview = { ...result, code }; render();
  } catch (error) {
    if (state.add.checked !== code) return; // a newer value is being checked
    state.add.error = String(error?.message || error); setCodeCheck(explain(error, true), "bad");
  }
}
/** Step 4 review: facts read from the code, with name and folder editable; Finish saves. */
function addReview(preview, folders) {
  return '<form id="finish-pairing" class="add-where"><dl class="facts"><dt>' + addText.hostName + "</dt><dd>" + esc(preview.hostName || "—") + "</dd><dt>" + addText.address + "</dt><dd>" + esc(requestAddress(state.pairing)) +
    "</dd><dt>" + addText.osName + "</dt><dd>" + esc(addText.osNames[preview.os] || preview.os) + "</dd><dt>" + addText.hostKey + '</dt><dd><code class="wrap">' + esc(preview.hostKeySha256) + "</code></dd></dl>" +
    '<div class="form-grid"><label>' + addText.shownAs + '<input name="label" maxlength="256" required value="' + esc(preview.label) + '"></label>' +
    folderSelect(folders, state.folders.members?.[preview.targetId]) + "</div>" +
    '<div class="form-actions"><button type="submit" class="primary" data-focus>' + addText.finish + '</button></div><p class="help">' + addText.reviewHelp + "</p></form>";
}
/** Step 4 result after Finish: ready / needs-attention, in the same card. */
function addResult(item) {
  const registered = state.targets.find(row => row.id === item.targetId);
  const routes = registered?.routes || item.envelope?.bundle?.routes || [];
  const failed = (item.diagnostics?.stages || []).filter(stage => !stage.ok);
  if (item.state === "ready") {
    const offer = addMasterLine(item);
    return '<div class="callout good">' + icons.check + '<div><strong class="ready-message">' + addText.readyTitle + "</strong><p>" + addText.readyText + "</p></div></div>" + offer +
      '<div class="form-actions"><button class="primary" data-action="open-paired-target" data-id="' + esc(item.targetId) + '" data-focus>' + addText.openHost + '</button><button data-action="new-pairing">' + icons.plus + addText.another + '</button><button data-action="enroll-close">' + addText.done + "</button></div>";
  }
  return '<div class="callout pending">' + icons.alert + "<div><strong>" + addText.attentionTitle + "</strong><p>" + addText.attentionText + "</p>" + (failed.length ? "<ul>" + failed.map(stage => "<li>" + errorHtml(explain(stage.message, true)) + "</li>").join("") + "</ul>" : "") + "</div></div>" +
    '<form id="retry-pairing"><label>' + addText.address + '<input name="address" required value="' + esc(routes[0]?.address || "") + '" data-focus><small>' + addText.retryHelp + '</small></label><div class="form-actions"><button type="submit" class="primary">' + addText.retry + '</button><button type="button" data-action="pairing-back">' + icons.back + addText.back + "</button></div></form>";
}
/** A sibling pair (unknown OS) is one row: the Windows request stands for both, so its listed partner is hidden. */
const pairPartner = (item, all) => item.sibling ? all.find(row => row.requestId === item.sibling) : undefined;
const standsForPair = (item, all) => !(pairPartner(item, all) && item.os !== "windows");
function addUnfinished(all) {
  // Cancel on the row cancels both requests of a pair.
  const requests = all.filter(item => standsForPair(item, all));
  if (!requests.length) return "";
  return '<section class="section add-unfinished"><div class="section-heading"><h2>' + addText.unfinished + '</h2><button class="small" data-action="refresh-pairings">' + icons.refresh + addText.refresh + "</button></div>" +
    '<div class="table-wrap"><table><thead><tr><th>' + addText.host + "</th><th>" + addText.status + "</th><th></th></tr></thead><tbody>" + requests.map(item => "<tr><td><strong>" + esc(item.label || item.targetId) + "</strong><small>" + esc((pairPartner(item, all) ? addText.eitherOs + " · " : "") + timestamp(item.createdAt)) + "</small></td>" +
      '<td><span class="badge pending">' + esc(pairingState(item)) + '</span></td><td class="actions"><div class="target-actions"><button class="small" data-action="resume-pairing" data-id="' + esc(item.requestId) + '">' + addText.resume + "</button>" +
      (item.state === "awaiting_approval" ? '<button class="small danger" data-action="pairing-cancel" data-id="' + esc(item.requestId) + '">' + addText.cancel + "</button>" : "") + "</div></td></tr>").join("") + "</tbody></table></div></section>";
}
/** Move focus to the current step's main control when the step changes. */
function focusAddStep() {
  const now = $('#add-host [aria-current="step"]');
  if (!now) return;
  const key = (state.pairing?.requestId || "") + ":" + now.dataset.step + ":" + now.dataset.phase;
  // Same step after a re-render: put focus back only if the re-render dropped it.
  if (state.add.focused === key && document.activeElement && document.activeElement !== document.body) return;
  state.add.focused = key; now.scrollIntoView({ block: "nearest" }); now.querySelector("[data-focus]")?.focus();
}
/** The Master line in the result card: merging, merged (host count, Merge gloss, optional Two-way) or failed. */
function addMasterLine(item) {
  const found = state.pairedMaster;
  if (found?.targetId !== item.targetId) return "";
  if (found.status === "merging") return '<div class="callout add-master">' + icons.info + "<div><strong>" + addText.isMasterMerging + "</strong><p>" + addText.mergeGloss + "</p></div></div>";
  const merge = state.merges.find(row => row.targetId === item.targetId);
  if (!merge) return '<div class="callout pending add-master">' + icons.alert + "<div><strong>" + addText.mergeFailed + "</strong><p>" + addText.mergeFailedText + "</p>" +
    (found.error ? '<p class="help wrap">' + addText.mergeReason + " " + esc(found.error) + "</p>" : "") + '<div class="form-actions"><button data-page="masters">' + addText.openMasters + "</button></div></div></div>";
  const two = twoWayButton(merge, addText.twoWay, ""), running = state.twoWay.has(merge.targetId) || merge.reverse;
  return '<div class="callout good add-master">' + icons.check + "<div><strong>" + esc(addText.isMaster((merge.children || []).length)) + "</strong><p>" + addText.mergeGloss + "</p>" +
    (running ? '<p class="add-two-way">' + reverseStatus(merge) + "</p>" : "") + (two ? '<div class="form-actions">' + two + '</div><p class="help">' + addText.twoWayHelp + "</p>" : "") + "</div></div>";
}
/** After a ready pairing: ask only the new host whether it is a Master (no fleet-wide probe). A Master is Merged at
 *  once with no click, and filed under the datacenter the flow was opened from. A non-Master is left alone. */
async function checkPairedMaster(pairing) {
  state.pairedMaster = null;
  if (pairing?.state !== "ready" || !state.service?.roles?.includes("master")) return;
  const targetId = pairing.targetId, site = state.dc.adding?.site;
  try {
    const [row] = await rpc("merges.discover", { targetId });
    if (row && !row.error) state.masterCheck.set(row.targetId, row.isMaster);
    if (!row?.isMaster) return;
  } catch { return; /* not knowing is not an error; the Masters page can still merge */ }
  state.pairedMaster = { targetId, status: "merging" }; render();
  if (site) await rpc("datacenters.assign", { master: targetId, datacenterId: site }).then(refreshDatacenters).catch(error => notify(error, true));
  try {
    if (!state.merges.some(row => row.targetId === targetId)) await rpc("merges.add", { targetId });
    state.pairedMaster = { targetId, status: "merged" };
  } catch (error) {
    state.pairedMaster = { targetId, status: "failed", error: String(error?.message || error).replace(/^E_[A-Z_]+:\s*/, "") };
  }
  await refreshMasters().catch(() => {});
}
function renderMasters() {
  const t = mastersText;
  return heading(t.title, t.lead, '<button data-action="refresh-masters">' + icons.refresh + t.refresh + "</button>") +
    '<div class="stack masters">' + (state.service?.roles?.includes("master") ? renderMerges() : "") + renderRelays() + "</div>";
}
function mergeStatus(targetId) {
  const t = mastersText;
  if (state.merging.has(targetId)) return '<span class="badge running">' + t.merging + "</span>";
  const merge = state.merges.find(row => row.targetId === targetId);
  if (!merge) return '<span class="badge">' + t.notMerged + "</span>";
  const count = (merge.children || []).length, label = t.mergeStates[merge.status];
  const style = merge.status === "merged" ? "good" : "bad", text = typeof label === "function" ? label(count) : label || merge.status;
  const detail = merge.status === "merged" ? t.lastSync(timestamp(merge.syncedAt)) : (merge.error || "") + (count ? t.previousList(count) : "");
  // A merged Master from before entry 6 relays one hop only: say so, and how to see further.
  const outdated = merge.status === "merged" && merge.relayVersion == null
    ? '<span class="badge pending" title="' + esc(T.share.outdatedHelp) + '">' + T.share.outdated + '</span><small class="wrap">' + esc(T.share.outdatedHelp) + "</small>" : "";
  return '<span class="badge ' + style + '">' + esc(text) + '</span><small class="wrap">' + esc(detail) + "</small>" + outdated;
}
function reverseStatus(merge) {
  const running = state.twoWay.get(merge.targetId);
  const t = mastersText;
  if (running) return '<span class="badge running">' + t.enablingTwoWay + '</span><small class="wrap">' + esc(running.message) + "</small>";
  const reverse = merge.reverse, peer = state.targets.find(item => item.id === merge.targetId);
  const relay = state.relays.find(row => row.masterId === peer?.nodeId);
  if (isTwoWay(merge)) return '<span class="badge good">' + t.twoWay + "</span><small>" + esc(t.canControl(merge.label || merge.targetId)) + "</small>";
  if (reverse?.status === "two-way" && relay?.revoked) return '<span class="badge">' + t.oneWay + "</span><small>" + t.mergingDisabledHere + "</small>";
  const styles = { "two-way": "pending", "needs-attention": "bad", error: "bad", pending: "pending" };
  const style = styles[reverse?.status] || "", text = t.reverseStates[reverse?.status] || t.oneWay;
  return '<span class="badge ' + style + '">' + text + "</span>" + (reverse?.error ? '<small class="wrap">' + esc(reverse.error) + "</small>" : !reverse ? "<small>" + t.oneWayOnly + "</small>" : "");
}
/** Today's rule for offering two-way: merged, not two-way yet, and merging not disabled here after a two-way;
 *  Windows Masters only. Used by the Masters page and the add flow's result card. */
function twoWayButton(merge, text, style = "primary small") {
  const id = merge.targetId, peer = state.targets.find(item => item.id === id);
  if (merge.status !== "merged" || isTwoWay(merge) || (merge.reverse?.status === "two-way" && state.relays.some(row => row.masterId === peer?.nodeId && row.revoked))) return "";
  const busyRow = state.merging.has(id) || state.twoWay.has(id);
  return '<button class="' + style + '" data-action="open-two-way" data-id="' + esc(id) + '"' + (busyRow ? " disabled" : !/Windows/.test(navigator.userAgent) ? ' disabled title="' + mastersText.windowsOnly + '"' : "") + ">" + text + "</button>";
}
/** Hosts the merged Masters reach but left out for being more than 4 hops away. */
const beyondCount = () => state.merges.reduce((sum, merge) => sum + (merge.beyond || 0), 0);
/** Outgoing: every Merge, plus every host whose health (or the add flow's check) says it is a Master and that is not merged yet. */
function renderMerges() {
  const unmerged = state.targets.filter(item => !item.relayed && (state.health.get(item.id)?.isMaster || state.masterCheck.get(item.id)) && !state.merges.some(merge => merge.targetId === item.id));
  const ids = [...state.merges.map(row => row.targetId), ...unmerged.map(item => item.id)];
  const name = id => state.merges.find(row => row.targetId === id)?.label || state.targets.find(row => row.id === id)?.label || id, t = mastersText;
  return '<section class="surface"><div class="card-head"><div><h2>' + t.outTitle + "</h2><p>" + t.outLead + "</p></div></div>" +
    (ids.length ? '<div class="table-wrap"><table><thead><tr><th>' + t.master + "</th><th>" + t.status + "</th><th>" + t.direction + "</th><th></th></tr></thead><tbody>" +
      ids.map(id => {
        const merge = state.merges.find(row => row.targetId === id), busyRow = state.merging.has(id) || state.twoWay.has(id);
        const pair = merge ? twoWayButton(merge, merge.reverse ? t.tryAgain : t.enableTwoWay) : "";
        const actions = merge ? pair + '<button class="small" data-action="sync-merge" data-id="' + esc(id) + '"' + (busyRow ? " disabled" : "") + ">" + t.sync + '</button><button class="danger small" data-action="remove-merge" data-id="' + esc(id) + '"' + (busyRow ? " disabled" : "") + ">" + t.removeMerge + "</button>" : '<button class="primary small" data-action="merge-master" data-id="' + esc(id) + '" title="' + t.mergeTitle + '"' + (busyRow ? " disabled" : "") + ">" + t.merge + "</button>";
        return "<tr><td><strong>" + esc(name(id)) + "</strong><small><code>" + esc(id) + "</code></small></td><td>" + mergeStatus(id) + "</td><td>" + (merge ? reverseStatus(merge) : '<span class="muted">' + t.none + "</span>") + '</td><td class="actions"><div class="target-actions">' + actions + "</div></td></tr>";
      }).join("") + "</tbody></table></div>"
      : '<div class="empty-inline">' + t.outEmpty + "</div>") +
    (beyondCount() ? '<p class="help mt" id="beyond-note">' + esc(T.share.beyond(beyondCount())) + "</p>" : "") +
    "<details><summary>" + t.howTitle + '</summary><p class="help">' + t.how + "</p></details></section>";
}
/** What this Master grants a Master that merged it (entry 6): role, Tag limit and pass-through, saved on change.
 *  Only a Merge has a grant; a Master that controls this one directly always has full rights. */
function grantControls(relay) {
  if (!relay || relay.revoked) return '<span class="muted">—</span>';
  // "Only these Tags" stays chosen while its Tags are being typed (nothing is saved until there is one).
  const t = T.share, id = esc(relay.masterId), limited = Array.isArray(relay.tags) || Boolean(state.grantScope?.has(relay.masterId));
  return '<div class="grant" data-master="' + id + '"><label>' + t.role + '<select data-grant="role">' +
    ["full", "view"].map(role => '<option value="' + role + '"' + (relay.role === role ? " selected" : "") + ">" + t[role] + "</option>").join("") + "</select></label>" +
    '<label>' + t.hosts + '<select data-grant="scope"><option value="all"' + (limited ? "" : " selected") + ">" + t.allHosts + '</option><option value="tags"' + (limited ? " selected" : "") + ">" + t.onlyTags + "</option></select></label>" +
    (limited ? '<label>' + t.tags + '<input data-grant="tags" list="grant-tags" value="' + esc((relay.tags || []).join(", ")) + '" placeholder="web, db" title="' + esc(t.tagsHelp) + '"></label>' : "") +
    '<label class="check-label"><input type="checkbox" data-grant="passThrough"' + (relay.passThrough !== false ? " checked" : "") + ">" + t.passThrough + "</label></div>";
}
let grantQueue = Promise.resolve();
/** Saves one grant control. Only that row's controls are redrawn (and only when the Tags field must appear or go),
 *  so a change made meanwhile on the same row is never drawn over or lost. */
async function saveGrant(control) {
  const box = control.closest(".grant"), masterId = box.dataset.master, kind = control.dataset.grant, params = { masterId };
  const redraw = () => { const cell = $('.grant[data-master="' + CSS.escape(masterId) + '"]')?.parentElement, relay = state.relays.find(row => row.masterId === masterId); if (cell && relay) cell.innerHTML = grantControls(relay); };
  if (kind === "role") params.role = control.value;
  if (kind === "passThrough") params.passThrough = control.checked;
  if (kind === "scope") {
    state.grantScope = state.grantScope || new Set();
    if (control.value === "all") { state.grantScope.delete(masterId); params.tags = null; }
    else { state.grantScope.add(masterId); redraw(); $('.grant[data-master="' + CSS.escape(masterId) + '"] [data-grant="tags"]')?.focus(); return; }
  }
  if (kind === "tags") {
    const tags = control.value.split(",").map(tag => tag.trim()).filter(Boolean);
    if (!tags.length) return;
    params.tags = tags;
  }
  const saved = await rpc("relays.set", params);
  state.relays = state.relays.map(row => row.masterId === masterId ? { ...row, ...saved } : row);
  if (kind === "scope" || kind === "tags") redraw();
  notify(T.share.saved(saved.label || masterId));
}
/* Full-control actions on a host this Master may only view are disabled with the reason, wherever they are drawn. */
const FULL_ACTIONS = ["open-shell", "toggle-new-job", "cancel-job", "ct-action", "mkdir", "upload-pick", "file-delete"];
function applyAccess(root) {
  if (!state.targets.some(viewOnly)) return;
  const page = pageTarget() || target();
  root.querySelectorAll(FULL_ACTIONS.map(action => '[data-action="' + action + '"]:not([data-access])').join(",") + ', #start-job [type="submit"]:not([data-access]), #save-file [type="submit"]:not([data-access])').forEach(button => {
    const item = button.dataset.id && button.dataset.action === "open-shell" ? state.targets.find(row => row.id === button.dataset.id) : page;
    if (!viewOnly(item)) return;
    button.dataset.access = "view"; button.disabled = true; button.setAttribute("data-blocked", ""); button.title = T.share.viewOnlyWhy(accessFrom(item));
  });
}
new MutationObserver(() => applyAccess(document)).observe(document.documentElement, { childList: true, subtree: true });
/** Incoming: allowed Masters and relays joined on the Master ID, one row per Master. */
function renderRelays() {
  const masters = Array.isArray(state.masters) ? state.masters : [], ids = [...new Set([...masters.map(row => row.id), ...state.relays.map(row => row.masterId)])];
  const rows = ids.map(id => ({ id, direct: masters.find(row => row.id === id), relay: state.relays.find(row => row.masterId === id) })), t = mastersText;
  const badges = ({ direct, relay }) =>
    (direct ? '<span class="badge ' + (direct.revoked ? "bad" : "good") + '">' + t.direct + "</span>" + (direct.revoked ? "<small>" + t.revoked + "</small>" : "") : "") +
    (relay ? '<span class="badge ' + (relay.revoked ? "bad" : "good") + '">' + t.throughMerge + "</span>" + (relay.revoked ? "<small>" + t.mergingDisabled + "</small>" : "<small>" + esc(t.lastUsed(timestamp(relay.lastUsedAt))) + "</small>") : "");
  const actions = ({ id, direct, relay }) =>
    (direct && !direct.revoked ? '<button class="danger small" data-action="revoke-master" data-id="' + esc(id) + '">' + t.revoke + "</button>" : "") +
    (relay ? (relay.revoked ? '<button class="small" data-action="allow-relay" data-id="' + esc(id) + '">' + t.allowAgain + "</button>" : '<button class="danger small" data-action="revoke-relay" data-id="' + esc(id) + '">' + t.disableMerging + "</button>") : "");
  return '<section class="surface"><div class="card-head"><div><h2>' + t.inTitle + "</h2><p>" + t.inLead + '</p></div><button data-action="accept-enrollment" title="' + t.allowTitle + '">' + icons.plus + t.allowAnother + "</button></div>" +
    (rows.length ? '<div class="table-wrap"><table><thead><tr><th>' + t.master + "</th><th>" + t.reaches + "</th><th>" + T.share.access + "</th><th></th></tr></thead><tbody>" +
      rows.map(row => '<tr data-relay="' + esc(row.id) + '"><td><strong>' + esc(row.direct?.label || row.relay?.label || row.id) + "</strong><small><code>" + esc(row.id) + '</code></small></td><td><div class="reach-badges">' + badges(row) + "</div></td><td>" + grantControls(row.relay) + '</td><td class="actions"><div class="target-actions">' + actions(row) + "</div></td></tr>").join("") +
      "</tbody></table></div>" : '<div class="empty-inline">' + t.inEmpty + "</div>") +
    '<datalist id="grant-tags">' + (state.tags.tags || []).map(row => '<option value="' + esc(row.tag) + '">').join("") + "</datalist>" +
    '<p class="help mt">' + t.inHelp + "</p></section>";
}
function copyRow(command) {
  return '<div class="copy-row"><code>' + esc(command) + '</code><button class="icon-button" data-action="copy" data-copy="' + esc(command) + '" aria-label="' + esc(T.common.copyCommand(command)) + '" title="' + esc(T.common.copy) + '">' + icons.copy + "</button></div>";
}
function linkBadge(status) {
  const styles = { connected: "good", pathMismatch: "pending", disconnected: "", missingClient: "" };
  return '<span class="badge ' + (styles[status] || "") + '">' + esc(T.service.links[status] || status) + "</span>";
}
function renderClientLink(link) {
  const canConnect = link.detected && link.status === "disconnected";
  const canUpdate = link.detected && link.status === "pathMismatch";
  const t = T.service;
  const actions =
    (canConnect ? '<button class="primary small" data-action="connect-client" data-id="' + esc(link.id) + '">' + t.connect + "</button>" : "") +
    (canUpdate ? '<button class="small" data-action="update-client-path" data-id="' + esc(link.id) + '">' + t.updatePath + "</button>" : "") +
    (link.status === "connected" ? '<span class="help">' + icons.check + t.linkReady + "</span>" : "") +
    (!link.detected ? '<button class="primary small" disabled title="' + esc(t.notInstalledHere) + '">' + t.connect + "</button>" : "");
  // `data-raw`: text from the service, shown as it comes (English).
  return '<article class="client-link surface">' +
    '<div class="card-head"><div><h3>' + esc(link.label) + "</h3><p data-raw>" + esc(link.message) + "</p></div>" + linkBadge(link.status) + "</div>" +
    '<dl class="facts"><dt>' + t.config + "</dt><dd><code>" + esc(link.configPath) + "</code></dd>" +
    (link.command ? "<dt>" + t.current + "</dt><dd><code>" + esc(link.command) + "</code></dd>" : "") +
    "<dt>" + t.expected + "</dt><dd><code>" + esc(link.expectedCommand) + "</code></dd></dl>" +
    '<div class="form-actions">' + actions + "</div>" +
    "<details><summary>" + t.snippet + "</summary><div>" + copyRow(link.snippet) + "</div></details>" +
    "</article>";
}
function renderClientLinks() {
  const t = T.service;
  const body = state.clientLinksError
    ? '<div class="callout bad">' + icons.alert + "<div><strong>" + t.linksFailed + "</strong><p data-raw>" + esc(state.clientLinksError) + "</p></div></div>"
    : (state.clientLinks.length
      ? '<div class="client-link-grid">' + state.clientLinks.map(renderClientLink).join("") + "</div>"
      : '<p class="help">' + t.checkingClients + "</p>");
  return '<section class="section mt"><div class="section-heading"><div><h2>' + t.aiTitle + '</h2><p class="help">' + t.aiLead + "</p></div></div>" +
    body +
    '<details class="mt"><summary>' + t.cli + '</summary><div><p class="help">' + t.cliHelp + "</p>" +
    copyRow("sshcc request system.status") + copyRow("sshcc mcp") + "</div></details></section>";
}
function renderService() {
  const service = state.service, t = T.service;
  const head = heading(t.title, t.lead, '<button data-action="refresh-service">' + icons.refresh + t.again + "</button>");
  if (!service) {
    return head + '<div class="callout bad">' + icons.alert + "<div><strong>" + t.downTitle + "</strong><p>" + t.downText + "</p><ul><li>" + t.downWindows + "</li><li>" + t.downLinux + "<code>systemctl status sshcc</code></li></ul></div></div>" +
      "<details><summary>" + t.error + '</summary><div class="output light"><pre>' + esc(state.serviceError || "—") + "</pre></div></details>" +
      renderClientLinks();
  }
  return head + '<div class="stat-grid">' +
    stat(t.status, '<span class="dot good"></span>' + t.ready) +
    stat(t.hostName, esc(service.label)) +
    stat(t.roles, esc(service.roles?.join(" + ") || "—")) +
    stat(t.uptime, esc(formatUptime(service.uptimeSeconds)), service.version ? t.version(service.version) : "") +
    "</div>" +
    "<div class=\"disclosure-list\"><details><summary>" + t.more + ' <small class="muted">' + t.moreIds + '</small></summary><div><dl class="facts"><dt>' + t.nodeId + "</dt><dd><code>" + esc(service.nodeId) + "</code></dd><dt>" + t.connection + "</dt><dd>" + t.pipe + "</dd><dt>" + t.window + "</dt><dd>" + t.windowText + "</dd></dl></div></details></div>" +
    renderClientLinks();
}
/* Inventory: optional Datacenters → Masters (current or merged) → optional folders → machines. Built from the same
   targets/merges data as the graph; containers are fetched only for the opened machine. */
const dcKey = item => item ? item.id : "local";
function dcModel() {
  const direct = state.targets.filter(item => !item.relayed), byNode = new Map(direct.map(item => [item.nodeId, item]));
  const peers = state.merges.map(merge => ({ merge, peer: direct.find(item => item.id === merge.targetId) })).filter(entry => entry.peer);
  const masters = [{ id: "local", label: state.service?.label || T.inv.thisHostLabel, current: true, item: null, machines: direct.map(item => ({ item })) }];
  for (const { merge, peer } of peers) {
    const machines = (merge.children || []).map(child => byNode.has(child.nodeId)
      ? { item: byNode.get(child.nodeId), alsoDirect: true }
      : { item: state.targets.find(item => item.id === peer.id + "/" + child.id) }).filter(entry => entry.item);
    masters.push({ id: peer.id, label: peer.label || peer.id, current: false, item: peer, merge, twoWay: isTwoWay(merge), machines });
  }
  for (const master of masters.slice(1)) nestMasters(master);
  const members = state.folders.members || {};
  for (const master of masters) {
    master.folders = (state.folders.folders || []).filter(folder => folder.master === master.id)
      .map(folder => ({ ...folder, machines: master.machines.filter(entry => !entry.alsoDirect && members[entry.item.id] === folder.id) }));
    const filed = new Set(master.folders.flatMap(folder => folder.machines.map(entry => entry.item.id)));
    master.loose = master.machines.filter(entry => !filed.has(entry.item.id) || entry.alsoDirect);
  }
  return masters;
}
/** Hosts behind a Master that is itself reached through a Merge nest under it (entry 6), so the tree reads Office →
 *  Branch Master → Lab Master → Lab's hosts. `all` keeps every host below, for counts and the filter. */
function nestMasters(master) {
  const entries = master.machines, below = (entry, other) => other.item.id.startsWith(entry.item.id + "/");
  const top = entries.filter(entry => !entries.some(other => other !== entry && below(other, entry)));
  const parents = top.filter(entry => entries.some(other => below(entry, other)));
  master.all = entries;
  master.subs = parents.map(entry => nestMasters({ id: entry.item.id, label: entry.item.label || entry.item.id, current: false, item: entry.item, nested: true,
    machines: entries.filter(other => below(entry, other)), folders: [] }));
  for (const sub of master.subs) sub.loose = sub.machines;
  master.machines = top.filter(entry => !parents.includes(entry));
  return master;
}
const flatMasters = masters => masters.flatMap(master => [master, ...flatMasters(master.subs || [])]);
function dcLookup(node) {
  const [kind, ...rest] = node.split(":"), masters = dcModel();
  if (kind === "master") { const master = flatMasters(masters).find(row => row.id === rest.join(":")); return master && { kind, master, label: master.label }; }
  if (kind === "site") { const site = (state.datacenters.datacenters || []).find(row => row.id === rest.join(":")); return site && { kind, site, label: site.name, masters: masters.filter(master => siteOf(master.id)?.id === site.id) }; }
  if (kind === "folder") {
    for (const master of masters) {
      const folder = master.folders.find(row => row.id === rest.join(":"));
      if (folder) return { kind, master, folder, label: folder.name };
    }
    return null;
  }
  if (kind === "node") { const item = state.targets.find(row => row.id === rest.join(":")); return item && { kind, item, label: item.label || item.id }; }
  return null;
}
/** Master branch a machine belongs to for folders: "peer" for "peer/child", otherwise this Master. */
const dcBranch = id => id.includes("/") ? id.split("/")[0] : "local";
const siteOf = masterId => (state.datacenters.datacenters || []).find(site => site.id === (state.datacenters.members || {})[masterId]);
const siteIcon = svg('<path d="M2.5 14V4.5l5-2.5v12M7.5 14V6.5l6 2V14M1.5 14h13M4.5 6h1M4.5 8.5h1M4.5 11h1M10 10h1M10 12h1"/>');
const dcFolderOf = id => (state.folders.folders || []).find(folder => folder.id === (state.folders.members || {})[id]);
const dcOnline = entries => entries.filter(entry => nodeState(entry.item) === "online").length;
function dcPath(item, master) {
  if (!item.relayed) return T.inv.sshFrom(state.service?.label || T.inv.thisMasterLower);
  // A host behind several Masters: the whole way, from this Master (entry 6).
  if ((item.path || []).length > 1) return T.share.path([state.service?.label || T.inv.thisMasterLower, ...item.path]);
  return T.inv.through(item.viaLabel || master?.label || item.via);
}
function dcMatches(item) { return hostMatches(item, state.dc.query); }
function dcRow(node, depth, content, options = {}) {
  const selected = state.page === "datacenter" && state.dc.node === node;
  const toggle = options.branch
    ? '<button class="dc-toggle" data-action="dc-toggle" data-node="' + esc(node) + '" aria-label="' + (options.open ? T.inv.collapse : T.inv.expand) + '" aria-expanded="' + options.open + '">' + icons.chevron + "</button>"
    : '<span class="dc-toggle-space"></span>';
  const drop = (options.drop ? ' data-drop="' + esc(options.drop) + '" data-drop-folder="' + esc(options.dropFolder || "") + '"' : "") +
    (options.dropSite ? ' data-drop-site="' + esc(options.dropSite) + '"' : "");
  const drag = options.drag ? ' draggable="true" data-drag="' + esc(options.drag) + '"' : options.dragMaster ? ' draggable="true" data-drag-master="' + esc(options.dragMaster) + '"' : "";
  return '<li class="dc-row' + (selected ? " is-selected" : "") + (options.open ? " is-open" : "") + " depth-" + Math.min(depth, 5) + '"' + drop + ">" + toggle +
    '<button class="dc-item" data-action="dc-select" data-node="' + esc(node) + '"' + drag + (selected ? ' aria-current="true"' : "") + ">" + content + "</button></li>";
}
function dcEditRow(depth, value, label, icon = icons.folder) {
  return '<li class="dc-row is-editing depth-' + Math.min(depth, 5) + '"><span class="dc-toggle-space"></span><span class="dc-icon">' + icon +
    '</span><input id="dc-edit" class="dc-edit" maxlength="64" aria-label="' + esc(label) + '" value="' + esc(value) + '" placeholder="' + esc(label) + '"></li>';
}
function dcMachineRow(entry, depth) {
  const item = entry.item, isMaster = state.merges.some(merge => merge.targetId === item.id);
  const cpu = sampledCpu(item.id), hot = isHot(item);
  const alarm = alarmLevel(item.id);
  return dcRow("node:" + item.id, depth, '<span class="dot ' + (alarm ? "alarm-" + alarm : hot ? "hot" : stateStyles[nodeState(item)] || "") + '"' + (alarm ? ' title="' + esc(T.alarms.level[alarm]) + '"' : "") + '></span><span class="dc-name">' + esc(item.label || item.id) + "</span>" +
    (isMaster ? '<span class="dc-tag">Master</span>' : "") + (viewOnly(item) ? '<span class="dc-tag" title="' + esc(T.share.viewOnlyWhy(accessFrom(item))) + '">' + T.share.viewOnly + "</span>" : "") + upgradeBadge(item, true) + (entry.alsoDirect ? '<span class="dc-tag" title="' + esc(T.inv.alsoDirectTitle) + '">' + T.inv.alsoDirectLower + "</span>" : "") +
    (cpu == null ? '<span class="dc-os">' + esc(osName(item.os)) + "</span>" : '<span class="dc-os' + (hot ? " is-hot" : "") + '" title="' + esc(osName(item.os)) + '">' + Math.round(cpu) + "%</span>"), { drag: entry.alsoDirect ? "" : item.id });
}
function dcMasterRows(master, depth, rows, open) {
  const node = "master:" + master.id, editing = state.dc.editing;
  // Masters start folded so the sidebar opens short; after that the person's choice sticks.
  if (!state.dc.seen.has(node)) { state.dc.seen.add(node); state.dc.collapsed.add(node); }
  const all = master.all || master.machines;
  if (state.dc.query && !all.some(entry => dcMatches(entry.item))) return;
  const tag = master.current ? '<span class="dc-tag is-current">' + T.inv.current + "</span>" : "";
  const status = master.current ? (state.service ? "good" : "bad") : stateStyles[nodeState(master.item)] || "";
  rows.push(dcRow(node, depth, '<span class="dot ' + status + '"></span><span class="dc-name">' + esc(master.label) + "</span>" + tag + (viewOnly(master.item) ? '<span class="dc-tag">' + T.share.viewOnly + "</span>" : "") + '<span class="dc-count">' + dcOnline(all) + "/" + all.length + "</span>",
    { branch: true, open: open(node) || editing?.master === master.id, drop: master.nested ? "" : master.id }));
  if (!open(node) && editing?.master !== master.id) return;
  if (editing?.mode === "new" && editing.master === master.id) rows.push(dcEditRow(depth + 1, "", T.inv.newFolderName));
  for (const folder of master.folders) {
    const folderNode = "folder:" + folder.id, machines = folder.machines.filter(entry => dcMatches(entry.item));
    if (state.dc.query && !machines.length) continue;
    if (editing?.mode === "rename" && editing.id === folder.id) rows.push(dcEditRow(depth + 1, folder.name, T.inv.renameFolder));
    else rows.push(dcRow(folderNode, depth + 1, '<span class="dc-icon">' + icons.folder + '</span><span class="dc-name">' + esc(folder.name) + '</span><span class="dc-count">' + dcOnline(folder.machines) + "/" + folder.machines.length + "</span>",
      { branch: folder.machines.length > 0, open: open(folderNode), drop: master.id, dropFolder: folder.id }));
    if (open(folderNode)) for (const entry of machines) rows.push(dcMachineRow(entry, depth + 2));
  }
  for (const entry of master.loose.filter(entry => dcMatches(entry.item))) rows.push(dcMachineRow(entry, depth + 1));
  for (const sub of master.subs || []) dcMasterRows(sub, depth + 1, rows, open);
}
function renderDcTree() {
  const rows = [], open = node => !state.dc.collapsed.has(node) || Boolean(state.dc.query);
  for (const master of dcModel()) dcMasterRows(master, 0, rows, open);
  return rows.length ? '<ul class="dc-list" role="tree">' + rows.join("") + "</ul>" : '<p class="help tree-empty">' + T.inv.noHostsMatch + "</p>";
}
function dcTabs(tabs) {
  if (!tabs.some(([id]) => id === state.dc.tab)) state.dc.tab = tabs[0][0];
  return '<div class="tabs" role="tablist" aria-label="' + esc(T.inv.sections) + '">' + tabs.map(([id, label]) => '<button role="tab" aria-selected="' + (id === state.dc.tab) + '" class="tab ' + (id === state.dc.tab ? "active" : "") + '" data-action="dc-tab" data-tab="' + id + '">' + label + "</button>").join("") + "</div>";
}
/** Every host in a datacenter: each Master followed by its target hosts, each host once. */
function siteHostTable(masters) {
  const seen = new Set(), rows = [];
  for (const master of masters) {
    const key = master.item ? master.item.id : "local";
    if (!seen.has(key)) { seen.add(key); rows.push(hostRow(master.item)); }
    for (const { item } of master.machines) if (!seen.has(item.id)) { seen.add(item.id); rows.push(hostRow(item)); }
  }
  return '<div class="table-wrap"><table>' + hostHead() + "<tbody>" + rows.join("") + "</tbody></table></div>";
}
function dcMachineTable(entries) {
  if (!entries.length) return '<div class="empty-inline">' + T.inv.noTargets + "</div>";
  return '<div class="table-wrap"><table>' + hostHead() + "<tbody>" + entries.map(({ item }) => hostRow(item)).join("") + "</tbody></table></div>";
}
/** The pinned host key, as a hairline chip beside the status (DESIGN.md: data is crisp). */
function keyChip(item) {
  const key = (item.hostKeys || [])[0];
  if (!key) return "";
  const type = String(key).split(/\s+/).find(part => /^(ssh-|ecdsa-|sk-)/.test(part)) || "ssh";
  const short = type.replace(/^ssh-|^sk-/, "").replace(/^ecdsa-sha2-/, "").replace(/@.*$/, "").toUpperCase();
  return '<span class="key-chip" title="' + esc(T.inv.keyPinned(short)) + '">' + icons.lock + esc(T.inv.keyPinned(short)) + "</span>";
}
/* The Inventory sky (sky.js, DESIGN.md "Relic Night") sits above #main and is only updated, never rebuilt, so a light
   running down a wire survives the re-renders. It shows on the Inventory overview once there are hosts to draw. */
const sky = createSky({ onSelect: id => {
  // Same path as clicking the host's row, so selection behaves exactly as everywhere else.
  const button = document.createElement("button"); button.hidden = true; button.dataset.action = "dc-select"; button.dataset.node = "node:" + id;
  document.body.append(button); button.click(); button.remove();
} });
document.querySelector(".app-main").prepend(sky.el);
window.sshccSky = sky; // for UI tests: sshccSky.paint(6.5) shows sunrise
function skyHost(item) {
  const status = nodeState(item), row = state.health.get(item.id);
  return { id: item.id, label: item.label || item.id, state: status, stateLabel: stateLabel(status) || status, hot: isHot(item), os: osName(item.os),
    address: item.relayed ? "" : item.routes?.[0]?.address || "", checked: row?.checkedAt ? timestamp(row.checkedAt) : "",
    // From the Master's banner check: how fast the host answered, and how long it has stayed online since the Master saw it come up
    response: row?.online && row.latencyMs != null ? T.sky.ms(row.latencyMs) : "",
    online: row?.online && row.onlineSince ? formatUptime(Math.max(60, (Date.now() - Date.parse(row.onlineSince)) / 1000)) : "" };
}
function syncSky() {
  const visible = state.page === "datacenter" && state.dc.node === "dc" && !state.dc.adding && !state.backup && state.targets.length > 0;
  if (!visible) { sky.update({ visible: false }, T); return; }
  const masters = dcModel(), local = masters[0], peers = masters.slice(1), peerIds = new Set(peers.map(master => master.id));
  sky.update({
    visible: true,
    master: { label: local.label, address: state.service?.address || "", version: state.service?.version ? "v" + state.service.version : "" },
    direct: local.machines.filter(entry => !peerIds.has(entry.item.id)).map(entry => skyHost(entry.item)),
    // A host below a merged Master that is itself a Master (peer.subs) stands as a smaller Master pod
    peers: peers.map(peer => { const subs = new Set(peer.subs.map(sub => sub.id)); return { ...skyHost(peer.item), hosts: peer.all.map(entry => ({ ...skyHost(entry.item), master: subs.has(entry.item.id) })) }; }),
    online: hostsOnline(), total: hostCount(), offline: state.targets.filter(item => nodeState(item) === "offline").length,
    checkedAt: state.healthAt ? state.healthAt.toLocaleTimeString(T.locale, { timeStyle: "short" }) : ""
  }, T);
}
/** One line instead of a stats strip: either all is well, or the hosts that need a look grouped by why, with the
 *  first few named (each opens the host) and the rest one click away in the Needs a look tab. */
function renderSummaryLine() {
  const problems = state.targets.filter(item => isHot(item) || ["offline", "unknown", "legacy"].includes(nodeState(item)));
  const checking = state.targets.filter(item => nodeState(item) === "checking").length;
  const when = state.healthError ? '<span class="status-meta error" title="' + esc(state.healthError) + '">' + icons.alert + T.inv.statusFailed + "</span>"
    : '<span class="status-meta">' + (state.healthAt ? esc(T.common.checkedAt(timestamp(state.healthAt))) : T.inv.checking) + "</span>";
  const why = item => isHot(item) ? "hot" : nodeState(item);
  const reason = { hot: T.inv.why.hot, legacy: T.inv.why.legacy }, dot = { offline: "bad", hot: "hot" };
  const groups = ["offline", "hot", "unknown", "legacy"].map(key => [key, problems.filter(item => why(item) === key)]).filter(([, items]) => items.length);
  const group = ([key, items]) => '<div class="sum-group is-' + key + '"><span class="sum-tag"><span class="dot ' + (dot[key] || "pending") + '" aria-hidden="true"></span>' +
    esc(reason[key] || stateLabel(key) || key) + " <b>" + items.length + "</b></span>" +
    items.slice(0, 3).map(item => '<button class="sum-host" data-action="dc-select" data-node="node:' + esc(item.id) + '">' + esc(item.label || item.id) + "</button>").join("") +
    (items.length > 3 ? '<button class="sum-more" data-action="sum-look">' + esc(T.inv.moreHosts(items.length - 3)) + "</button>" : "") + "</div>";
  const body = problems.length
    ? '<span class="summary-icon is-bad" aria-hidden="true">!</span><strong>' + esc(T.inv.needLook(problems.length, hostCount())) + "</strong>"
    : checking ? '<span class="summary-icon" aria-hidden="true">…</span><strong>' + esc(T.inv.checkingN(checking)) + "</strong>"
    : '<span class="summary-icon is-good" aria-hidden="true">✓</span><strong>' + esc(T.inv.allOnline(hostCount())) + "</strong>";
  return '<div id="inv-summary" class="inv-summary" role="status">' + body + when + '<button class="icon-button" data-action="refresh-fleet" aria-label="' + esc(T.inv.checkAgain) + '" title="' + esc(T.inv.checkAgainTitle) + '">' + icons.refresh + "</button>" +
    (groups.length ? '<div class="sum-groups">' + groups.map(group).join("") + "</div>" : "") + "</div>";
}
function renderDcPanel() {
  if (state.backup) return renderBackupFlow();
  if (state.dc.adding) return renderAddHost(state.dc.adding);
  const node = state.dc.node, found = node === "dc" ? null : dcLookup(node);
  if (node !== "dc" && !found) { state.dc.node = "dc"; return renderDcPanel(); }
  if (!found) {
    state.view = "graph";
    return renderSummaryLine() + renderRestoreOffer() + renderUpgradePanel() + renderFleetTools();
  }
  if (found.kind === "master") {
    const master = found.master, target = pageTarget();
    const tabs = dcTabs(hostPageTabs(master.current));
    const actions = target ? terminalButton(target).replace('class="small"', 'class="primary"') : "";
    const t = T.host;
    const head = heading(master.label, (master.current ? t.currentLead : t.mergedLead), actions);
    const needsSelf = '<div class="callout">' + icons.info + "<div><strong>" + t.selfTitle + "</strong><p>" + t.selfText + "</p>" +
      '<div class="form-actions"><button class="primary" data-action="dc-add" data-self="1">' + t.setUp + "</button></div></div></div>";
    const body = state.dc.tab === "summary"
      ? '<div class="stat-grid">' + stat(t.role, master.current ? t.currentMaster : t.mergedMaster) + stat(t.targetHosts, String(master.machines.length)) + stat(t.online, String(dcOnline(master.machines))) + "</div>" +
        '<section class="section mt"><div class="section-heading"><h2>' + t.targetHosts + "</h2></div>" + dcMachineTable(master.machines) + "</section>" +
        (master.current ? renderBackupSection() + renderActivityKeep() : "")
      : state.dc.tab === "activity" && master.current ? renderActivity()
      : state.dc.tab === "jobs" ? (target ? renderJobs(target) : (state.groupRun ? renderGroupRun(state.groupRun) : "") + needsSelf)
      : state.dc.tab === "files" ? (target ? renderFiles(target) : needsSelf)
      : state.dc.tab === "monitoring" ? renderMonitorTab(dcKey(master.item), master.label) : renderContainerTab(dcKey(master.item), master.label);
    return head + tabs + body;
  }
  const t = T.host;
  if (found.kind === "site") {
    const { site, masters } = found, machines = masters.flatMap(master => master.machines);
    return heading(site.name, t.siteLead,
      '<button data-action="site-rename" data-id="' + esc(site.id) + '">' + T.common.rename + '</button><button class="danger" data-action="site-delete" data-id="' + esc(site.id) + '">' + t.deleteSite + "</button>") +
      '<div class="stat-grid">' + stat(t.masters, String(masters.length)) + stat(t.targetHosts, String(machines.length)) + stat(t.online, String(dcOnline(machines))) + "</div>" +
      '<section class="section mt"><div class="section-heading"><h2>' + t.hosts + "</h2></div>" + (masters.length ? siteHostTable(masters)
        : '<div class="empty-inline">' + t.siteEmpty + "</div>") + "</section>";
  }
  if (found.kind === "folder") {
    const { folder, master } = found;
    const actions = '<button data-action="folder-rename" data-id="' + esc(folder.id) + '">' + T.common.rename + '</button><button class="danger" data-action="folder-delete" data-id="' + esc(folder.id) + '">' + t.deleteFolder + "</button>";
    return heading(folder.name, t.folderLead(master.label), actions) +
      '<div class="stat-grid">' + stat(t.master, esc(master.label)) + stat(t.targetHosts, String(folder.machines.length)) + stat(t.online, String(dcOnline(folder.machines))) + "</div>" +
      '<section class="section mt">' + (folder.machines.length ? dcMachineTable(folder.machines) : '<div class="empty-inline">' + t.folderEmpty + "</div>") + "</section>";
  }
  const item = found.item;
  const head = '<div class="page-heading"><div class="target-title"><span class="live-label">● ' + esc(dcPath(item)) + "</span><h1>" + esc(item.label || item.id) + '</h1><div class="meta-row">' + statusBadge(item) + (item.os ? chip(osName(item.os)) : "") + keyChip(item) + "</div></div>" +
    (upgradeRow(item.id)?.outdated ? '<div class="meta-row upgrade-row">' + upgradeBadge(item) + upgradeControls(item) + "</div>" : "") +
    (viewOnly(item) ? '<div class="meta-row">' + accessBadge(item) + '<small class="muted">' + esc(T.share.rightView(accessFrom(item))) + "</small></div>" : "") +
    '<div class="target-actions"><button class="primary" data-action="open-shell" data-id="' + esc(item.id) + '"' + (!item.runnerInstalled ? ' disabled title="' + esc(T.inv.installFirst) + '"' : "") + ">" + icons.shell + t.openTerminal + '</button><button data-action="diagnose">' + icons.pulse + t.checkConnection + "</button></div></div>" +
    (!item.runnerInstalled ? '<div class="callout pending">' + icons.alert + "<div><strong>" + t.v1Title + "</strong><p>" + t.v1Text + "</p></div></div>" : "");
  const tabs = '<div id="check-slot">' + renderCheckPanel(item) + "</div>" + dcTabs(hostPageTabs());
  const body = state.dc.tab === "summary" ? renderOverview(item, renderTagRow(item) + renderHostAlarms(item)) : state.dc.tab === "jobs" ? renderJobs(item) : state.dc.tab === "files" ? renderFiles(item)
    : state.dc.tab === "monitoring" ? renderMonitorTab(item.id, item.label || item.id) : renderContainerTab(item.id, item.label || item.id);
  return head + tabs + body;
}
/* Backup and restore (roadmap entry 3). The section sits on the current Master's Summary tab; each action opens a
   one-column flow in the same step layout as Add host. A new passphrase stays in its fields; a restore keeps the one that
   opened the file only while the flow is open (closing or finishing the flow drops it). */
const MIN_PASSPHRASE = 12;
async function refreshBackupStatus() { state.backupStatus = await rpc("backup.status").catch(() => state.backupStatus); }
function backupLastLine() {
  const t = T.backup, status = state.backupStatus;
  if (status?.backupBeforeKeys) return t.sinceKeys;
  return status?.lastBackupAt ? t.last(timestamp(status.lastBackupAt)) : t.never;
}
function renderBackupSection() {
  const t = T.backup, status = state.backupStatus, owner = status?.owner !== false, empty = status?.empty === true;
  return '<section class="section mt" id="backup-section"><div class="section-heading"><div><h2>' + t.title + '</h2><p id="backup-last">' + esc(backupLastLine()) + "</p></div></div>" +
    '<p class="help">' + t.lead + "</p>" +
    '<div class="form-actions"><button data-action="backup-start"' + (owner ? "" : ' disabled data-blocked title="' + esc(t.notOwner) + '"') + ">" + t.backUp + "</button>" +
    '<button data-action="restore-start"' + (empty ? "" : " disabled data-blocked") + ">" + t.restore + "</button>" +
    (keysPending() ? '<button data-action="keys-change">' + t.changeKeysLater + "</button>" : "") + "</div>" +
    (owner ? "" : '<p class="help mt">' + t.notOwner + "</p>") + (empty ? "" : '<p class="help mt">' + t.restoreOnlyEmpty + "</p>") +
    (keysPending() ? '<p class="help mt">' + t.keysPending + "</p>" : "") +
    (state.rotation?.active ? '<div class="mt">' + renderKeysProgress() + "</div>"
      : status?.backupBeforeKeys ? '<div class="callout pending mt" id="keys-progress">' + icons.alert + "<div><strong>" + t.changed + "</strong><p>" + t.oldBackups + "</p></div></div>" : "") +
    "</section>";
}
/** Restored, and the keys were not changed since (skipped, or never finished). */
function keysPending() {
  const status = state.backupStatus;
  return Boolean(status?.restoredAt && !state.rotation?.active && !(status.keysChangedAt && status.keysChangedAt > status.restoredAt));
}
/** One step card, as in the Add host column: the current step has aria-current and the acid rule. */
function flowCard(n, now, title, body) {
  const status = n === now ? "is-now" : n < now ? "is-done" : "is-later";
  return '<section class="add-step ' + status + '" data-step="' + n + '"' + (status === "is-now" ? ' aria-current="step"' : "") + '><div class="add-n">' + (status === "is-done" ? "✓" : n) + "</div>" +
    '<div class="add-body"><h2>' + title + "</h2>" + body + "</div></section>";
}
/** Moves the current-step marking without a re-render, so typed passphrases and the caret stay put. */
function setFlowStep(root, now) {
  for (const card of root?.querySelectorAll(".add-step[data-step]") || []) {
    const n = Number(card.dataset.step), status = n === now ? "is-now" : n < now ? "is-done" : "is-later";
    card.classList.remove("is-now", "is-done", "is-later"); card.classList.add(status);
    if (status === "is-now") card.setAttribute("aria-current", "step"); else card.removeAttribute("aria-current");
    card.querySelector(".add-n").textContent = status === "is-done" ? "✓" : String(n);
  }
}
function renderBackupFlow() {
  const t = T.backup, flow = state.backup;
  if (flow.mode === "restore") return renderRestoreFlow();
  const now = flow.saved ? 3 : flow.valid ? 2 : 1;
  const close = '<button data-action="backup-close">' + icons.close + t.close + "</button>";
  const pass = '<form id="backup-pass-form" class="add-body" autocomplete="off">' +
    '<label>' + t.pass + '<input name="pass" type="password" autocomplete="new-password" spellcheck="false" data-focus' + (flow.saved ? " disabled" : "") + "></label>" +
    '<label>' + t.again + '<input name="again" type="password" autocomplete="new-password" spellcheck="false"' + (flow.saved ? " disabled" : "") + "></label>" +
    '<p class="add-check" id="backup-check" aria-live="polite">' + esc(t.passIdle(MIN_PASSPHRASE)) + "</p>" +
    '<div class="callout pending">' + icons.alert + "<div><strong>" + t.keyTitle + "</strong><p>" + t.keyFull + "</p><p>" + t.keyLost + "</p></div></div></form>";
  const save = flow.saved ? '<p class="add-check">' + icons.check + esc(t.saved) + "</p>"
    : '<div class="form-actions"><button class="primary" id="backup-save" data-action="backup-save"' + (flow.valid ? "" : " disabled") + ">" + t.save + "</button></div>" +
      (flow.error ? '<p class="add-check bad" role="alert">' + errorHtml(explain(flow.error)) + "</p>" : "");
  const done = flow.saved ? '<p class="wrap">' + esc(t.savedTo(flow.saved)) + '</p><p class="help">' + t.keepAway + '</p><div class="form-actions"><button class="primary" data-action="backup-close">' + t.done + "</button></div>" : "";
  return '<div id="backup-flow" class="add-flow">' + heading(t.backupTitle, t.backupLead, close) +
    '<div class="add-column">' + flowCard(1, now, t.backupSteps[0], pass) + flowCard(2, now, t.backupSteps[1], save) + flowCard(3, now, t.backupSteps[2], done) + "</div></div>";
}
/** The line under the passphrase fields, the Save button and the current step, from what is typed. */
function onBackupInput(form) {
  const t = T.backup, pass = form.pass.value, again = form.again.value, short = [...pass].length < MIN_PASSPHRASE;
  state.backup.valid = !short && pass === again;
  const [text, tone] = !pass ? [t.passIdle(MIN_PASSPHRASE), ""] : short ? [t.short(MIN_PASSPHRASE), again ? "bad" : ""]
    : !again ? [t.passIdle(MIN_PASSPHRASE), ""] : pass !== again ? [t.mismatch, "bad"] : [t.match, "good"];
  const line = $("#backup-check");
  line.textContent = text; line.className = "add-check" + (tone ? " " + tone : "");
  $("#backup-save").disabled = !state.backup.valid;
  setFlowStep($("#backup-flow"), state.backup.valid ? 2 : 1);
}
/** On an empty Master's Inventory: restore a backup, next to adding hosts one by one. */
function renderRestoreOffer() {
  if (state.backupStatus?.empty !== true || state.targets.length) return "";
  return '<div class="callout" id="restore-offer">' + icons.info + "<div><strong>" + T.inv.restoreTitle + "</strong><p>" + T.inv.restoreText + "</p>" +
    '<div class="form-actions"><button class="primary" data-action="restore-start">' + T.backup.restore + '</button><button data-action="dc-add">' + T.side.addHost + "</button></div></div></div>";
}
function renderRestoreFlow() {
  const t = T.backup, flow = state.backup, summary = flow.summary, result = flow.result;
  const now = result ? 4 : summary ? 3 : flow.file ? 2 : 1;
  const error = n => flow.error && flow.errorStep === n ? '<p class="add-check bad" role="alert">' + errorHtml(explain(flow.error)) + "</p>" : "";
  const choose = (flow.file ? '<p class="wrap">' + esc(t.chosen(flow.file.name)) + "</p>" : "") +
    (result ? "" : '<div class="form-actions"><button' + (flow.file ? "" : ' class="primary" data-focus') + ' data-action="restore-choose">' + (flow.file ? t.chooseAgain : t.choose) + "</button></div>") + error(1);
  const pass = summary ? '<p class="add-check">' + icons.check + esc(t.opened(summary.label)) + "</p>"
    : flow.file ? '<form id="restore-pass-form" class="add-body" autocomplete="off"><label>' + t.passOpen +
      '<input name="pass" type="password" autocomplete="current-password" spellcheck="false" data-focus></label>' +
      '<div class="form-actions"><button type="submit" class="primary">' + t.unlock + "</button></div>" + error(2) + "</form>" : "";
  const facts = summary ? '<dl class="facts"><dt>' + t.summaryMaster + "</dt><dd>" + esc(summary.label) + "</dd><dt>" + t.summaryDate + "</dt><dd>" + esc(timestamp(summary.createdAt)) +
    "</dd><dt>" + t.summaryHosts + "</dt><dd>" + esc(summary.hosts) + "</dd><dt>" + t.summaryMerges + "</dt><dd>" + esc(summary.merges) + "</dd><dt>" + t.summaryFolders + "</dt><dd>" + esc(summary.folders) + "</dd></dl>" +
    (result ? "" : '<div class="callout pending">' + icons.alert + "<div><strong>" + t.offTitle + "</strong><p>" + t.offText + "</p></div></div>" +
      '<label class="check-label is-body"><input type="checkbox" id="restore-off" data-focus> ' + t.oldOff + "</label>" +
      '<div class="form-actions"><button class="primary" id="restore-run" data-action="restore-run" disabled>' + t.restoreNow + "</button></div>" + error(3)) : "";
  const done = !result ? "" : flow.restarting ? '<p class="add-check" aria-live="polite">' + esc(t.restarting) + "</p>"
    : '<p class="add-check good" aria-live="polite">' + icons.check + esc(t.restored(result.label, result.hosts)) + "</p>" + renderRestoreFollowUps(result) + renderKeysChoice();
  const close = result ? "" : '<button data-action="backup-close">' + icons.close + t.close + "</button>";
  return '<div id="backup-flow" class="add-flow">' + heading(t.restoreTitle, t.restoreLead, close) + '<div class="add-column">' +
    flowCard(1, now, t.restoreSteps[0], choose) + flowCard(2, now, t.restoreSteps[1], pass) + flowCard(3, now, t.restoreSteps[2], facts) + flowCard(4, now, t.restoreSteps[3], done) + "</div></div>";
}
/** After a restore: other Masters still using the old address, two-way Merges to enable again, jump keys left behind. */
function renderRestoreFollowUps(result) {
  const t = T.backup, reached = result.reachedBy || [], twoWay = result.twoWayAgain || [], paths = result.checkPaths || [];
  if (!reached.length && !twoWay.length && !paths.length) return "";
  const list = rows => '<ul class="follow-ups">' + rows.join("") + "</ul>";
  const two = row => { const merge = state.merges.find(item => item.targetId === row.targetId); return merge ? twoWayButton(merge, mastersText.enableTwoWay, "small") : ""; };
  return '<div class="callout pending" id="restore-follow-ups">' + icons.alert + "<div><strong>" + t.afterTitle + "</strong>" +
    (reached.length ? "<p>" + t.reachedHelp + "</p>" + list(reached.map(row => "<li>" + esc(t.updateAddress(row.label || row.masterId)) + "</li>")) : "") +
    (twoWay.length ? "<p><strong>" + t.twoWayAgain + "</strong>" + t.twoWayHelp + "</p>" + list(twoWay.map(row => '<li data-two-way="' + esc(row.targetId) + '"><span>' + esc(row.label || row.targetId) + "</span> " + two(row) + "</li>")) : "") +
    (paths.length ? "<p>" + t.pathsHelp + "</p>" + list(paths.map(row => "<li>" + esc(t.checkPath(row.label || row.targetId)) + " <code>" + esc(row.path) + "</code></li>")) : "") +
    "</div></div>";
}
/** Change keys after a restore: the choice, then live progress, then "back up now". */
function renderKeysChoice() {
  const t = T.backup, rotation = state.rotation;
  const inventory = '<button data-action="backup-close">' + t.openInventory + "</button>";
  if (!rotation?.startedAt || (!rotation.active && !state.backup?.rotating)) {
    return '<div class="callout good" id="keys-choice">' + icons.info + "<div><strong>" + t.keysTitle + "</strong><p>" + t.keysText + "</p><p>" + t.keysOffline + "</p>" +
      '<div class="form-actions"><button class="primary" data-action="keys-change" data-focus>' + t.changeKeys + '</button><button data-action="backup-close">' + t.skipKeys + "</button></div></div></div>";
  }
  return renderKeysProgress() + '<div class="form-actions">' + (rotation.active ? "" : '<button class="primary" data-action="backup-start">' + t.backUp + "</button>") + inventory + "</div>";
}
/** Where a change of keys stands: how many hosts still accept the old key, or done with "back up now". */
function renderKeysProgress() {
  const t = T.backup, rotation = state.rotation;
  if (!rotation?.startedAt) return "";
  if (!rotation.active) return '<div class="callout good" id="keys-progress">' + icons.check + "<div><strong>" + t.changed + "</strong><p>" + t.oldBackups + "</p></div></div>";
  const trouble = (rotation.hosts || []).filter(host => host.state !== "moved" && host.error);
  const merged = rotation.merged || [];
  return '<div class="callout" id="keys-progress" aria-live="polite">' + icons.info + "<div><strong>" + t.changing + "</strong><p>" + esc(t.progress(rotation.moved, rotation.total)) + "</p>" +
    '<p class="keys-old">' + esc(t.stillOld(rotation.oldKey)) + "</p>" +
    (trouble.length ? '<ul class="follow-ups keys-trouble">' + trouble.map(host => '<li data-host="' + esc(host.targetId) + '">' + troubleLine(host) + "</li>").join("") + "</ul><p class=\"help\">" + t.retrying + "</p>" : "") +
    (merged.length ? '<ul class="follow-ups keys-merged">' + merged.map(row => "<li>" + esc(t.mergedBy(row.label, row.master)) + "</li>").join("") + "</ul>" : "") +
    "</div></div>";
}
/** A host the change could not move yet: the plain sentence for a known error (it names the host), else the host
 *  name and the Master's English text. */
function troubleLine(host) {
  const name = host.label || host.targetId, shown = explain(host.error, false, name);
  return (shown.text === String(host.error) ? "<strong>" + esc(name) + "</strong> · " : "") + errorHtml(shown);
}
let rotationTimer = 0;
/** Polls keys.rotation_status while a change of keys runs and something on screen shows it. */
async function watchRotation() {
  clearTimeout(rotationTimer); rotationTimer = 0;
  state.rotation = await rpc("keys.rotation_status").catch(() => state.rotation);
  if (!state.rotation?.active) await refreshBackupStatus();
  if ((state.backup?.mode === "restore" && $("#backup-flow")) || $("#backup-section")) paintFlow();
  if (state.rotation?.active && $("#keys-progress")) rotationTimer = setTimeout(() => void watchRotation(), 2000);
}
async function changeKeys() {
  state.rotation = await rpc("keys.rotate");
  if (state.backup) state.backup.rotating = true;
  paintFlow();
  void watchRotation();
}
/** Re-renders the flow keeping what is typed, then focuses the current step's main control. */
function paintFlow() {
  const restore = keepInput($("#main")); render(); restore();
  $('#backup-flow [aria-current="step"] [data-focus]')?.focus();
}
async function chooseBackup() {
  const flow = state.backup;
  try {
    const picked = await native("open_backup", { title: T.dialogs.openBackup });
    if (!picked) return;
    Object.assign(flow, { file: picked, summary: null, passphrase: "", error: "", errorStep: 0 });
  } catch (error) { Object.assign(flow, { error: String(error?.message || error), errorStep: 1 }); }
  render(); $('#backup-flow [aria-current="step"] [data-focus]')?.focus();
}
async function openBackup(form) {
  const flow = state.backup;
  try {
    flow.summary = await rpc("backup.inspect", { file: flow.file.file, passphrase: form.pass.value });
    Object.assign(flow, { passphrase: form.pass.value, error: "", errorStep: 0 });
  } catch (error) { Object.assign(flow, { error: String(error?.message || error), errorStep: 2 }); }
  paintFlow();
  if (flow.errorStep === 2) $("#restore-pass-form [name=pass]")?.select();
}
/** Restore, then wait for the service to come back as the restored Master (it restarts to load it). */
async function runRestore() {
  const flow = state.backup;
  try {
    flow.result = await rpc("backup.restore", { file: flow.file.file, passphrase: flow.passphrase });
    state.rotation = null;
    Object.assign(flow, { error: "", errorStep: 0, restarting: true });
  } catch (error) { Object.assign(flow, { error: String(error?.message || error), errorStep: 3 }); paintFlow(); return; }
  delete flow.file.file; delete flow.passphrase; // not needed any more
  paintFlow();
  for (const until = Date.now() + 120000; Date.now() < until;) {
    const status = await rpc("system.status").catch(() => null);
    if (status?.nodeId === flow.result.nodeId) break;
    await new Promise(done => setTimeout(done, 1000));
  }
  flow.restarting = false;
  await refreshLocal();
  try { await refreshFleet(); } catch (error) { notify(error?.message || error, true); }
  if (state.backup === flow) paintFlow(); else render();
}
/** Today's date as YYYY-MM-DD and the Master name as a file-name part, for the default file name. */
const fileDate = () => { const d = new Date(), two = n => String(n).padStart(2, "0"); return d.getFullYear() + "-" + two(d.getMonth() + 1) + "-" + two(d.getDate()); };
const fileNamePart = text => String(text || "master").normalize("NFKC").replace(/[\\/:*?"<>|\s]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 60) || "master";
async function saveBackup() {
  const form = $("#backup-pass-form");
  onBackupInput(form);
  if (!state.backup.valid) return;
  state.backup.error = "";
  try {
    const path = await native("save_backup", { passphrase: form.pass.value, name: T.backup.fileName(fileNamePart(state.service?.label), fileDate()), title: T.dialogs.saveBackup });
    if (!path) return;
    state.backup.saved = path;
  } catch (error) { state.backup.error = String(error?.message || error); }
  await refreshBackupStatus();
  const restore = keepInput($("#main")); render(); restore();
  $('#backup-flow [aria-current="step"] button.primary')?.focus();
}
function ctBadge(status) {
  const styles = { running: "good", exited: "", created: "pending", paused: "pending", restarting: "running", dead: "bad", removing: "pending" };
  return '<span class="badge ' + (styles[status] || "") + '">' + esc(T.containers.states[status] || status || T.containers.unknown) + "</span>";
}
function renderContainerTab(key, label) {
  const entry = state.containers.get(key), data = entry?.data, t = T.containers;
  const view = '<div class="segmented" role="group" aria-label="' + esc(t.show) + '">' + [["containers", t.containers], ["images", t.images]].map(([id, text]) => '<button data-action="ct-view" data-view="' + id + '" aria-pressed="' + (state.dc.ctView === id) + '">' + text + "</button>").join("") + "</div>";
  const bar = '<div class="toolbar">' + view + (state.dc.ctView === "containers"
    ? '<div class="segmented" role="group" aria-label="' + esc(t.filter) + '">' + [["running", t.running], ["all", t.all]].map(([id, text]) => '<button data-action="ct-filter" data-filter="' + id + '" aria-pressed="' + (state.dc.filter === id) + '">' + text + "</button>").join("") + "</div>"
    : "") + (data?.runtime ? chip(data.runtime) : "") + '<span class="status-meta">' + (entry?.loading ? t.loading : entry?.at ? esc(t.updated(timestamp(entry.at))) : "") + "</span>" +
    '<button data-action="ct-refresh" data-key="' + esc(key) + '">' + icons.refresh + T.common.refresh + "</button></div>";
  if (!entry || (entry.loading && !data)) return bar + '<div class="empty-inline">' + esc(t.reading(label)) + "</div>";
  if (entry.error) return bar + '<div class="callout bad">' + icons.alert + "<div><strong>" + t.failed + "</strong><p data-raw>" + esc(entry.error) + "</p></div></div>";
  if (!data.runtime) return bar + '<div class="callout">' + icons.info + "<div><strong>" + t.noRuntime + "</strong><p>" + esc(t.noRuntimeText(label)) + "</p></div></div>";
  if (data.error) return bar + '<div class="callout bad">' + icons.alert + "<div><strong>" + esc(t.notAnswering(data.runtime)) + "</strong><p data-raw>" + esc(data.error) + "</p></div></div>";
  const note = data.truncated ? '<p class="help">' + t.first500 + "</p>" : "";
  if (state.dc.ctView === "images") {
    if (!data.images.length) return bar + '<div class="empty-inline">' + t.noImages + "</div>";
    const h = t.imageHead;
    return bar + '<div class="table-wrap"><table><thead><tr><th>' + h.repository + "</th><th>" + h.tag + "</th><th>" + h.id + "</th><th>" + h.size + "</th><th>" + h.created + "</th></tr></thead><tbody>" +
      data.images.map(image => "<tr><td><code>" + esc(image.repository) + "</code></td><td>" + chip(image.tag) + "</td><td><code>" + esc(String(image.id).replace(/^sha256:/, "").slice(0, 12)) + '</code></td><td class="num">' + esc(image.size || formatSize(image.sizeBytes)) + '</td><td class="muted" data-raw>' + esc(image.created) + "</td></tr>").join("") +
      "</tbody></table></div>" + note;
  }
  const running = data.containers.filter(row => row.state === "running").length;
  const rows = state.dc.filter === "running" ? data.containers.filter(row => row.state === "running") : data.containers;
  const summary = '<p class="info-line">' + esc(t.summary(running, data.containers.length, data.images.length)) + "</p>";
  const h = t.head;
  const table = rows.length
    ? '<div class="table-wrap"><table><thead><tr><th>' + h.container + "</th><th>" + h.state + "</th><th>" + h.status + "</th><th>" + h.ports + "</th><th></th></tr></thead><tbody>" +
      rows.map(row => {
        const ref = esc(row.name || row.id), on = row.state === "running";
        const act = (action, text, danger) => '<button class="small' + (danger ? " danger" : "") + '" data-action="ct-action" data-key="' + esc(key) + '" data-container="' + ref + '" data-verb="' + action + '">' + text + "</button>";
        // Image sits under the name so long references never squeeze a column of their own.
        return '<tr><td class="ct-name"><strong title="' + esc(row.name) + '">' + esc(row.name) + '</strong><small class="ct-image" title="' + esc(row.image) + '"><code>' + esc(row.image) + "</code></small><small><code>" + esc(String(row.id).slice(0, 12)) + "</code></small></td><td>" + ctBadge(row.state) + '</td><td class="muted ct-status" data-raw>' + esc(row.status) + '</td><td class="wrap"><code>' + esc(row.ports || "—") + "</code></td>" +
          '<td class="actions"><div class="target-actions"><button class="small" data-action="ct-logs" data-key="' + esc(key) + '" data-container="' + ref + '">' + t.logs + "</button>" +
          (on ? act("restart", t.restart) + act("stop", t.stop, true) : act("start", t.start)) + "</div></td></tr>";
      }).join("") + "</tbody></table></div>"
    : '<div class="empty-inline">' + (state.dc.filter === "running" ? t.noRunning : t.none) + "</div>";
  const logs = state.ctLogs?.key === key
    ? '<div class="console mt"><div class="console-bar"><strong>' + esc(t.logsOf(state.ctLogs.container)) + "</strong><small>" + t.last200 + "</small>" +
      '<button class="small" data-action="ct-logs" data-key="' + esc(key) + '" data-container="' + esc(state.ctLogs.container) + '">' + icons.refresh + t.reload + '</button><button class="small" data-action="ct-logs-close">' + T.common.close + "</button></div>" +
      '<div class="output" tabindex="0" aria-label="' + esc(t.logsLabel) + '"><pre>' + esc(state.ctLogs.loading ? t.loading : state.ctLogs.error || state.ctLogs.content || t.noLog) + "</pre></div></div>"
    : "";
  return bar + summary + table + note + logs;
}
/* Monitoring: each poll is one bridge call returning counters; CPU % and network rates come from the
   difference between consecutive samples. Polling runs only while this tab is on screen and focused. */
const MONITOR_SAMPLES = 120;
function visibleMonitorKey() {
  if (state.page !== "datacenter" || state.dc.adding || state.dc.tab !== "monitoring") return null;
  const found = state.dc.node === "dc" ? null : dcLookup(state.dc.node);
  if (found?.kind === "master") return dcKey(found.master.item);
  if (found?.kind === "node") return found.item.id;
  return null;
}
function monitorSeries(samples) {
  const rows = [];
  for (let i = 1; i < samples.length; i++) {
    const a = samples[i - 1], b = samples[i], seconds = Math.max(0.001, (b.at - a.at) / 1000);
    const total = b.cpu.total - a.cpu.total, idle = b.cpu.idle - a.cpu.idle;
    rows.push({
      at: b.at,
      cpu: total > 0 ? Math.min(100, Math.max(0, 100 * (1 - idle / total))) : null,
      memory: b.memory.total ? 100 * (1 - b.memory.available / b.memory.total) : null,
      rx: a.net && b.net ? Math.max(0, (b.net.rx - a.net.rx) / seconds) : null,
      tx: a.net && b.net ? Math.max(0, (b.net.tx - a.net.tx) / seconds) : null,
      load: b.load?.length ? b.load[0] : null
    });
  }
  return rows;
}
const formatRate = value => value == null ? "—" : formatSize(value) + "/s";
const clock = at => new Date(at).toLocaleTimeString(T.locale);
/** One-axis sparkline: thin line, faint area, endpoint marker, a native tooltip per sample. */
function sparkline(rows, series, max) {
  const w = 300, h = 56, n = rows.length;
  if (n < 2) return '<div class="spark-empty">' + T.monitor.collecting + "</div>";
  const top = max ?? Math.max(1, ...rows.flatMap(row => series.map(s => row[s.key] ?? 0)));
  const x = i => (i / (n - 1)) * w, y = v => h - 2 - (Math.min(v, top) / top) * (h - 6);
  const lines = series.map((s, index) => {
    const cls = " series-" + (index + 1);
    const points = rows.map((row, i) => row[s.key] == null ? null : [x(i), y(row[s.key])]).filter(Boolean);
    if (points.length < 2) return "";
    const d = points.map((p, i) => (i ? "L" : "M") + p[0].toFixed(1) + " " + p[1].toFixed(1)).join(" ");
    const last = points[points.length - 1];
    return (series.length === 1 ? '<path class="spark-area" d="' + d + " L" + last[0].toFixed(1) + " " + h + " L" + points[0][0].toFixed(1) + " " + h + ' Z"/>' : "") +
      '<path class="spark-line' + cls + '" d="' + d + '"/><circle class="spark-dot' + cls + '" cx="' + last[0].toFixed(1) + '" cy="' + last[1].toFixed(1) + '" r="3"/>';
  }).join("");
  const hits = rows.map((row, i) => '<rect class="spark-hit" x="' + (x(i) - w / (2 * (n - 1))).toFixed(1) + '" y="0" width="' + (w / (n - 1)).toFixed(1) + '" height="' + h + '"><title>' +
    esc(clock(row.at) + " · " + series.map(s => s.label + " " + s.format(row[s.key])).join(" · ")) + "</title></rect>").join("");
  return '<svg class="spark" viewBox="0 0 ' + w + " " + h + '" preserveAspectRatio="none" role="img" aria-label="' + esc(T.monitor.over(series.map(s => s.label).join(T.monitor.and))) + '">' +
    '<line class="spark-grid" x1="0" x2="' + w + '" y1="' + (h - 2) + '" y2="' + (h - 2) + '"/>' + lines + hits + "</svg>";
}
function meter(percent) {
  const value = Math.min(100, Math.max(0, percent));
  return '<svg class="meter" viewBox="0 0 100 10" preserveAspectRatio="none" role="meter" aria-valuemin="0" aria-valuemax="100" aria-valuenow="' + Math.round(value) + '"><rect class="meter-track" width="100" height="10"/><rect class="meter-fill' + (value >= HOT ? " is-hot" : "") + '" width="' + value.toFixed(1) + '" height="10"/></svg>';
}
function renderMonitorBody(key) {
  const entry = state.monitor.get(key), t = T.monitor;
  if (!entry || (!entry.samples.length && !entry.error)) return '<div class="empty-inline">' + t.reading + "</div>";
  if (!entry.samples.length) return '<div class="callout bad">' + icons.alert + "<div><strong>" + t.failed + "</strong><p data-raw>" + esc(entry.error) + "</p></div></div>";
  const latest = entry.samples[entry.samples.length - 1], rows = monitorSeries(entry.samples), now = rows[rows.length - 1] || {};
  const pct = value => value == null ? "—" : value.toFixed(value < 10 ? 1 : 0) + "%";
  const memUsed = latest.memory.total - latest.memory.available;
  // Every partition (hosts from entry 8 on); the tile shows the fullest. Older hosts report only / or C:\.
  const parts = (latest.disks || []).filter(d => d.total > 0), used = d => d.total - d.free;
  const disk = parts.length ? parts.reduce((a, b) => used(b) / b.total > used(a) / a.total ? b : a) : latest.disk, diskUsed = disk ? used(disk) : 0;
  // Six tiles of one shape (label · big value · one line · chart of one height), three by two, so the page reads evenly.
  const tile = (title, value, sub, chart, hint = "") => '<section class="mon-tile"' + (hint ? ' title="' + esc(hint) + '"' : "") + '><span class="stat-label">' + title + (hint ? ' <span class="mon-hint" aria-hidden="true">?</span>' : "") + "</span>" +
    '<span class="mon-value">' + value + '</span><small class="mon-sub">' + (sub || "&nbsp;") + '</small><div class="mon-chart">' + (chart || "") + "</div></section>";
  const cores = latest.cpu.count, load = latest.load;
  const net = (key, label) => latest.net ? sparkline(rows, [{ key, label, format: formatRate }]) : "";
  const tiles = [
    tile(t.cpu, pct(now.cpu), esc(t.cores(cores)), sparkline(rows, [{ key: "cpu", label: t.cpu, format: pct }], 100)),
    tile(t.memory, pct(now.memory ?? 100 * memUsed / latest.memory.total), esc(t.of(formatSize(memUsed), formatSize(latest.memory.total))) + (latest.swap?.total ? esc(t.swap(formatSize(latest.swap.total - latest.swap.free))) : ""),
      sparkline(rows, [{ key: "memory", label: t.memory, format: pct }], 100)),
    tile(esc(t.disk(disk?.path || "")), disk ? pct(100 * diskUsed / disk.total) : "—", disk ? esc(t.free(formatSize(diskUsed), formatSize(disk.total), formatSize(disk.free))) : t.notReported, disk ? meter(100 * diskUsed / disk.total) : ""),
    tile(t.download, latest.net ? esc(formatRate(now.rx)) : "—", latest.net ? t.received : t.notOnOs, net("rx", t.downloadLabel)),
    tile(t.upload, latest.net ? esc(formatRate(now.tx)) : "—", latest.net ? t.sent : t.notOnOs, net("tx", t.uploadLabel)),
    tile(t.load, load?.length ? load[0].toFixed(2) : "—", load?.length ? esc(t.loadSub(cores, load[1].toFixed(2), load[2].toFixed(2))) : t.notOnWindows,
      load?.length ? sparkline(rows, [{ key: "load", label: t.loadLabel, format: v => v == null ? "—" : v.toFixed(2) }], Math.max(cores, ...rows.map(row => row.load ?? 0))) : "",
      t.loadHint(cores))
  ];
  const partitions = parts.length ? '<section class="section partitions" id="partitions"><div class="section-heading"><h2>' + t.partitions + "</h2>" + (parts.length > 1 ? "<small class=\"muted\">" + t.fullest + "</small>" : "") + '</div><div class="table-wrap"><table class="partition-table"><thead><tr><th>' + t.partHead.path + "</th><th>" + t.partHead.fs + "</th><th>" + t.partHead.used + "</th><th>" + t.partHead.size + "</th><th></th></tr></thead><tbody>" +
    parts.map(d => '<tr data-partition="' + esc(d.path) + '"><td class="mono">' + esc(d.path) + '</td><td class="mono">' + esc(d.fs || "—") + "</td><td>" + pct(100 * used(d) / d.total) + "</td><td>" + esc(formatSize(d.total)) + '</td><td class="meter-cell">' + meter(100 * used(d) / d.total) + "</td></tr>").join("") + "</tbody></table></div></section>"
    : latest.disk ? '<p class="help mt" id="partitions-hint">' + t.updateForPartitions + "</p>" : "";
  return (entry.error ? '<div class="callout bad">' + icons.alert + "<div><strong>" + t.lastFailed + "</strong><p data-raw>" + esc(entry.error) + "</p></div></div>" : "") + '<div class="mon-grid">' + tiles.join("") + "</div>" + partitions;
}
/** The status line beside Pause: interval or Paused, then when the last sample came and the host's uptime. */
function monitorMeta(at, uptimeSeconds, history = "") {
  const t = T.monitor;
  return (state.dc.monPaused ? t.paused : t.while(state.dc.monInterval / 1000)) + (at ? t.updated(clock(at)) : "") + history + (uptimeSeconds != null ? t.up(formatUptime(uptimeSeconds)) : "");
}
function renderMonitorTab(key) {
  const entry = state.monitor.get(key), t = T.monitor, last = entry?.samples[entry.samples.length - 1];
  const history = entry?.samples.length > 1 ? t.history(formatUptime((last.at - entry.samples[0].at) / 1000)) : "";
  return '<div class="toolbar"><div class="segmented" role="group" aria-label="' + esc(t.every) + '">' + [2000, 5000, 10000].map(ms => '<button data-action="mon-interval" data-ms="' + ms + '" aria-pressed="' + (!state.dc.monPaused && state.dc.monInterval === ms) + '">' + esc(t.seconds(ms / 1000)) + "</button>").join("") +
    '</div><button data-action="mon-pause" aria-pressed="' + state.dc.monPaused + '">' + (state.dc.monPaused ? t.resume : t.pause) + '</button><span class="status-meta" id="mon-meta">' +
    esc(monitorMeta(last?.at, last?.uptimeSeconds, history)) + "</span></div>" +
    '<div id="mon-body">' + renderMonitorBody(key) + "</div>";
}
function refreshMonitorView() {
  const key = visibleMonitorKey();
  if (key && $("#dc-panel")) $("#dc-panel").innerHTML = renderDcPanel();
}
async function loadStats(key) {
  const entry = state.monitor.get(key) || { samples: [], loading: false, error: "" };
  if (entry.loading) return;
  entry.loading = true; state.monitor.set(key, entry);
  try {
    const sample = await rpc("monitor.stats", dcTargetParams(key));
    // The window's clock times the samples so rates stay right even if the Target's clock is off.
    entry.samples.push({ ...sample, at: Date.now() });
    if (entry.samples.length > MONITOR_SAMPLES) entry.samples.splice(0, entry.samples.length - MONITOR_SAMPLES);
    entry.error = "";
  } catch (error) { entry.error = String(error?.message || error); }
  finally { entry.loading = false; entry.polled = Date.now(); }
  if (visibleMonitorKey() === key) {
    $("#mon-body") && ($("#mon-body").innerHTML = renderMonitorBody(key));
    const meta = $("#mon-meta");
    const last = entry.samples[entry.samples.length - 1];
    if (meta) meta.textContent = monitorMeta(Date.now(), last?.uptimeSeconds);
  }
}
function dcTargetParams(key) { return key === "local" ? {} : { targetId: key }; }
async function loadContainers(key) {
  const previous = state.containers.get(key);
  state.containers.set(key, { ...previous, loading: true });
  if (state.page === "datacenter") $("#dc-panel").innerHTML = renderDcPanel();
  try {
    const data = await rpc("containers.list", dcTargetParams(key));
    state.containers.set(key, { data, at: new Date(), loading: false });
  } catch (error) {
    state.containers.set(key, { error: String(error?.message || error), at: new Date(), loading: false });
  }
  if (state.page === "datacenter") $("#dc-panel").innerHTML = renderDcPanel();
}
/** Key of the machine whose Containers/Images tab is visible, or null. */
function visibleContainerKey() {
  if (state.page !== "datacenter" || state.dc.tab !== "containers") return null;
  const found = state.dc.node === "dc" ? null : dcLookup(state.dc.node);
  if (found?.kind === "master") return dcKey(found.master.item);
  if (found?.kind === "node") return found.item.id;
  return null;
}
async function loadContainersIfShown(force = false) {
  const key = visibleContainerKey();
  if (key && (force || !state.containers.has(key))) await loadContainers(key);
}
/** The machine page in the Datacenter shows `tab` ("jobs", "files", …). */
const activityShown = () => state.page === "datacenter" && !state.dc.adding && state.dc.tab === "activity" && state.dc.node === "master:local";
const machineTab = tab => state.page === "datacenter" && !state.dc.adding && state.dc.tab === tab && Boolean(pageTarget());
async function selectTarget(id, tab = "summary") {
  state.page = "datacenter"; state.dc.node = "node:" + id; state.dc.tab = tab; state.dc.adding = null; state.ctLogs = null;
  state.selected = id; state.job = null; state.jobs = []; state.jobsError = "";
  state.files = null; state.filesError = ""; state.file = null; state.pendingJob = null; state.newJob = false;
  clearNotice(); render();
}
async function refreshJobs() {
  const targetId = state.selected;
  if (!targetId) return;
  try {
    const jobs = await rpc("jobs.list", { targetId });
    if (targetId !== state.selected) return;
    state.jobs = Array.isArray(jobs) ? jobs : []; state.jobsError = "";
  } catch (error) {
    if (targetId !== state.selected) return;
    state.jobsError = String(error);
  }
  if (machineTab("jobs") && $("#job-list")) $("#job-list").innerHTML = renderJobRows();
}
let logVersion = 0;
async function refreshSelectedJob(reset = false) {
  if (!state.job) return;
  const version = ++logVersion, targetId = state.selected, jobId = state.job.id, stream = state.stream;
  const [job, logs] = await Promise.all([
    rpc("jobs.status", { targetId, jobId }),
    rpc("jobs.logs", { targetId, jobId, stream, offset: reset ? 0 : state.offset, limit: 65536 })
  ]);
  if (version !== logVersion || targetId !== state.selected || jobId !== state.job?.id || stream !== state.stream) return;
  state.job = job; state.log = ((reset ? "" : state.log) + (logs.content || "")).slice(-500000);
  state.offset = logs.nextOffset; state.eof = logs.eof;
  if (machineTab("jobs") && $("#job-detail")) {
    const oldLog = $("#log-output"), scroll = oldLog?.scrollTop || 0;
    const wasAtBottom = !oldLog || oldLog.scrollHeight - scroll - oldLog.clientHeight < 40;
    $("#job-detail").innerHTML = renderJobDetail();
    if ($("#log-output")) $("#log-output").scrollTop = wasAtBottom ? $("#log-output").scrollHeight : scroll;
  }
}
async function importRegistration(registration) {
  const imported = await rpc("targets.import", { registration });
  await refreshFleet(); await selectTarget(imported.id || registration.id); notify(T.host.imported);
}
async function browseFiles(path) {
  const targetId = state.selected;
  const result = await rpc("files.list", { targetId, path });
  if (targetId !== state.selected) return;
  const resolved = Array.isArray(result) ? path : result.path || path;
  if (resolved !== state.files?.path) state.fileQuery = "";
  state.files = { path: resolved, entries: Array.isArray(result) ? result : result.entries || [] }; state.filesError = "";
  state.recentPaths = [resolved, ...(state.recentPaths || []).filter(row => row !== resolved)].slice(0, 6);
  state.file = null;
  if (machineTab("files")) render();
}
/** Opening the Files tab lists the system root right away instead of an empty path prompt. */
async function loadFilesIfShown() {
  if (!machineTab("files") || state.files || state.filesError) return;
  try { await browseFiles("~"); }
  catch (error) { state.filesError = String(error?.message || error); if (machineTab("files")) render(); }
}
const UPLOAD_MAX = 2 * 1024 ** 3, UPLOAD_CHUNK = 1024 * 1024;
const randomId = () => [...crypto.getRandomValues(new Uint8Array(16))].map(byte => byte.toString(16).padStart(2, "0")).join("");
/** Upload status above the file list: progress bar with a cancel ✕ at its end, or a Replace/Skip question. */
function renderUploadStatus() {
  const up = state.upload;
  if (!up) return "";
  const t = T.files, file = up.files[up.index], count = up.files.length > 1 ? t.count(up.index + 1, up.files.length) : "";
  if (up.ask) return '<div class="upload-status is-ask" role="alert"><p>' + t.exists(file.name) + esc(count) + "</p>" +
    '<div class="upload-ask"><button type="button" class="small" data-action="upload-answer" data-answer="replace">' + t.replace + '</button><button type="button" class="small" data-action="upload-answer" data-answer="skip">' + t.skip + "</button>" +
    '<button type="button" class="small" data-action="upload-cancel">' + t.cancelAll + "</button></div></div>";
  const percent = up.total ? Math.floor(up.sent / up.total * 100) : 0;
  return '<div class="upload-status" role="status"><p><span>' + (up.cancelling ? t.cancelling : t.uploading(file.name) + esc(count)) + '</span><span class="upload-numbers">' +
    esc(t.progress(formatSize(up.sent), formatSize(up.total), percent)) + "</span></p>" +
    '<div class="upload-row"><div class="upload-bar" role="progressbar" aria-label="' + esc(t.progressLabel) + '" aria-valuemin="0" aria-valuemax="100" aria-valuenow="' + percent + '"><i></i></div>' +
    '<button type="button" class="icon-button upload-cancel" data-action="upload-cancel" aria-label="' + esc(t.cancelUpload) + '" title="' + esc(t.cancelUpload) + '"' + (up.cancelling ? " disabled" : "") + ">" + icons.close + "</button></div></div>";
}
function paintUpload() {
  const box = $("#upload-status");
  if (!box) return;
  box.innerHTML = renderUploadStatus();
  const bar = box.querySelector(".upload-bar i");
  if (bar) bar.style.width = (state.upload.total ? state.upload.sent / state.upload.total * 100 : 0) + "%";
}
/** Asks Replace / Skip / Cancel in the status bar (no confirm(): the desktop webview has none). */
function askReplace() {
  state.upload.ask = true; paintUpload();
  return new Promise(resolve => { state.upload.answer = answer => { state.upload.ask = false; state.upload.answer = null; paintUpload(); resolve(answer); }; });
}
/** Upload files into the open folder in 1 MiB chunks (files.upload); a file appears only when complete and never replaces one without asking. */
async function uploadFiles(files) {
  const targetId = state.selected, folder = state.files?.path;
  if (!folder) throw new Error(T.files.openFirst);
  if (state.upload) { notify(T.files.busy, true); return; }
  const big = files.filter(file => file.size > UPLOAD_MAX);
  if (big.length) notify(T.files.tooBig(big.map(file => file.name).join(", ")), true);
  files = files.filter(file => file.size <= UPLOAD_MAX);
  if (!files.length) return;
  const up = state.upload = { files, index: 0, sent: 0, total: files.reduce((sum, file) => sum + file.size, 0), cancelled: false };
  const names = new Set((state.files?.entries || []).map(entry => entry.name));
  let done = 0, skipped = 0;
  paintUpload();
  try {
    for (; up.index < files.length && !up.cancelled; up.index++) {
      const file = files[up.index], path = joinPath(folder, file.name), uploadId = randomId(), base = up.sent;
      let replace = false;
      if (names.has(file.name)) {
        const answer = await askReplace();
        if (answer === "cancel") break;
        if (answer === "skip") { up.total -= file.size; skipped++; paintUpload(); continue; }
        replace = true;
      }
      let offset = 0;
      try {
        do {
          const bytes = new Uint8Array(await file.slice(offset, offset + UPLOAD_CHUNK).arrayBuffer());
          const last = offset + bytes.length >= file.size;
          await rpc("files.upload", { targetId, path, uploadId, offset, contentBase64: bytesToBase64(bytes), ...(last ? { done: true, ...(replace ? {} : { expectedSha256: "" }) } : {}) });
          offset += bytes.length; up.sent = base + offset; paintUpload();
        } while (offset < file.size && !up.cancelled);
        if (up.cancelled && offset < file.size) { await rpc("files.upload", { targetId, path, uploadId, abort: true }).catch(() => {}); break; }
        done++;
      } catch (error) {
        // Created by someone else meanwhile: ask, then commit the finished part or discard it.
        if (!/E_CONFLICT/.test(String(error?.message || error)) || offset + UPLOAD_CHUNK < file.size) { await rpc("files.upload", { targetId, path, uploadId, abort: true }).catch(() => {}); throw error; }
        const answer = await askReplace();
        if (answer === "replace") { await rpc("files.upload", { targetId, path, uploadId, offset: file.size, contentBase64: "", done: true }); up.sent = base + file.size; done++; }
        else { await rpc("files.upload", { targetId, path, uploadId, abort: true }).catch(() => {}); if (answer === "cancel") break; skipped++; }
      }
    }
  } finally {
    const cancelled = up.cancelled;
    state.upload = null; paintUpload();
    if (targetId === state.selected && state.files?.path === folder) await browseFiles(folder).catch(() => {});
    if (cancelled) notify(T.files.uploadCancelled(done));
    else if (done) notify(T.files.uploaded(done, folder, skipped));
  }
}
/** Saves through the native dialog; the app fetches the bytes itself, so any size works. */
async function downloadFile(path, name) {
  const saved = await native("save_remote_file", { targetId: state.selected, path, name, title: T.dialogs.download });
  if (saved) notify(T.files.downloaded(name, saved));
}
async function readFile(path) {
  const targetId = state.selected;
  const result = await rpc("files.read", { targetId, path, offset: 0, limit: 262144 });
  if (targetId !== state.selected) return;
  const bytes = Uint8Array.from(atob(result.contentBase64), c => c.charCodeAt(0));
  let content, validUtf8 = true;
  try { content = new TextDecoder("utf-8", { fatal: true }).decode(bytes); }
  catch { validUtf8 = false; content = T.files.binary; }
  if (content?.includes("\0")) { validUtf8 = false; content = T.files.binaryData; }
  state.file = { path, content, sha256: result.sha256, editable: validUtf8 && result.eof, size: result.size };
  if (machineTab("files")) render();
}
async function refreshFolders() { state.folders = await rpc("folders.list"); }
async function refreshDatacenters() { state.datacenters = await rpc("datacenters.list"); }
async function saveFolderEdit(input) {
  const editing = state.dc.editing;
  if (!editing || input.dataset.saving) return;
  input.dataset.saving = "1";
  const name = input.value.trim();
  try {
    if (name && editing.mode === "new-site") {
      await rpc("datacenters.create", { name }); await refreshDatacenters(); notify(T.tree.siteCreated(name));
    } else if (name && editing.mode === "rename-site") {
      await rpc("datacenters.rename", { id: editing.id, name }); await refreshDatacenters();
    } else if (name && editing.mode === "new") {
      const folder = await rpc("folders.create", { name, master: editing.master });
      state.dc.collapsed.delete("master:" + editing.master);
      await refreshFolders(); notify(T.tree.folderCreated(folder.name));
    } else if (name && editing.mode === "rename") {
      await rpc("folders.rename", { id: editing.id, name }); await refreshFolders();
    }
  } catch (error) { notify(error?.message || error, true); }
  state.dc.editing = null; render();
}
async function runDiagnose(targetId) {
  // Only the result slot is redrawn, so an open file editor or job form keeps its contents.
  const paint = () => { const slot = $("#check-slot"); if (slot && state.selected === targetId) slot.innerHTML = renderCheckPanel(state.targets.find(item => item.id === targetId)); };
  state.checking = targetId; (state.checkHidden ||= new Set()).delete(targetId); paint();
  try {
    const result = await rpc("targets.doctor", { targetId });
    state.diagnostics.set(targetId, { ...result, checkedAt: new Date().toISOString() });
  } finally { state.checking = null; paint(); }
}
async function removeTarget(targetId) {
  if (!confirm(T.host.removeConfirm(targetId))) return;
  await rpc("targets.remove", { targetId });
  if (state.selected === targetId) { state.selected = null; state.dc.node = "dc"; state.dc.tab = "summary"; }
  await refreshFleet(); render(); notify(T.host.removed);
}
const encodeBase64 = text => bytesToBase64(new TextEncoder().encode(text));
function bytesToBase64(bytes) {
  let raw = "";
  for (let i = 0; i < bytes.length; i += 8192) raw += String.fromCharCode(...bytes.subarray(i, i + 8192));
  return btoa(raw);
}
document.addEventListener("click", event => {
  if (event.target.closest("#notice-close")) { clearNotice(); return; }
  const pick = event.target.closest("[data-lang]");
  if (pick) { setLanguage(pick.dataset.lang); return; }
  const nav = event.target.closest("[data-page]");
  if (nav) { state.backup = null; if (nav.dataset.page === "datacenter") { state.dc.node = "dc"; state.dc.adding = null; } void navigate(nav.dataset.page); return; }
  const button = event.target.closest("[data-action]");
  if (button?.closest("#dc-menu")) closeDcMenu();
  if (!button) return;
  void busy(button, async () => {
    const action = button.dataset.action;
    if (action === "open-shell") return openShell(button.dataset.id);
    if (action === "close-shell-dialog") { $("#shell-dialog").close(); return; }
    if (action === "close-two-way-dialog") { $("#two-way-dialog").close(); return; }
    if (action === "open-two-way") {
      const dialog = $("#two-way-dialog"), merge = state.merges.find(row => row.targetId === button.dataset.id);
      dialog.dataset.peer = merge?.label || button.dataset.id; applyStatic();
      dialog.querySelector('[name="targetId"]').value = button.dataset.id;
      dialog.showModal(); return;
    }
    if (action === "copy") return copyText(button.dataset.copy);
    if (action === "copy-log") return copyText(state.log, T.jobs.logCopied);
    if (action === "copy-diagnostics") return copyText(pretty(state.diagnostics.get(state.selected)), T.check.copiedJson);
    if (action === "add") return navigate("enroll");
    if (action === "new-pairing") { state.pairing = null; addReset(); render(); }
    if (action === "refresh-pairings") { await refreshPairings(); render(); }
    if (action === "resume-pairing") { state.pairing = state.pairings.find(item => item.requestId === button.dataset.id); addReset(); render(); }
    if (action === "save-setup") await saveInstaller();
    if (action === "add-skip-save") { state.add.skipped.add(state.pairing?.requestId); render(); }
    if (action === "open-paired-target") { state.pairing = null; addReset(); await refreshFleet(); await selectTarget(button.dataset.id); }
    if (action === "back-fleet") { state.page = "datacenter"; state.dc.node = "dc"; state.selected = null; render(); }
    if (action === "tree-toggle") { state.dc.treeHidden = state.dc.treeHidden !== true; try { localStorage.setItem("sshcc.sideHosts", state.dc.treeHidden ? "folded" : "open"); } catch {} renderHeader(); return; }
    if (action === "backup-start") { state.backup = { mode: "backup", valid: false, saved: "", error: "" }; render(); $("#backup-flow [data-focus]")?.focus(); return; }
    if (action === "backup-save") return saveBackup();
    if (action === "backup-close") { const restored = state.backup?.result; state.backup = null; if (restored) { state.page = "datacenter"; state.dc.node = "dc"; } render(); return; }
    if (action === "restore-start") { state.page = "datacenter"; state.dc.adding = null; state.backup = { mode: "restore", file: null, summary: null, result: null, error: "", errorStep: 0 }; paintFlow(); return; }
    if (action === "restore-choose") return chooseBackup();
    if (action === "keys-change") return changeKeys();
    if (action === "tag-filter") { filterByTag(button.dataset.tag); return; }
    if (action === "close-group-dialog") { state.groupDraft = null; $("#group-dialog").close(); return; }
    if (action === "group-close") { clearTimeout(groupTimer); state.groupRun = null; render(); return; }
    if (action === "group-log") return openGroupLog(button.dataset.id, button.dataset.job);
    if (action === "group-cancel") { state.groupRun = await rpc("groups.cancel", { runId: state.groupRun.id, ...(button.dataset.id ? { targetId: button.dataset.id } : {}) }); render(); return watchGroup(); }
    if (action === "group-retry") { state.groupRun = await rpc("groups.retry", { runId: state.groupRun.id }); render(); return watchGroup(); }
    if (action === "alarm-tab") { state.alarmTab = button.dataset.tab; state.alarmEdit = null; render(); return; }
    if (action === "alarm-ack") { await rpc("alarms.ack", { id: button.dataset.id }); await refreshAlarms(); render(); return; }
    if (action === "alarm-rule-new") { state.alarmEdit = { scope: { all: true }, condition: "disk", threshold: 90, level: "red", containers: [] }; render(); return; }
    if (action === "alarm-rule-edit") { state.alarmEdit = structuredClone(state.alarmRules.find(r => r.id === button.dataset.id)); render(); return; }
    if (action === "alarm-rule-cancel") { state.alarmEdit = null; render(); return; }
    if (action === "alarm-rule-delete") {
      if (!confirm(T.alarms.removeConfirm)) return;
      await rpc("alarms.rule_delete", { id: button.dataset.id }); await Promise.all([refreshAlarmRules(), refreshAlarms()]); render(); notify(T.alarms.removed); return;
    }
    if (action === "alarm-for-host") {
      state.page = "alarms"; state.alarmTab = "rules"; state.alarmEdit = { scope: { hosts: [button.dataset.id] }, condition: "disk", threshold: 90, level: "red", containers: [] };
      await refreshAlarmRules(); render(); return;
    }
    if (action === "check-updates") return checkUpdates(button.dataset.id);
    if (action === "host-upgrade") return upgradeHost(button.dataset.id);
    if (action === "add-same") { const f = state.add.fetch; state.add.preview = { ...f.preview, requestId: f.requestId, fetched: true }; render(); return; }
    if (action === "add-different") {
      const id = state.add.fetch?.requestId;
      try { await rpc("pairing.confirm", { requestId: id, same: false }); }
      catch (error) { state.pairing = null; addReset(); await refreshPairings().catch(() => {}); render(); notify(explain(error, true).text, true); }
      return;
    }
    if (action === "host-upgrade-file") {
      const path = await native("save_upgrade", { targetId: button.dataset.id, os: button.dataset.os, title: T.upgrade.saveTitle });
      if (path) notify(T.upgrade.saved(path)); return;
    }
    if (action === "upgrade-all") { await rpc("targets.upgrade_all"); return watchUpgrades(); }
    if (action === "activity-view") { const a = activityState(); a.view = button.dataset.view; a.detail = null; $("#dc-panel").innerHTML = renderDcPanel(); return loadActivity(); }
    if (action === "activity-range") { const a = activityState(); a.range = button.dataset.range; if (a.range === "custom" && !a.from) a.from = thaiDay(); $("#dc-panel").innerHTML = renderDcPanel(); return loadActivity(); }
    if (action === "activity-toggle") { const open = activityState().open, id = button.dataset.group; if (open.has(id)) open.delete(id); else open.add(id); redrawActivity(); return; }
    if (action === "activity-open") return openActivityJob(button.dataset.target, button.dataset.job);
    if (action === "activity-detail-close") { activityState().detail = null; redrawActivity(); return; }
    if (action === "activity-export") return exportActivity();
    if (action === "ctx-tag") {
      state.page = "datacenter"; state.dc.node = button.dataset.node; state.dc.adding = null; state.backup = null;
      await selectTarget(state.dc.node.slice(5), "summary");
      $("#tag-add [name=tag]")?.focus(); return;
    }
    if (action === "tag-remove") return setHostTags(button.dataset.id, hostTags(button.dataset.id).filter(tag => tag !== button.dataset.tag));
    if (action === "restore-run") { if ($("#restore-off")?.checked) return runRestore(); return; }
    if (action === "add-host" || action === "dc-add") {
      state.page = "datacenter"; state.dc.adding = { folder: button.dataset.folder || "", site: button.dataset.site || "", self: Boolean(button.dataset.self) };
      state.pairing = null; state.pairedMaster = null; addReset(); render();
      try { await refreshPairings(); render(); } catch (error) { notifyError(error, true); }
    }
    if (action === "enroll-close") {
      state.pairing = null; addReset();
      if (state.page === "datacenter") { state.dc.adding = null; render(); } else await navigate("datacenter");
    }
    if (action === "pairing-back") { state.pairing = null; addReset(); render(); }
    if (action === "pairing-cancel") {
      const requestId = button.dataset.id;
      if (!confirm(addText.cancelConfirm)) return;
      const sibling = state.pairings.find(item => item.sibling === requestId && item.state === "awaiting_approval")?.requestId;
      for (const id of [requestId, sibling].filter(Boolean)) {
        await rpc("pairing.cancel", { requestId: id });
        state.saved.delete(id);
        if (state.pairing?.requestId === id) { state.pairing = null; addReset(); }
      }
      await refreshPairings(); render(); notify(addText.cancelled);
    }
    if (action === "dc-goto") {
      const node = button.dataset.node;
      if (button.dataset.view) state.dc.ctView = button.dataset.view;
      if (node.startsWith("node:")) await selectTarget(node.slice(5), button.dataset.tab || "summary");
      else { state.page = "datacenter"; state.dc.node = node; state.dc.tab = button.dataset.tab || "summary"; state.dc.adding = null; resetHostState(pageTarget()?.id); render(); }
      if (state.dc.tab === "jobs") await refreshJobs();
      await loadContainersIfShown(); await loadFilesIfShown();
      if (visibleMonitorKey()) await loadStats(visibleMonitorKey());
    }
    if (action === "ctx-diagnose") { await selectTarget(button.dataset.id); await runDiagnose(button.dataset.id); }
    if (action === "ctx-remove") await removeTarget(button.dataset.id);
    if (action === "folder-new") {
      state.dc.editing = { mode: "new", master: button.dataset.master };
      $("#dc-tree").innerHTML = renderDcTree(); $("#dc-edit")?.focus();
    }
    if (action === "folder-rename") {
      const folder = (state.folders.folders || []).find(row => row.id === button.dataset.id);
      state.dc.editing = { mode: "rename", id: button.dataset.id, master: folder?.master };
      $("#dc-tree").innerHTML = renderDcTree(); $("#dc-edit")?.select();
    }
    if (action === "folder-delete") {
      const folder = (state.folders.folders || []).find(row => row.id === button.dataset.id);
      if (!confirm(T.tree.folderDeleteConfirm(folder?.name || ""))) return;
      await rpc("folders.delete", { id: button.dataset.id });
      if (state.dc.node === "folder:" + button.dataset.id) state.dc.node = "master:" + (folder?.master || "local");
      await refreshFolders(); render(); notify(T.tree.folderDeleted);
    }
    if (action === "site-new") { state.dc.editing = { mode: "new-site" }; state.dc.collapsed.delete("dc"); $("#dc-tree").innerHTML = renderDcTree(); $("#dc-edit")?.focus(); }
    if (action === "site-rename") { state.dc.editing = { mode: "rename-site", id: button.dataset.id }; $("#dc-tree").innerHTML = renderDcTree(); $("#dc-edit")?.select(); }
    if (action === "site-delete") {
      const site = (state.datacenters.datacenters || []).find(row => row.id === button.dataset.id);
      if (!confirm(T.tree.siteDeleteConfirm(site?.name || ""))) return;
      await rpc("datacenters.delete", { id: button.dataset.id });
      if (state.dc.node === "site:" + button.dataset.id) state.dc.node = "dc";
      await refreshDatacenters(); render(); notify(T.tree.siteDeleted);
    }
    if (action === "site-unassign") { await rpc("datacenters.assign", { master: button.dataset.master, datacenterId: null }); await refreshDatacenters(); render(); }
    if (action === "folder-unfile") { await rpc("folders.assign", { targetId: button.dataset.id, folderId: null }); await refreshFolders(); render(); }
    if (action === "clear-filter") { state.query = ""; state.group = ""; state.status = ""; render(); }
    if (action === "status-filter") { state.status = button.dataset.status; $("#fleet-table").innerHTML = renderFleetView(); }
    if (action === "refresh-fleet") { await refreshFleet(); render(); await refreshHealth(true); }
    if (action === "fleet-view") {
      state.view = button.dataset.view; localStorage.setItem("sshcc.fleetView", state.view);
      document.querySelectorAll('[data-action="fleet-view"]').forEach(item => item.setAttribute("aria-pressed", String(item.dataset.view === state.view)));
      $("#fleet-table").innerHTML = renderFleetView();
    }
    if (action === "manage-merge") return navigate("masters");
    if (action === "sum-look") { state.invTab = "look"; $("#fleet-table").innerHTML = renderFleetView(); $(".inv-tabs")?.scrollIntoView({ behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth", block: "start" }); $('.inv-tab[aria-selected="true"]')?.focus({ preventScroll: true }); return; }
    if (action === "inv-tab") { state.invTab = button.dataset.tab; $("#fleet-table").innerHTML = renderFleetView(); $('.inv-tab[aria-selected="true"]')?.focus(); return; }
    if (action === "dc-select") {
      if (state.page !== "datacenter") state.page = "datacenter";
      state.dc.node = button.dataset.node; state.dc.adding = null; state.backup = null; state.ctLogs = null;
      if (state.dc.node.startsWith("node:")) await selectTarget(state.dc.node.slice(5), ["summary", "jobs", "files", "containers", "monitoring"].includes(state.dc.tab) ? state.dc.tab : "summary");
      else { resetHostState(pageTarget()?.id); render(); }
      if (machineTab("jobs")) await refreshJobs();
      await loadContainersIfShown(); await loadFilesIfShown();
    }
    if (action === "dc-toggle") {
      const node = button.dataset.node;
      if (state.dc.collapsed.has(node)) state.dc.collapsed.delete(node); else state.dc.collapsed.add(node);
      $("#dc-tree").innerHTML = renderDcTree();
    }
    if (action === "dc-tab") {
      state.dc.tab = button.dataset.tab; state.ctLogs = null; $("#dc-panel").innerHTML = renderDcPanel();
      if (machineTab("jobs")) await refreshJobs();
      if (activityShown()) await loadActivity(true);
      await loadContainersIfShown(); await loadFilesIfShown();
      if (visibleMonitorKey()) await loadStats(visibleMonitorKey());
    }
    if (action === "ct-view") { state.dc.ctView = button.dataset.view; $("#dc-panel").innerHTML = renderDcPanel(); }
    if (action === "mon-interval") {
      state.dc.monInterval = Number(button.dataset.ms); state.dc.monPaused = false;
      try { localStorage.setItem("sshcc.monitorInterval", String(state.dc.monInterval)); } catch {}
      refreshMonitorView();
    }
    if (action === "mon-pause") { state.dc.monPaused = !state.dc.monPaused; refreshMonitorView(); }
    if (action === "ct-filter") { state.dc.filter = button.dataset.filter; $("#dc-panel").innerHTML = renderDcPanel(); }
    if (action === "ct-refresh") await loadContainers(button.dataset.key);
    if (action === "ct-logs-close") { state.ctLogs = null; $("#dc-panel").innerHTML = renderDcPanel(); }
    if (action === "ct-logs") {
      const key = button.dataset.key, container = button.dataset.container;
      state.ctLogs = { key, container, loading: true }; $("#dc-panel").innerHTML = renderDcPanel();
      try {
        const result = await rpc("containers.logs", { ...dcTargetParams(key), container, tail: 200 });
        if (state.ctLogs?.key === key && state.ctLogs.container === container) state.ctLogs = { key, container, content: result.content };
      } catch (error) {
        if (state.ctLogs?.key === key) state.ctLogs = { key, container, error: String(error?.message || error) };
      }
      if (state.page === "datacenter") $("#dc-panel").innerHTML = renderDcPanel();
    }
    if (action === "ct-action") {
      const key = button.dataset.key, container = button.dataset.container, verb = button.dataset.verb;
      if (verb !== "start" && !confirm(T.containers.confirm(verb, container))) return;
      await rpc("containers.action", { ...dcTargetParams(key), container, action: verb });
      notify(T.containers.done(container, verb));
      await loadContainers(key);
    }
    if (action === "select-target") await selectTarget(button.dataset.id);
    if (action === "toggle-new-job") { state.newJob = !state.newJob; render(); if (state.newJob) $('#start-job [name="script"]')?.focus(); }
    if (action === "diagnose") await runDiagnose(state.selected);
    if (action === "check-close") { (state.checkHidden ||= new Set()).add(state.selected); $("#check-slot") && ($("#check-slot").innerHTML = ""); return; }
    if (action === "remove-target") await removeTarget(state.selected);
    if (action === "import-registration") {
      const registration = await native("open_json", { title: T.dialogs.openJson });
      if (registration) await importRegistration(registration);
    }
    if (action === "import-v1-registry") {
      const document = await native("open_json", { title: T.dialogs.openJson });
      if (document) {
        const result = await rpc("targets.import_legacy", { document });
        await refreshFleet(); notify(T.tree.registryImported(pretty(result)));
      }
    }
    if (action === "save-enrollment") {
      const path = await native("save_json", { document: state.export, title: T.dialogs.saveJson });
      if (path) notify(T.tree.enrollmentSaved(path));
    }
    if (action === "refresh-masters") { await refreshMasters(); render(); }
    if (action === "revoke-master") {
      if (!confirm(mastersText.revokeConfirm(button.dataset.id))) return;
      await rpc("masters.revoke", { masterId: button.dataset.id }); await refreshMasters(); render(); notify(mastersText.revokedNote);
    }
    if (action === "ctx-merge") {
      const [row] = await rpc("merges.discover", { targetId: button.dataset.id });
      if (row && !row.error) state.masterCheck.set(row.targetId, row.isMaster);
      const name = row?.label || button.dataset.id;
      if (!row?.isMaster) { notify(row?.error ? mastersText.notChecked(name, row.error) : mastersText.notMaster(name), Boolean(row?.error)); return; }
    }
    if (action === "merge-master" || action === "sync-merge" || action === "ctx-merge") {
      const targetId = button.dataset.id;
      state.merging.add(targetId); render();
      try {
        const merge = await rpc(action === "sync-merge" ? "merges.sync" : "merges.add", { targetId });
        notify(mastersText.mergedNote(merge.label || targetId, merge.children.length));
      } finally {
        state.merging.delete(targetId);
        await refreshMasters(); await refreshFleet(); render();
      }
    }
    if (action === "remove-merge") {
      if (!confirm(mastersText.removeConfirm(button.dataset.id))) return;
      await rpc("merges.remove", { targetId: button.dataset.id }); await refreshMasters(); await refreshFleet(); render(); notify(mastersText.removedNote);
    }
    if (action === "revoke-relay" || action === "allow-relay") {
      const revoke = action === "revoke-relay";
      if (revoke && !confirm(mastersText.disableConfirm)) return;
      await rpc(revoke ? "relays.revoke" : "relays.allow", { masterId: button.dataset.id }); await refreshMasters(); render();
      notify(revoke ? mastersText.disabledNote : mastersText.allowedNote);
    }
    if (action === "accept-enrollment") {
      const bundle = await native("open_json", { title: T.dialogs.openJson });
      if (!bundle) return;
      if (!confirm(mastersText.allowConfirm(bundle.master?.label || bundle.master?.id || ""))) return;
      await rpc("enrollment.accept", { bundle }); await refreshMasters(); render();
      notify(mastersText.allowedMaster);
    }
    if (action === "refresh-service") { await refreshLocal(); render(); }
    if (action === "connect-client") {
      await native("client_connect", { client: button.dataset.id });
      await refreshLocal(); render();
      notify(T.service.connectedNote(state.clientLinks.find(item => item.id === button.dataset.id)?.label || button.dataset.id));
    }
    if (action === "update-client-path") {
      await native("client_update_path", { client: button.dataset.id });
      await refreshLocal(); render();
      notify(T.service.updatedNote(state.clientLinks.find(item => item.id === button.dataset.id)?.label || button.dataset.id));
    }
    if (action === "refresh-jobs") { await refreshJobs(); await refreshSelectedJob(); }
    if (action === "select-job") {
      state.job = state.jobs.find(job => job.id === button.dataset.id);
      state.log = ""; state.offset = 0; state.eof = true;
      $("#job-list").innerHTML = renderJobRows(); $("#job-detail").innerHTML = renderJobDetail();
      await refreshSelectedJob(true);
    }
    if (action === "log-stream") {
      state.stream = button.dataset.stream; state.log = ""; state.offset = 0;
      $("#job-detail").innerHTML = renderJobDetail();
      await refreshSelectedJob(true);
    }
    if (action === "read-logs") await refreshSelectedJob();
    if (action === "cancel-job") {
      const targetId = state.selected, jobId = state.job.id;
      if (!confirm(T.jobs.cancelConfirm(jobId))) return;
      const job = await rpc("jobs.cancel", { targetId, jobId });
      if (targetId !== state.selected || jobId !== state.job?.id) return;
      state.job = job;
      await refreshJobs(); await refreshSelectedJob(); notify(T.jobs.cancelSent);
    }
    if (action === "upload-cancel") { if (state.upload) { state.upload.cancelled = state.upload.cancelling = true; state.upload.answer?.("cancel"); paintUpload(); } return; }
    if (action === "upload-answer") { state.upload?.answer?.(button.dataset.answer); return; }
    if (action === "upload-pick") { $("#upload-input")?.click(); return; }
    if (action === "path-menu") {
      const box = button.getBoundingClientRect(), chain = [];
      for (let path = state.files?.path; path && !chain.includes(path); path = parentPath(path)) chain.unshift(path);
      const items = [{ label: T.files.home, action: "open-folder", data: { path: "~" }, mono: true }, "-",
        ...chain.map((path, depth) => ({ label: depth ? path.split(/[\\/]/).filter(Boolean).pop() : path, action: "open-folder", data: { path }, indent: depth, mono: true }))];
      const recent = (state.recentPaths || []).filter(path => !chain.includes(path));
      if (recent.length) items.push("-", ...recent.map(path => ({ label: path, action: "open-folder", data: { path }, mono: true })));
      openMenu(items, box.right - 320, box.bottom + 6);
      return;
    }
    if (action === "file-more") { const box = button.getBoundingClientRect(); openFileMenu(button.closest("tr[data-file]"), box.right - 200, box.bottom + 4, box.top); return; }
    if (action === "file-download") { await downloadFile(button.dataset.path, button.dataset.name); return; }
    if (action === "file-delete") { openDeleteDialog(button.dataset.path, button.dataset.name, button.dataset.dir === "1"); return; }
    if (action === "close-delete") { $("#delete-dialog").close(); return; }
    if (action === "parent-folder") await browseFiles(parentPath($("#file-path").value));
    if (action === "open-file") await readFile(button.dataset.path);
    if (action === "open-folder") await browseFiles(button.dataset.path);
    if (action === "mkdir") {
      const folder = state.files?.path, targetId = state.selected;
      const name = (prompt(T.files.newFolderPrompt(folder)) || "").trim();
      if (!name) return;
      if (/[\\/]/.test(name) || name === "." || name === "..") throw new Error(T.files.plainName);
      await rpc("files.mkdir", { targetId, path: joinPath(folder, name) });
      if (targetId === state.selected) { await browseFiles(folder); notify(T.files.created(name)); }
    }
    if (action === "close-file") { state.file = null; $("#file-editor").innerHTML = ""; }
  });
});
// "Paste the code instead" stays open across redraws once opened.
document.addEventListener("toggle", event => { if (event.target.id === "add-paste-instead") state.add.pasting = event.target.open; }, true);
let activitySearchTimer = 0;
document.addEventListener("input", event => {
  if (event.target.form?.id === "create-pairing" && ["address", "port"].includes(event.target.name)) onWhereInput(event.target.form);
  if (event.target.id === "add-code") onCodeInput(event.target, event.inputType === "insertFromPaste");
  if (event.target.form?.id === "backup-pass-form") onBackupInput(event.target.form);
  if (event.target.name === "runOn" && event.target.form?.id === "start-job") onRunOnChange(event.target.form);
  if (event.target.id === "restore-off") $("#restore-run").disabled = !event.target.checked;
  if (event.target.closest?.("#confirm-delete")) { const form = $("#confirm-delete"); form.querySelector('[type="submit"]').disabled = event.target.value !== form.dataset.name; }
  if (event.target.id === "activity-search") { activityState().search = event.target.value; clearTimeout(activitySearchTimer); activitySearchTimer = setTimeout(() => void loadActivity().catch(error => notifyError(error)), 250); }
  if (event.target.id === "file-filter") { state.fileQuery = event.target.value; $("#files-output").innerHTML = renderFileList(); }
  if (event.target.id === "fleet-search") { state.query = event.target.value; $("#fleet-table").innerHTML = renderFleetView(); }
  if (event.target.id === "dc-search") {
    state.dc.query = state.query = event.target.value; $("#dc-tree").innerHTML = renderDcTree();
    if ($("#fleet-table")) $("#fleet-table").innerHTML = renderFleetView();
  }
});
document.addEventListener("change", event => {
  // One grant change at a time, in order: a quick second change must not be drawn over by the first one's reply.
  // The rule editor redraws when what it asks depends on the choice (folder / Tag / hosts, percent, container names).
  if (event.target.form?.id === "alarm-rule" && ["scope", "condition"].includes(event.target.name)) { state.alarmEdit = ruleFromForm(event.target.form); render(); }
  if (event.target.form?.id === "alarm-interval") void busy(null, async () => { const reply = await rpc("alarms.settings", { interval: Number(event.target.value) }); state.alarmInterval = reply.interval; notify(T.alarms.intervalSaved(reply.interval)); });
  if (event.target.dataset?.grant) { const control = event.target; grantQueue = grantQueue.then(() => busy(null, () => saveGrant(control))); }
  if (event.target.id === "group-filter") { state.group = event.target.value; $("#fleet-table").innerHTML = renderFleetView(); }
  if (event.target.id === "activity-status") { activityState().status = event.target.value; void loadActivity().catch(error => notifyError(error)); }
  if (["activity-from", "activity-to"].includes(event.target.id)) { activityState()[event.target.id.slice(9)] = event.target.value; void loadActivity().catch(error => notifyError(error)); }
});
document.addEventListener("submit", event => {
  const form = event.target;
  event.preventDefault();
  const values = Object.fromEntries(new FormData(form));
  const submittedPairing = state.pairing;
  void busy($('button[type="submit"]', form), async () => {
    if (form.id === "backup-pass-form") return saveBackup();
    if (form.id === "alarm-rule") {
      await rpc("alarms.rule_set", ruleFromForm(form)); state.alarmEdit = null;
      await Promise.all([refreshAlarmRules(), refreshAlarms()]); render(); return notify(T.alarms.saved);
    }
    if (form.id === "activity-keep-form") {
      activityState().info = await rpc("activity.settings", { keepDays: Number(form.elements.keepDays.value) });
      return notify(T.activity.keepSaved(activityState().info.keepDays));
    }
    if (form.id === "tag-add") { const tag = form.elements.tag.value; if (tag.trim()) return setHostTags(form.dataset.id, [...hostTags(form.dataset.id), tag]); return; }
    if (form.id === "restore-pass-form") { if (!state.backup?.summary) return openBackup(form); return; }
    if (form.id === "open-shell-session") {
      $("#shell-dialog").close();
      return openShell(values.targetId, values.ownerSessionId);
    }
    if (form.id === "confirm-two-way") {
      $("#two-way-dialog").close();
      const targetId = values.targetId;
      state.twoWay.set(targetId, { step: "request", message: T.twoWayDialog.starting }); render();
      try {
        const result = await native("merge_two_way", { targetId });
        const label = state.merges.find(row => row.targetId === targetId)?.label || targetId;
        if (result.status === "two-way") notify(mastersText.twoWayOn(label, result.children));
        else notify(mastersText.twoWayFailed(label, result.error || result.status), true);
      } finally {
        state.twoWay.delete(targetId);
        await refreshMasters(); await refreshFleet(); render();
      }
    }
    if (form.id === "create-pairing") {
      // Pressed before the check for this address answered: check first, so a duplicate is still refused
      // and a detected OS is still used.
      if (form.dataset.osKey !== whereKey(form)) { clearTimeout(whereTimer); await checkWhere(form); }
      if (form.dataset.duplicate) return;
      const address = values.address.trim();
      // Name is optional: an unnamed host is shown by its address.
      // No OS from Advanced or the check (no SSH answer yet): "unknown" saves both installers.
      const detected = form.dataset.osKey === whereKey(form) ? form.dataset.os : "";
      const params = { label: values.label.trim() || address, tenant: values.tenant, os: values.os || detected || "unknown", address, user: values.user, port: Number(values.port) };
      if (values.targetId.trim()) params.targetId = values.targetId.trim();
      if (values.routeKind) params.routeKind = values.routeKind;
      if (values.routes.trim()) params.routes = JSON.parse(values.routes);
      const folderId = await chosenFolder(values);
      const created = await rpc("pairing.create", params);
      state.pairing = Array.isArray(created) ? created[0] : created;
      // The Target ID is known now, so the machine lands in its folder as soon as it pairs.
      if (folderId) await rpc("folders.assign", { targetId: state.pairing.targetId, folderId });
      if (folderId) state.folders = await rpc("folders.list");
      await refreshPairings(); await saveInstaller();
    }
    if (form.id === "finish-pairing") {
      const preview = state.add.preview;
      if (!preview) return;
      // Finish: save with the edited name, then file it in the folder chosen on the card (only if it changed).
      // A code the Master fetched itself (the digits matched) is confirmed; a pasted one is completed.
      const folderId = await chosenFolder(values);
      const label = values.label.trim() || preview.label;
      const result = preview.fetched ? await rpc("pairing.confirm", { requestId: preview.requestId, same: true, label }) : await rpc("pairing.complete", { code: preview.code, label });
      if (folderId !== (state.folders.members?.[preview.targetId] || "")) { await rpc("folders.assign", { targetId: preview.targetId, folderId: folderId || null }); }
      state.folders = await rpc("folders.list");
      state.pairing = result; state.add.preview = null;
      await checkPairedMaster(result);
      await refreshPairings(); await refreshFleet(); render();
    }
    if (form.id === "retry-pairing") {
      if (!submittedPairing) return;
      const current = await rpc("targets.list");
      const registered = current.find(item => item.id === submittedPairing.targetId);
      const routes = (registered?.routes || submittedPairing.envelope.bundle.routes).map((route, i) => i === 0 ? { ...route, address: values.address.trim() } : route);
      const result = await rpc("pairing.status", { requestId: submittedPairing.requestId, retry: true, routes });
      if (state.pairing?.requestId === submittedPairing.requestId) state.pairing = result;
      await checkPairedMaster(result);
      await refreshPairings(); await refreshFleet(); render(); notify(result.state === "ready" ? T.tree.ready : T.tree.stillFailing, result.state !== "ready");
    }
    if (form.id === "edit-target") {
      const routes = JSON.parse(values.routes);
      if (!Array.isArray(routes) || !routes.length) throw new Error(T.host.routesArray);
      await rpc("targets.update", { target: { ...target(), label: values.label, routes } });
      await refreshFleet(); render(); notify(T.host.saved);
    }
    if (form.id === "paste-registration") await importRegistration(JSON.parse(values.document));
    if (form.id === "export-enrollment") {
      const routes = values.routes.trim() ? JSON.parse(values.routes) : [{ kind: values.kind, address: values.address, port: Number(values.port) }];
      if (!Array.isArray(routes) || !routes.length) throw new Error(T.host.routesArray);
      state.export = await rpc("enrollment.export", { targetId: values.targetId, label: values.label || values.targetId, tenant: values.tenant, os: values.os, user: values.user, routes });
      if (state.page === "enroll") {
        $("#enrollment-result").innerHTML = renderEnrollmentResult();
        $("#enrollment-result").scrollIntoView({ block: "nearest" });
      }
      notify(T.tree.enrollmentCreated);
    }
    if (form.id === "confirm-group") return runGroup();
    if (form.id === "start-job") {
      const env = JSON.parse(values.env || "{}");
      if (!env || Array.isArray(env) || typeof env !== "object" || Object.values(env).some(value => typeof value !== "string")) throw new Error(T.jobs.envObject);
      if (values.runOn) return draftGroupRun(form, values, env);
      const params = { targetId: state.selected, script: values.script, cwd: values.cwd, shell: values.shell, env };
      if(values.ownerSessionId) params.ownerSessionId=values.ownerSessionId;
      if(values.asAdmin === 'on') params.asAdmin=true;
      const fingerprint = JSON.stringify(params);
      if (state.pendingJob?.fingerprint !== fingerprint) state.pendingJob = { fingerprint, jobId: crypto.randomUUID() };
      const jobId = state.pendingJob.jobId;
      let job;
      try { job = await rpc("jobs.start", { ...params, jobId }); }
      // A refusal by a Merge Role is final: retrying the same job can't help.
      catch (error) { if (String(error).includes("E_MERGE_ROLE")) { state.pendingJob = null; throw error; } throw new Error(T.jobs.retry(String(error), jobId)); }
      state.pendingJob = null;
      if (params.targetId !== state.selected) return;
      state.job = job; state.log = ""; state.offset = 0; state.eof = true; state.stream = "stdout"; state.newJob = false;
      render(); await refreshJobs(); await refreshSelectedJob(true); notify(T.jobs.started(job.id));
    }
    if (form.id === "browse-files") await browseFiles(values.path.trim());
    if (form.id === "confirm-delete") {
      const { path, name } = form.dataset, targetId = state.selected, folder = state.files?.path;
      if (values.confirm !== name) return;
      $("#delete-dialog").close();
      await rpc("files.remove", { targetId, path });
      if (targetId === state.selected) { await browseFiles(folder); notify(T.files.deleted(name)); }
    }
    if (form.id === "save-file") {
      if (!state.file?.editable) return;
      const path = state.file.path, targetId = state.selected;
      await rpc("files.write", { targetId, path, contentBase64: encodeBase64(values.content), expectedSha256: state.file.sha256 });
      if (targetId === state.selected) { await readFile(path); notify(T.files.saved); }
    }
  });
});
$("#refresh-all").addEventListener("click", event => void busy(event.currentTarget, async () => {
  await refreshLocal();
  if (state.page === "datacenter") await refreshFleet();
  if (state.page === "datacenter") await loadContainersIfShown(true);
  if (state.page === "masters") await refreshMasters();
  if (state.page === "enroll") await refreshPairings();
  render();
}));
/* Activity: running jobs are asked again while the Jobs view is open. */
setInterval(async () => {
  const a = state.activity;
  if (document.hidden || state.activityPolling || !activityShown() || a?.view !== "jobs") return;
  const running = (a.jobs || []).some(entry => (entry.kind === "group" ? entry.hosts : [entry.job]).some(job => ["queued", "running"].includes(job.status)));
  if (!running) return;
  state.activityPolling = true;
  try { await loadActivity(true); } catch {} finally { state.activityPolling = false; }
}, 5000);
setInterval(async () => {
  if (document.hidden || state.polling || !machineTab("jobs") || !target()?.runnerInstalled) return;
  state.polling = true;
  try { await refreshJobs(); await refreshSelectedJob(); }
  catch (error) { notify(T.common.jobsFailed(error), true); }
  finally { state.polling = false; }
}, 5000);
setInterval(() => {
  if (document.hidden) return;
  void refreshLocal();
  refreshHealthIfDue();
}, 30000);
/* Folder name editing in the tree: Enter or leaving the field saves, Escape cancels. */
document.addEventListener("keydown", event => {
  if (event.target.id !== "dc-edit") return;
  if (event.key === "Enter") { event.preventDefault(); void saveFolderEdit(event.target); }
  if (event.key === "Escape") { event.stopPropagation(); state.dc.editing = null; $("#dc-tree").innerHTML = renderDcTree(); }
}, true);
document.addEventListener("focusout", event => { if (event.target.id === "dc-edit" && state.dc.editing) void saveFolderEdit(event.target); });
document.addEventListener("change", event => {
  if (event.target.id === "upload-input" && event.target.files?.length) { const files = [...event.target.files]; event.target.value = ""; void busy(null, () => uploadFiles(files)); return; }
  if (event.target.matches("[data-new-folder]")) { const input = event.target.form.elements.newFolder; input.hidden = event.target.value !== "__new__"; if (!input.hidden) input.focus(); }
});
const skyPoint = target => { const host = target.closest?.("[data-host]")?.dataset.host || target.closest?.('[data-node^="node:"]')?.dataset.node.slice(5); if (host && host !== "local") sky.point(host); };
/* Pointing at a host row, a tree item or a topology pod points the sky's callout at that host. */
document.addEventListener("mouseover", event => skyPoint(event.target));
document.addEventListener("focusin", event => skyPoint(event.target));
/* Files tab: drop files from this computer to upload them into the open folder. */
const draggingFiles = event => [...(event.dataTransfer?.types || [])].includes("Files");
document.addEventListener("dragover", event => {
  const zone = event.target.closest?.(".file-drop");
  if (!zone || !draggingFiles(event) || !state.files) return;
  event.preventDefault(); event.dataTransfer.dropEffect = "copy"; zone.classList.add("is-drop");
});
document.addEventListener("dragleave", event => { const zone = event.target.closest?.(".file-drop"); if (zone && !zone.contains(event.relatedTarget)) zone.classList.remove("is-drop"); });
document.addEventListener("drop", event => {
  const zone = event.target.closest?.(".file-drop");
  if (!zone || !draggingFiles(event)) return;
  event.preventDefault(); zone.classList.remove("is-drop");
  const files = [...event.dataTransfer.files];
  if (files.length) void busy(null, () => uploadFiles(files));
});
/* Drag a machine onto a folder to file it, or onto its Master to unfile it (same Master only). */
document.addEventListener("dragstart", event => {
  const master = event.target.closest?.("[data-drag-master]");
  if (master) {
    event.dataTransfer.setData("text/plain", "master:" + master.dataset.dragMaster);
    event.dataTransfer.effectAllowed = "move";
    state.dc.draggingMaster = master.dataset.dragMaster; state.dc.dragging = null;
    return;
  }
  const source = event.target.closest?.("[data-drag]");
  if (!source?.dataset.drag) return;
  state.dc.draggingMaster = null;
  event.dataTransfer.setData("text/plain", source.dataset.drag);
  event.dataTransfer.effectAllowed = "move";
  state.dc.dragging = source.dataset.drag;
});
document.addEventListener("dragend", () => { state.dc.dragging = null; state.dc.draggingMaster = null; document.querySelectorAll(".dc-row.is-drop").forEach(row => row.classList.remove("is-drop")); });
document.addEventListener("dragover", event => {
  if (state.dc.draggingMaster) {
    const site = event.target.closest?.("[data-drop-site]");
    if (!site) return;
    event.preventDefault(); event.dataTransfer.dropEffect = "move";
    document.querySelectorAll(".dc-row.is-drop").forEach(other => { if (other !== site) other.classList.remove("is-drop"); });
    site.classList.add("is-drop");
    return;
  }
  const row = event.target.closest?.("[data-drop]");
  if (!row || !state.dc.dragging || dcBranch(state.dc.dragging) !== row.dataset.drop) return;
  event.preventDefault(); event.dataTransfer.dropEffect = "move";
  document.querySelectorAll(".dc-row.is-drop").forEach(other => { if (other !== row) other.classList.remove("is-drop"); });
  row.classList.add("is-drop");
});
document.addEventListener("dragleave", event => { const row = event.target.closest?.("[data-drop]"); if (row && !row.contains(event.relatedTarget)) row.classList.remove("is-drop"); });
document.addEventListener("drop", event => {
  const site = event.target.closest?.("[data-drop-site]"), payload = event.dataTransfer.getData("text/plain");
  if (site && payload.startsWith("master:")) {
    event.preventDefault(); site.classList.remove("is-drop");
    const master = payload.slice(7), datacenterId = site.dataset.dropSite === "-" ? null : site.dataset.dropSite;
    if (((state.datacenters.members || {})[master] || null) === datacenterId) return;
    void busy(null, async () => {
      await rpc("datacenters.assign", { master, datacenterId });
      if (datacenterId) state.dc.collapsed.delete("site:" + datacenterId);
      await refreshDatacenters(); render();
    });
    return;
  }
  const row = event.target.closest?.("[data-drop]"), targetId = payload;
  if (targetId.startsWith("master:")) return;
  if (!row || !targetId || dcBranch(targetId) !== row.dataset.drop) return;
  event.preventDefault(); row.classList.remove("is-drop");
  const folderId = row.dataset.dropFolder || null;
  if ((state.folders.members || {})[targetId] === (folderId || undefined)) return;
  void busy(null, async () => {
    await rpc("folders.assign", { targetId, folderId });
    if (folderId) state.dc.collapsed.delete("folder:" + folderId);
    await refreshFolders(); $("#dc-tree").innerHTML = renderDcTree();
    if (state.dc.node.startsWith("folder:") || state.dc.node.startsWith("master:")) $("#dc-panel").innerHTML = renderDcPanel();
  });
});
/* vCenter-style context menu for tree rows: right-click, the Menu key or Shift+F10. */
function dcMenuItems(node) {
  const found = node === "dc" ? { kind: "dc" } : dcLookup(node);
  if (!found) return [];
  const item = (label, action, data = {}, extra = {}) => ({ label, action, data, ...extra });
  const local = found.kind === "dc" || found.master?.current;
  const m = T.menu, remoteNote = found.master && !found.master.current ? m.remoteNote(found.master.label) : "";
  if (found.kind === "site") return [item(m.addHost, "dc-add", { site: found.site.id }), "-", item(m.rename, "site-rename", { id: found.site.id }), item(m.deleteSite, "site-delete", { id: found.site.id }, { danger: true })];
  if (found.kind === "dc") return [item(m.addHost, "dc-add"), "-", item(m.topology, "dc-goto", { node: "dc" }), "-", item(m.refresh, "refresh-fleet")];
  if (found.kind === "master") return [
    item(m.addHost, "dc-add", {}, { disabled: !local, title: remoteNote }),
    item(m.newFolder, "folder-new", { master: found.master.id }),
    "-",
    item(m.openTerminal, "open-shell", { id: found.master.item?.id || "" }, found.master.current ? { disabled: true, title: m.selfTerminal } : { disabled: !found.master.item?.runnerInstalled }),
    item(m.containers, "dc-goto", { node, tab: "containers", view: "containers" }), item(m.monitoring, "dc-goto", { node, tab: "monitoring" }),
    ...(local ? [] : ["-", item(m.manageMerge, "manage-merge")]), "-", item(m.refresh, "refresh-fleet"),
    ...(local ? [] : ["-", item(m.removeMerge, "remove-merge", { id: found.master.id }, { danger: true })])];
  if (found.kind === "folder") return [
    item(m.addHere, "dc-add", { folder: found.folder.id }, { disabled: !local, title: remoteNote }),
    "-", item(m.rename, "folder-rename", { id: found.folder.id }), item(m.deleteFolder, "folder-delete", { id: found.folder.id }, { danger: true })];
  const target = found.item;
  return [
    item(m.openTerminal, "open-shell", { id: target.id }, { disabled: !target.runnerInstalled }),
    item(m.checkConnection, "ctx-diagnose", { id: target.id }),
    "-",
    item(m.jobs, "dc-goto", { node, tab: "jobs" }), item(m.files, "dc-goto", { node, tab: "files" }),
    item(m.containers, "dc-goto", { node, tab: "containers", view: "containers" }), item(m.images, "dc-goto", { node, tab: "containers", view: "images" }),
    item(m.monitoring, "dc-goto", { node, tab: "monitoring" }),
    "-", item(m.tag, "ctx-tag", { node }),
    // Health only reads the SSH banner, so whether a machine is a Master is asked on click (one SSH call).
    ...(!target.relayed && state.masterCheck.get(target.id) !== false && !state.merges.some(merge => merge.targetId === target.id) ? ["-", item(m.mergeIfMaster, "ctx-merge", { id: target.id })] : []),
    ...(dcFolderOf(target.id) ? ["-", item(m.unfile(dcFolderOf(target.id).name), "folder-unfile", { id: target.id })] : []),
    ...(target.relayed ? [] : ["-", item(m.removeHost, "ctx-remove", { id: target.id }, { danger: true })])];
}
function closeDcMenu() { $("#dc-menu")?.remove(); }
function openDcMenu(node, x, y) { openMenu(dcMenuItems(node), x, y); }
/** Small popup menu at (x, y); items are {label, action, data, danger, disabled, title, indent} or "-".
 *  With `above` (the top of the button that opened it), a menu that does not fit below opens above instead of covering the button. */
function openMenu(items, x, y, above) {
  closeDcMenu();
  if (!items.length) return;
  const menu = document.createElement("div");
  menu.id = "dc-menu"; menu.className = "ctx-menu"; menu.setAttribute("role", "menu");
  menu.innerHTML = items.map(entry => entry === "-" ? '<div class="ctx-sep" role="separator"></div>'
    : '<button role="menuitem" class="ctx-item' + (entry.danger ? " danger" : "") + (entry.indent ? " indent-" + Math.min(entry.indent, 8) : "") + '" data-action="' + entry.action + '"' +
      Object.entries(entry.data).map(([key, value]) => " data-" + key + '="' + esc(value) + '"').join("") +
      (entry.disabled ? " disabled" : "") + (entry.title ? ' title="' + esc(entry.title) + '"' : "") + ">" + esc(entry.label) + "</button>").join("");
  if (items.some(entry => entry.mono)) menu.classList.add("is-mono");
  document.body.append(menu);
  const box = menu.getBoundingClientRect();
  menu.style.left = Math.max(4, Math.min(x, innerWidth - box.width - 4)) + "px";
  const top = above !== undefined && y + box.height > innerHeight - 4 ? above - box.height - 4 : y;
  menu.style.top = Math.max(4, Math.min(top, innerHeight - box.height - 4)) + "px";
  menu.querySelector(".ctx-item:not(:disabled)")?.focus();
}
/** One menu for a file row, from its ⋮ button or a right-click. */
function openFileMenu(row, x, y, above) {
  const { file: path, name, dir } = row.dataset;
  const t = T.files;
  openMenu([{ label: dir ? t.openFolder : t.open, action: dir ? "open-folder" : "open-file", data: { path } },
    ...(dir ? [] : [{ label: t.download, action: "file-download", data: { path, name } }]), "-",
    { label: t.deleteMenu, action: "file-delete", data: { path, name, dir }, danger: true }], x, y, above);
}
document.addEventListener("contextmenu", event => {
  const file = event.target.closest("tr[data-file]");
  if (file) { event.preventDefault(); openFileMenu(file, event.clientX, event.clientY); return; }
  const row = event.target.closest(".dc-row .dc-item");
  if (!row) return;
  event.preventDefault();
  openDcMenu(row.dataset.node, event.clientX, event.clientY);
});
document.addEventListener("keydown", event => {
  if (event.key === "Escape") { closeDcMenu(); return; }
  const menu = $("#dc-menu");
  if (menu && ["ArrowDown", "ArrowUp"].includes(event.key)) {
    const items = [...menu.querySelectorAll(".ctx-item:not(:disabled)")], index = items.indexOf(document.activeElement);
    items[(index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus();
    event.preventDefault(); return;
  }
  const row = event.target.closest?.(".dc-item");
  if (row && (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10"))) {
    event.preventDefault();
    const box = row.getBoundingClientRect();
    openDcMenu(row.dataset.node, box.left + 24, box.bottom);
  }
});
document.addEventListener("pointerdown", event => { if (!event.target.closest("#dc-menu")) closeDcMenu(); }, true);
window.addEventListener("blur", closeDcMenu);
window.addEventListener("resize", closeDcMenu);
// Monitoring polls only while its tab is on screen, the window has focus and it is not paused.
setInterval(() => {
  const key = visibleMonitorKey();
  if (!key || state.dc.monPaused || document.hidden || !document.hasFocus()) return;
  const entry = state.monitor.get(key);
  if (!entry?.loading && Date.now() - (entry?.polled || 0) >= state.dc.monInterval - 250) void loadStats(key);
}, 1000);
// Containers refresh only while their tab is on screen and the window has focus.
setInterval(() => {
  if (document.hidden || !document.hasFocus() || state.dc.tab !== "containers") return;
  const key = visibleContainerKey();
  if (key && !state.containers.get(key)?.loading) void loadContainers(key);
}, 15000);
window.addEventListener("focus", refreshHealthIfDue);
window.__TAURI__?.event?.listen("merge-two-way", ({ payload }) => {
  if (!state.twoWay.has(payload.targetId) || payload.step === "done") return;
  state.twoWay.set(payload.targetId, payload);
  if (state.page === "masters") render();
});
window.__TAURI__?.event?.listen("management-window-shown", async () => {
  await refreshLocal();
  if (state.page !== "datacenter") return;
  try { await refreshFleet(); if (state.page === "datacenter" && !holdsForm() && !state.dc.adding) render(); } catch (error) { notify(error, true); }
  void refreshHealth();
});
applyStatic();
await refreshLocal();
try { await refreshFleet(); } catch (error) { notify(error, true); }
render();
void refreshHealth();
