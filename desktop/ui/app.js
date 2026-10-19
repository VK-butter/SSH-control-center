const $ = (selector, parent = document) => parent.querySelector(selector);
const esc = value => String(value ?? "").replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const pretty = value => JSON.stringify(value, null, 2);
document.documentElement.dataset.theme = "dark";
const titles = { fleet: "เครื่องทั้งหมด", enroll: "เพิ่มเครื่อง", masters: "Master ที่อนุญาต", service: "Local service" };
const state = {
  page: "fleet", targets: [], masters: [], service: null, serviceError: "", selected: null,
  tab: "overview", query: "", group: "", status: "", diagnostics: new Map(), jobs: [], jobsError: "",
  job: null, stream: "stdout", log: "", offset: 0, eof: true, polling: false, newJob: false,
  export: null, pendingJob: null, file: null, files: null, pairing: null, pairings: [], setupSaved: null,
  merges: [], relays: [], discovered: null, merging: new Set(),
  view: localStorage.getItem("sshcc.fleetView") === "table" ? "table" : "graph",
  health: new Map(), healthError: "", healthLoading: false, healthAt: null, healthStarted: 0, flipped: new Set(), twoWay: new Map(),
  clientLinks: [], clientLinksError: ""
};
const native = (command, args = {}) => {
  if (!window.__TAURI__?.core?.invoke) throw new Error("เปิดโปรแกรม SSH Control Center เพื่อเชื่อมต่อ local service");
  return window.__TAURI__.core.invoke(command, args);
};
const rpc = (method, params = {}) => native("rpc", { method, params });
const target = () => state.targets.find(item => item.id === state.selected);
const isWindows = item => /win/i.test(item?.os || "");
const osName = os => ({ windows: "Windows", linux: "Linux" })[os] || os || "";
const timestamp = value => {
  if (!value) return "—";
  const date = new Date(typeof value === "number" ? value * 1000 : value);
  return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString("th-TH", { dateStyle: "short", timeStyle: "short" });
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
  if (days) return days + " วัน " + (hours % 24) + " ชม.";
  if (hours) return hours + " ชม. " + (minutes % 60) + " นาที";
  return minutes + " นาที";
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
  up: svg('<path d="M8 13V3.5M4 7.5l4-4 4 4"/>'),
  chevron: svg('<path d="m6 3.5 4.5 4.5L6 12.5"/>'),
  folder: svg('<path d="M1.75 4.25c0-.7.55-1.25 1.25-1.25h3l1.5 1.75H13c.7 0 1.25.55 1.25 1.25v6c0 .7-.55 1.25-1.25 1.25H3c-.7 0-1.25-.55-1.25-1.25Z"/>'),
  file: svg('<path d="M4 1.75h5l3.25 3.25v8c0 .7-.55 1.25-1.25 1.25H4c-.7 0-1.25-.55-1.25-1.25V3c0-.7.55-1.25 1.25-1.25Z"/><path d="M9 1.75V5h3.25"/>'),
  copy: svg('<rect x="5.25" y="5.25" width="8.5" height="8.5" rx="1.5"/><path d="M10.75 5.25V3.5c0-.7-.55-1.25-1.25-1.25H3.5c-.7 0-1.25.55-1.25 1.25v6c0 .7.55 1.25 1.25 1.25h1.75"/>'),
  check: svg('<path d="m3.25 8.25 3 3 6.5-6.5"/>'),
  alert: svg('<path d="M8 2 1.5 13.5h13Z"/><path d="M8 6.5v3.25M8 11.75h.01"/>'),
  info: svg('<circle cx="8" cy="8" r="6.25"/><path d="M8 7.25v4M8 4.75h.01"/>'),
  pulse: svg('<path d="M1.5 8.5h3l1.75-4.5 3.5 8 1.75-3.5h3"/>'),
  close: svg('<path d="m4 4 8 8M12 4l-8 8"/>')
};
const chip = text => '<span class="chip">' + esc(text) + "</span>";
function notify(message, error = false) {
  const notice = $("#notice");
  $("#notice-text").textContent = String(message); notice.classList.toggle("error", error); notice.hidden = false;
}
function clearNotice() { $("#notice").hidden = true; }
async function copyText(text, message = "คัดลอกแล้ว") {
  try { await navigator.clipboard.writeText(String(text ?? "")); }
  catch { throw new Error("คัดลอกไม่ได้ ลองเลือกข้อความแล้วกด Ctrl+C"); }
  notify(message);
}
async function openShell(targetId, ownerSessionId) {
  const item = state.targets.find(row => row.id === targetId);
  if (!item) throw new Error("ไม่พบเครื่องนี้ในทะเบียน");
  const result = await native("open_shell", { targetId, ownerSessionId: ownerSessionId || null });
  if (result.needsSession) {
    $("#shell-machine").textContent = item.label || item.id;
    $("#shell-target").value = item.id;
    $("#shell-session").innerHTML = '<option value="">เลือก session ที่ต้องการ</option>' + result.sessions.map(session => '<option value="' + esc(session.id) + '">' + esc(session.account) + ' · session ' + esc(session.id) + ' · ' + esc(session.state) + '</option>').join("");
    $("#shell-dialog").showModal();
    return;
  }
  notify("เปิด Shell ของ " + (item.label || item.id) + " ในหน้าต่าง Terminal แล้ว");
}
const jobLabels = { succeeded: "สำเร็จ", failed: "ล้มเหลว", interrupted: "ถูกขัดจังหวะ", cancelled: "ยกเลิกแล้ว", running: "กำลังรัน", queued: "รอคิว" };
function badge(status) {
  const styles = { succeeded: "good", failed: "bad", interrupted: "bad", cancelled: "", running: "running", queued: "pending" };
  return '<span class="badge ' + (styles[status] || "") + '" title="' + esc(status) + '">' + esc(jobLabels[status] || status) + "</span>";
}
async function busy(button, work) {
  if (button?.disabled) return;
  if (button) button.disabled = true;
  clearNotice();
  try { return await work(); }
  catch (error) { notify(state.page === "enroll" ? pairingError(error) : error?.message || error, true); }
  finally { if (button?.isConnected) button.disabled = false; }
}
function renderHeader() {
  const item = state.page === "fleet" ? target() : null;
  $("#breadcrumb").innerHTML = item
    ? '<button data-action="back-fleet">' + esc(titles.fleet) + "</button>" + icons.chevron + "<strong>" + esc(item.label || item.id) + "</strong>"
    : "<strong>" + esc(titles[state.page]) + "</strong>";
  $("#fleet-count").textContent = String(state.targets.length);
  $("#node-label").textContent = state.service?.label || "Local service";
  $("#node-role").textContent = state.service?.roles?.join(" + ") || "ยังไม่เชื่อมต่อ";
  $("#node-dot").className = "dot " + (state.service ? "good" : "bad");
  $("#service-label").textContent = state.service ? "Service พร้อมใช้งาน" : "Service ไม่พร้อม";
  $("#service-dot").className = "dot " + (state.service ? "good" : "bad");
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
  const [rows, merges, relays] = await Promise.all([
    rpc("targets.list"), rpc("merges.list").catch(() => []), rpc("relays.list").catch(() => [])
  ]);
  if (!Array.isArray(rows)) throw new Error("Service ส่งรายการ Target ในรูปแบบที่ไม่รองรับ");
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
  if (state.page === "fleet" && !target() && $("#fleet-table")) $("#fleet-table").innerHTML = renderFleetView();
}
function refreshHealthIfDue() {
  if (state.page !== "fleet" || target() || document.hidden || !document.hasFocus()) return;
  if (Date.now() - state.healthStarted >= HEALTH_INTERVAL - 1000) void refreshHealth();
}
async function navigate(page) {
  state.page = page; clearNotice(); render();
  if (page === "fleet") {
    try { await refreshFleet(); if (state.page === page) render(); } catch (error) { notify(error, true); }
    void refreshHealth();
  }
  if (page === "masters") {
    try { await refreshMasters(); if (state.page === page) render(); } catch (error) { notify(error, true); }
  }
  if (page === "service") { await refreshLocal(); if (state.page === page) render(); }
  if (page === "enroll") { try { await refreshPairings(); if (state.page === page) render(); } catch (error) { notify(pairingError(error), true); } }
}
function render() {
  renderHeader();
  const main = $("#main");
  if (state.page === "fleet") main.innerHTML = target() ? renderTarget() : renderFleet();
  if (state.page === "enroll") main.innerHTML = renderEnrollment();
  if (state.page === "masters") main.innerHTML = renderMasters();
  if (state.page === "service") main.innerHTML = renderService();
}
function heading(title, description, actions = "") {
  return '<div class="page-heading"><div><h1>' + esc(title) + "</h1>" + (description ? "<p>" + esc(description) + "</p>" : "") + "</div>" + (actions ? '<div class="page-actions">' + actions + "</div>" : "") + "</div>";
}
function renderFleet() {
  const groups = [...new Set(state.targets.map(item => item.tenant).filter(Boolean))].sort();
  return heading("เครื่องทั้งหมด", "เลือกเครื่องเพื่อดูการเชื่อมต่อ งาน และไฟล์", '<button class="primary" data-action="add">' + icons.plus + "เพิ่มเครื่อง</button>") +
    '<div class="toolbar"><div class="search">' + icons.search + '<input id="fleet-search" type="search" aria-label="ค้นหาเครื่อง" placeholder="ค้นหาชื่อเครื่อง กลุ่ม หรือ IP…" value="' + esc(state.query) + '"></div>' +
    '<select id="group-filter" aria-label="กรองกลุ่ม"><option value="">ทุกกลุ่ม</option>' +
    groups.map(group => '<option value="' + esc(group) + '"' + (state.group === group ? " selected" : "") + ">" + esc(group) + "</option>").join("") +
    '</select><div class="segmented" role="group" aria-label="มุมมอง">' +
    [["graph", "Graph", icons.graph], ["table", "ตาราง", icons.table]].map(([view, label, icon]) => '<button data-action="fleet-view" data-view="' + view + '" aria-pressed="' + (state.view === view) + '">' + icon + label + "</button>").join("") +
    '</div><button data-action="refresh-fleet" title="ตรวจสถานะทุกเครื่องผ่าน SSH อีกครั้ง">' + icons.refresh + 'รีเฟรช</button></div><div id="fleet-table">' + renderFleetView() + "</div>" +
    '<details class="fleet-hint"><summary>สถานะออนไลน์ตรวจอย่างไร</summary><div><ul><li>ตรวจพอร์ต SSH ทุก 1 นาทีขณะเปิดหน้าต่างนี้</li><li>กดรีเฟรชเพื่อตรวจผ่าน SSH เต็มรูปแบบ</li><li>ตรวจละเอียดรายเครื่องได้ที่ “ตรวจการเชื่อมต่อ” ในหน้าเครื่อง</li><li>รองรับ 50 Target ต่อ Master</li></ul></div></details>';
}
function renderFleetView() {
  if (!state.targets.length) return renderFleetRows();
  return renderStatusBar() + (state.view === "graph" ? renderGraph() : renderFleetRows());
}
const stateLabels = { online: "ออนไลน์", offline: "ออฟไลน์", checking: "กำลังตรวจ…", unknown: "ไม่ทราบสถานะ", legacy: "รอติดตั้ง service" };
const stateStyles = { online: "good", offline: "bad", checking: "pending", legacy: "pending" };
function nodeState(item) {
  if (item.runnerInstalled === false) return "legacy";
  const row = state.health.get(item.id);
  if (!row) return state.healthError ? "unknown" : "checking";
  return row.online === true ? "online" : row.online === false ? "offline" : "unknown";
}
const statusBucket = item => ({ online: "online", offline: "offline" })[nodeState(item)] || "other";
function statusBadge(item) {
  const status = nodeState(item), error = state.health.get(item.id)?.error;
  return '<span class="badge ' + (stateStyles[status] || "") + '"' + (error ? ' title="' + esc(error) + '"' : "") + ">" + esc(stateLabels[status]) + "</span>";
}
function renderStatusBar() {
  const pool = state.targets.filter(item => matchesFilter(item, true));
  const count = bucket => pool.filter(item => statusBucket(item) === bucket).length;
  const filters = [["", "ทั้งหมด", pool.length, ""], ["online", "ออนไลน์", count("online"), "good"], ["offline", "ออฟไลน์", count("offline"), "bad"], ["other", "ยังไม่ทราบ", count("other"), "pending"]]
    .filter(([id, , total]) => id !== "other" || total || state.status === "other");
  const meta = state.healthError
    ? '<span class="status-meta error" title="' + esc(state.healthError) + '">' + icons.alert + "ตรวจสถานะไม่ได้ ชี้เพื่อดูสาเหตุ</span>"
    : '<span class="status-meta">' + (state.healthAt ? "ตรวจล่าสุด " + esc(timestamp(state.healthAt)) : "กำลังตรวจสถานะทุกเครื่อง…") + "</span>";
  return '<div class="status-bar"><div class="status-filters" role="group" aria-label="กรองตามสถานะ">' +
    filters.map(([id, label, total, dot]) => '<button class="status-filter" data-action="status-filter" data-status="' + id + '" aria-pressed="' + (state.status === id) + '">' + (dot ? '<span class="dot ' + dot + '"></span>' : "") + label + " <strong>" + total + "</strong></button>").join("") +
    "</div>" + meta + "</div>";
}
function pcIcon(master) {
  const monitor = '<rect class="pc-body" x="2" y="4" width="44" height="30" rx="3"/><rect class="pc-screen" x="5" y="7" width="38" height="24" rx="1.5"/>' +
    '<path class="pc-glare" d="M9 29 27 9h7L16 29Z"/><rect class="pc-line" x="9" y="12" width="13" height="2.5" rx="1"/><rect class="pc-line pc-type" x="9" y="18" width="22" height="2.5" rx="1"/>' +
    '<rect class="pc-cursor" x="9" y="24" width="4" height="2.5" rx=".5"/><path class="pc-stand" d="M20 34h8l1.5 6h-11ZM14 40h20a1.5 1.5 0 0 1 0 3H14a1.5 1.5 0 0 1 0-3Z"/>';
  const tower = '<rect class="pc-body" x="50" y="7" width="12" height="36" rx="2"/><rect class="pc-bay" x="52.5" y="11" width="7" height="2" rx=".5"/><rect class="pc-bay" x="52.5" y="15" width="7" height="2" rx=".5"/><circle class="pc-led" cx="56" cy="37" r="1.6"/>';
  return '<svg class="pc" viewBox="0 0 64 46" aria-hidden="true">' + (master ? monitor + tower : '<g transform="translate(7 0)">' + monitor + "</g>") + "</svg>";
}
function graphNode(item, options = {}) {
  const status = options.status || nodeState(item), row = state.health.get(item.id), name = item.label || item.id, route = item.routes?.[0];
  const hint = status === "offline" || status === "unknown" ? row?.error || "" : status === "online" && row?.checkedAt ? "ตรวจเมื่อ " + timestamp(row.checkedAt) : "";
  const sub = options.sub || [osName(item.os), item.tenant, !item.relayed && route?.address ? route.address : ""].filter(Boolean).join(" · ");
  const main = pcIcon(options.master) + '<span class="node-info"><span class="node-label">' + esc(name) +
    (options.alsoDirect ? ' <span class="node-tag">เชื่อมตรงด้วย</span>' : "") + '</span><span class="node-sub">' + esc(sub) + "</span>" +
    '<span class="node-state"><span class="dot"></span>' + esc(stateLabels[status]) + "</span></span>";
  return '<div class="node is-' + status + (options.master ? " is-master" : "") + (options.root ? " is-root" : "") + (state.flipped.has(item.id) ? " just-changed" : "") + '">' +
    (options.static ? '<div class="node-main">' + main + "</div>" : '<button class="node-main" data-action="select-target" data-id="' + esc(item.id) + '"' + (hint ? ' title="' + esc(hint) + '"' : "") + ">" + main + "</button>") +
    (options.static || item.runnerInstalled === false ? "" : '<button class="node-shell" data-action="open-shell" data-id="' + esc(item.id) + '" aria-label="เปิด Shell ของ ' + esc(name) + '" title="เปิด Shell">' + icons.shell + "</button>") + "</div>";
}
function matchesFilter(item, ignoreStatus = false) {
  const search = state.query.toLowerCase();
  return (!state.group || state.group === item.tenant) && (ignoreStatus || !state.status || statusBucket(item) === state.status) &&
    [item.id, item.label, item.tenant, item.viaLabel, ...(item.routes || []).map(route => route.address)].some(value => String(value || "").toLowerCase().includes(search));
}
function isTwoWay(merge) {
  const peer = state.targets.find(item => item.id === merge?.targetId);
  return merge?.reverse?.status === "two-way" && state.relays.some(relay => relay.masterId === peer?.nodeId && !relay.revoked);
}
const noMatch = '<div class="empty"><h2>ไม่พบเครื่องที่ค้นหา</h2><p>ลองค้นหาด้วยชื่อ กลุ่ม หรือ IP อื่น หรือล้างตัวกรองสถานะ</p><button data-action="clear-filter">ล้างตัวกรอง</button></div>';
function renderGraph() {
  const direct = state.targets.filter(item => !item.relayed), byNode = new Map(direct.map(item => [item.nodeId, item]));
  const peers = state.merges.map(merge => ({ merge, peer: direct.find(item => item.id === merge.targetId) })).filter(entry => entry.peer);
  const peerIds = new Set(peers.map(entry => entry.peer.id));
  const online = list => list.filter(entry => nodeState(entry.item) === "online").length;
  const branches = [];
  const own = direct.filter(item => !peerIds.has(item.id) && matchesFilter(item)).map(item => ({ item }));
  if (own.length || !peers.length) {
    branches.push({
      live: Boolean(state.service),
      head: '<div class="sector-title"><div><h3>เชื่อมตรงจากเครื่องนี้</h3><span class="meta-row">ออนไลน์ ' + online(own) + "/" + own.length + " เครื่อง</span></div></div>",
      nodes: own
    });
  }
  for (const { merge, peer } of peers) {
    const children = (merge.children || []).map(child => byNode.has(child.nodeId)
      ? { item: byNode.get(child.nodeId), alsoDirect: true }
      : { item: state.targets.find(item => item.id === peer.id + "/" + child.id) }).filter(entry => entry.item);
    const shown = children.filter(entry => matchesFilter(entry.item));
    if (!shown.length && !matchesFilter(peer)) continue;
    const both = isTwoWay(merge);
    const direction = '<span class="direction ' + (both ? "is-both" : "") + '" title="' + (both ? "สั่งเครื่องลูกของกันและกันได้" : "เราสั่งเครื่องลูกของ Master นี้ได้ แต่ Master นี้ยังสั่งเครื่องลูกของเราไม่ได้") + '">' + (both ? icons.both + "สองทาง" : icons.one + "ทางเดียว") + "</span>";
    const synced = merge.status === "merged";
    branches.push({
      live: nodeState(peer) === "online",
      head: '<div class="sector-title"><div><h3>ผ่าน ' + esc(peer.label || peer.id) + '</h3><span class="meta-row"><span' + (synced ? ' title="sync ล่าสุด ' + esc(timestamp(merge.syncedAt)) + '"' : "") + ">ออนไลน์ " + online(shown) + "/" + shown.length + " เครื่อง</span>" + direction +
        (synced ? "" : '<span class="badge pending" title="' + esc(merge.error || merge.status) + '">merge มีปัญหา</span>') +
        '</span></div><button class="text-button" data-action="manage-merge">จัดการ merge</button></div>' +
        graphNode(peer, { master: true, sub: [ "Master", peer.routes?.[0]?.address, children.length + " เครื่องลูก" ].filter(Boolean).join(" · ") }),
      nodes: shown
    });
  }
  if (!branches.length) return noMatch;
  const root = graphNode({ id: "local", label: state.service?.label || "เครื่องนี้" }, { master: true, root: true, static: true, status: state.service ? "online" : "offline", sub: "เครื่องนี้ · " + ((state.service?.roles || []).join(" + ") || "master") });
  return '<div class="constellation"><div class="constellation-core' + (state.service ? " is-live" : "") + '">' + root + '</div><div class="constellation-sectors' + (branches.length > 1 ? " is-multi" : "") + '">' +
    branches.map(branch => '<section class="sector' + (branch.live ? " is-live" : "") + '"><div class="sector-head">' + branch.head + "</div>" +
      (branch.nodes.length ? '<div class="sector-nodes">' + branch.nodes.map(entry => graphNode(entry.item, { alsoDirect: entry.alsoDirect })).join("") + "</div>" : '<p class="empty-inline">ไม่มีเครื่องที่ตรงกับตัวกรอง</p>') + "</section>").join("") +
    "</div></div>";
}
function renderFleetRows() {
  const rows = state.targets.filter(item => matchesFilter(item));
  if (!state.targets.length) return '<div class="empty"><h2>เริ่มจากเชื่อมเครื่องแรก</h2><p>สร้างชุดติดตั้งสำหรับเครื่องใหม่ กด Approve ที่เครื่องนั้น แล้วคัดลอกรหัสกลับมาวางที่นี่</p><div class="page-actions"><button class="primary" data-action="add">' + icons.plus + 'เพิ่มเครื่อง</button><button data-action="import-registration">นำเข้า registration</button></div></div>';
  if (!rows.length) return noMatch;
  return '<div class="table-wrap"><table><thead><tr><th>เครื่อง</th><th>กลุ่ม</th><th>ที่อยู่</th><th>สถานะ</th><th></th></tr></thead><tbody>' +
    rows.map(item => {
      const check = state.diagnostics.get(item.id), route = item.routes?.[0];
      const status = !item.runnerInstalled ? '<span class="badge pending">รอติดตั้ง service</span>' : check ? '<span class="badge ' + (check.ok ? "good" : "bad") + '">' + (check.ok ? "เชื่อมต่อได้" : "ตรวจพบปัญหา") + "</span><small>ตรวจเมื่อ " + esc(timestamp(check.checkedAt)) + "</small>" : statusBadge(item);
      const address = item.relayed ? '<span class="muted">ผ่าน ' + esc(item.viaLabel || item.via) + "</span>"
        : route ? '<code title="' + esc(route.kind || "") + '">' + esc(route.address) + (route.port && route.port !== 22 ? ":" + esc(route.port) : "") + "</code>" : "—";
      return '<tr class="clickable"><td><div class="name-cell"><div><button class="row-button" data-action="select-target" data-id="' + esc(item.id) + '">' + esc(item.label || item.id) + "</button><small>" + esc([osName(item.os), item.label && item.label !== item.id ? item.id : ""].filter(Boolean).join(" · ")) + "</small></div></div></td>" +
        "<td>" + (item.tenant ? chip(item.tenant) : '<span class="muted">—</span>') + "</td><td>" + address + "</td><td>" + status + "</td>" +
        '<td class="actions"><div class="target-actions"><button class="small" data-action="open-shell" data-id="' + esc(item.id) + '" aria-label="เปิด Shell ของ ' + esc(item.label || item.id) + '"' + (!item.runnerInstalled ? ' disabled title="ติดตั้ง Target service ก่อน"' : "") + ">" + icons.shell + '<span class="btn-label">เปิด Shell</span></button><button class="icon-button" data-action="select-target" data-id="' + esc(item.id) + '" aria-label="เปิดรายละเอียด ' + esc(item.label || item.id) + '" title="เปิดรายละเอียด">' + icons.chevron + "</button></div></td></tr>";
    }).join("") + "</tbody></table></div>";
}
function stat(label, value, sub = "") {
  return '<div class="stat"><span class="stat-label">' + esc(label) + '</span><div class="stat-value">' + value + "</div>" + (sub ? "<small>" + esc(sub) + "</small>" : "") + "</div>";
}
function renderTarget() {
  const item = target();
  return '<button class="text-button back" data-action="back-fleet">' + icons.back + "เครื่องทั้งหมด</button>" +
    '<div class="page-heading"><div class="target-title"><h1>' + esc(item.label || item.id) + '</h1><div class="meta-row">' + statusBadge(item) +
    (item.os ? chip(osName(item.os)) : "") + (item.tenant ? chip(item.tenant) : "") + (item.relayed ? chip("ผ่าน " + (item.viaLabel || item.via)) : "") + "</div></div>" +
    '<div class="target-actions"><button class="primary" data-action="open-shell" data-id="' + esc(item.id) + '"' + (!item.runnerInstalled ? ' disabled title="ติดตั้ง Target service ก่อน"' : "") + ">" + icons.shell + 'เปิด Shell</button><button data-action="diagnose">' + icons.pulse + "ตรวจการเชื่อมต่อ</button></div></div>" +
    (!item.runnerInstalled ? '<div class="callout pending">' + icons.alert + "<div><strong>เครื่องนี้นำเข้าด้วย v1 registry</strong><p>ติดตั้ง Target service แล้วนำเข้า registration ใหม่ เพื่อเรียกงานบนเครื่องนี้</p></div></div>" : "") +
    '<div class="tabs" role="tablist" aria-label="ข้อมูลเครื่อง">' +
    [["overview", "การเชื่อมต่อ"], ["jobs", "งานและ logs"], ["files", "ไฟล์"]].map(([id, label]) => '<button role="tab" aria-selected="' + (id === state.tab) + '" class="tab ' + (id === state.tab ? "active" : "") + '" data-action="target-tab" data-tab="' + id + '">' + label + "</button>").join("") + "</div>" +
    (state.tab === "overview" ? renderOverview(item) : state.tab === "jobs" ? renderJobs(item) : renderFiles(item));
}
const routeKinds = { lan: "LAN", tailscale: "Tailscale", public: "Public IP" };
function healthStat(item) {
  const row = state.health.get(item.id), status = nodeState(item);
  return stat("สถานะล่าสุด", '<span class="dot ' + (stateStyles[status] || "") + '"></span>' + esc(stateLabels[status]), row?.checkedAt ? "ตรวจเมื่อ " + timestamp(row.checkedAt) : "");
}
function renderOverview(item) {
  const check = state.diagnostics.get(item.id);
  if (item.relayed) {
    return '<div class="stat-grid">' + stat("เชื่อมต่อผ่าน Master", esc(item.viaLabel || item.via)) + stat("สิทธิ์", "ใช้สิทธิ์ของ Master นั้น") + healthStat(item) + "</div>" +
      renderDiagnostics(check) +
      '<div class="disclosure-list"><details><summary>ข้อมูลเทคนิค <small class="muted">Master ID, Node ID</small></summary><div><dl class="facts"><dt>Master ID</dt><dd><code>' + esc(item.via) + "</code></dd><dt>Node ID</dt><dd><code>" + esc(item.nodeId) + '</code></dd></dl><p class="help mt">คำสั่งส่งผ่าน Master นั้น แก้เส้นทางหรือนำเครื่องนี้ออกได้ที่ Master ' + esc(item.viaLabel || item.via) + " · ยกเลิก merge ได้ที่หน้า Master ที่อนุญาต</p></div></details></div>";
  }
  const routes = item.routes || [], primary = routes[0];
  return '<div class="stat-grid">' +
    stat("ที่อยู่หลัก", primary ? "<code>" + esc(primary.address) + ":" + esc(primary.port || 22) + "</code>" : "—", primary ? (primary.proxyJump ? "ผ่าน " + primary.proxyJump.address : routeKinds[primary.kind] || primary.kind || "") : "") +
    stat("เส้นทางสำรอง", routes.length > 1 ? (routes.length - 1) + " เส้นทาง" : "ไม่มี") +
    healthStat(item) +
    stat("Target service", item.runnerInstalled ? '<span class="dot good"></span>ติดตั้งแล้ว' : '<span class="dot pending"></span>ยังไม่ติดตั้ง') +
    "</div>" + renderDiagnostics(check) +
    '<div class="disclosure-list">' +
    '<details><summary>ข้อมูลเทคนิค <small class="muted">Node ID, SSH user, เส้นทางทั้งหมด, host keys</small></summary><div><dl class="facts"><dt>Node ID</dt><dd><code>' + esc(item.nodeId || "รอ registration ใหม่") + "</code></dd><dt>SSH user</dt><dd>" + esc(item.user) + "</dd><dt>Bridge</dt><dd><code>" + esc(item.bridgePath || "—") + "</code></dd><dt>Target ID</dt><dd><code>" + esc(item.id) + "</code></dd></dl>" +
    '<div class="table-wrap mt"><table><thead><tr><th>ลำดับ</th><th>ประเภท</th><th>ปลายทาง</th><th>ผ่านเครื่อง</th></tr></thead><tbody>' +
    routes.map((route, i) => '<tr><td class="num">' + (i + 1) + "</td><td>" + esc(routeKinds[route.kind] || route.kind) + "</td><td><code>" + esc(route.address) + ":" + esc(route.port || 22) + "</code></td><td>" + (route.proxyJump?.address ? "<code>" + esc(route.proxyJump.address) + "</code>" : '<span class="muted">ตรง</span>') + "</td></tr>").join("") +
    '</tbody></table></div><h3 class="mt">Host keys ที่ pin ไว้</h3><div class="output light mt"><pre>' + esc((item.hostKeys || []).join("\n") || "—") + "</pre></div></div></details>" +
    '<details><summary>แก้ไขชื่อและเส้นทาง</summary><form id="edit-target"><div class="form-grid"><label class="full">ชื่อเครื่อง<input name="label" required value="' + esc(item.label || item.id) + '"></label><label class="full">เส้นทาง JSON<textarea class="mono" name="routes" rows="7" required spellcheck="false">' + esc(pretty(routes)) + '</textarea><small>ลำดับในรายการคือลำดับที่ลองเชื่อมต่อ · ProxyJump ต้องมี hostKeys และ identityFile</small></label></div><div class="form-actions"><button type="submit" class="primary">บันทึก</button></div><p class="help">เปลี่ยนกลุ่มโดยนำเครื่องออกแล้วลงทะเบียนใหม่ เพื่อใช้กุญแจของกลุ่มที่ถูกต้อง</p></form></details>' +
    '<details><summary>นำเครื่องออกจาก Master นี้</summary><div><p class="help">ลบเฉพาะทะเบียนใน Master นี้ งานและข้อมูลบนเครื่องยังอยู่ หากต้องการถอนสิทธิ์ ให้ทำในหน้า Master ที่อนุญาตบนเครื่องนั้น</p><button class="danger" data-action="remove-target">นำ ' + esc(item.label || item.id) + " ออก</button></div></details>" +
    "</div>";
}
function renderDiagnostics(check) {
  const stages = check?.stages || [], failed = stages.filter(stage => !stage.ok);
  const detail = stage => stage.message || (stage.executor ? pretty(stage.executor) : "");
  const selected = check?.result?.executor?.selected || stages.find(stage => stage.name === "executor")?.executor?.selected;
  const selectedNote = selected?.ownerSessionId
    ? "<p>ใช้ Owner session <code>" + esc(selected.ownerSessionId) + "</code>" + (selected.account ? " (" + esc(selected.account) + ")" : "") + "</p>"
    : "";
  const summary = !check
    ? '<div class="callout">' + icons.info + "<div><strong>ยังไม่ได้ตรวจในรอบนี้</strong><p>ตรวจทะเบียน กุญแจ SSH และ Target service โดยไม่เริ่มงานบนเครื่องปลายทาง</p></div></div>"
    : check.ok
      ? '<div class="callout good">' + icons.check + "<div><strong>เชื่อมต่อได้ ผ่านทุกขั้นตอน</strong><p>ตรวจ " + stages.length + " ขั้นตอนเมื่อ " + esc(timestamp(check.checkedAt)) + "</p>" + selectedNote + "</div></div>"
      : '<div class="callout bad">' + icons.alert + "<div><strong>ตรวจพบปัญหา " + (failed.length || "") + " ขั้นตอน</strong>" +
        (failed.length ? "<ul>" + failed.map(stage => "<li><b>" + esc(stage.name) + "</b> — " + esc(detail(stage)) + "</li>").join("") + "</ul>" : "") +
        "<p>ตรวจเมื่อ " + esc(timestamp(check.checkedAt)) + "</p></div></div>";
  const table = stages.length ? '<details><summary>ดูรายละเอียดผลการตรวจ</summary><div class="table-wrap"><table><thead><tr><th>ขั้นตอน</th><th>ผล</th><th>รายละเอียด</th></tr></thead><tbody>' +
    stages.map(stage => "<tr><td>" + esc(stage.name) + '</td><td><span class="badge ' + (stage.ok ? "good" : "bad") + '">' + (stage.ok ? "ผ่าน" : "ไม่ผ่าน") + '</span></td><td class="wrap">' + esc(detail(stage)) + "</td></tr>").join("") +
    '</tbody></table></div><div class="form-actions"><button class="small" data-action="copy-diagnostics">' + icons.copy + "คัดลอกผลตรวจ JSON</button></div></details>" : "";
  return '<section class="section mt"><div class="section-heading"><h2>ผลตรวจการเชื่อมต่อ</h2>' + (check ? '<button class="small" data-action="diagnose">' + icons.refresh + "ตรวจอีกครั้ง</button>" : "") + "</div>" + summary + table + "</section>";
}
function renderJobs(item) {
  const disabled = !item.runnerInstalled ? ' disabled title="ติดตั้ง Target service ก่อน"' : "";
  return (state.newJob ? renderNewJob(item) : "") +
    '<section class="section"><div class="section-heading"><h2>งานบนเครื่องนี้</h2><div class="page-actions">' +
    (state.newJob ? "" : '<button class="primary" data-action="toggle-new-job"' + disabled + ">" + icons.plus + "เริ่มงานใหม่</button>") +
    '<button data-action="refresh-jobs">' + icons.refresh + 'รีเฟรช</button></div></div><div class="job-layout"><div class="job-list" id="job-list">' + renderJobRows() + '</div><div class="job-detail" id="job-detail">' + renderJobDetail() + "</div></div></section>";
}
function renderNewJob(item) {
  const windows = isWindows(item);
  return '<form id="start-job" class="surface new-job"><h2>เริ่มงานใหม่</h2><div class="form-grid">' +
    '<label class="full">Working directory<input name="cwd" required class="mono" placeholder="' + (windows ? "C:\\projects\\my-app" : "/srv/my-app") + '"></label>' +
    '<label class="full">คำสั่งหรือสคริปต์<textarea name="script" required rows="6" class="mono" spellcheck="false" placeholder="' + (windows ? "Get-Location" : "pwd") + '"></textarea></label></div>' +
    '<details><summary>ตัวเลือกขั้นสูง</summary><div class="form-grid"><label>Shell<select name="shell"><option value="' + (windows ? "powershell" : "bash") + '">' + (windows ? "PowerShell" : "Bash") + "</option></select></label>" +
    '<label>Owner session ID<input name="ownerSessionId" placeholder="ว่างไว้ให้ระบบเลือกอัตโนมัติ"></label>' +
    (!windows ? '<label class="check-label full"><input type="checkbox" name="asAdmin">รันด้วยสิทธิ์ admin (root)</label>' : "") +
    '<label class="full">Environment JSON <small>ตัวแปรเฉพาะงานนี้</small><textarea name="env" rows="2" class="mono" spellcheck="false">{}</textarea></label></div></details>' +
    '<div class="form-actions"><button type="submit" class="primary"' + (!item.runnerInstalled ? " disabled" : "") + ">เริ่มงาน</button>" +
    '<button type="button" data-action="toggle-new-job">ยกเลิก</button><span class="help">งานทำต่อแม้ปิดหน้าต่าง · ส่งซ้ำหลัง timeout จะใช้ job ID เดิม ไม่รันซ้ำ</span></div></form>';
}
function renderJobRows() {
  if (state.jobsError) return '<div class="error-inline">' + esc(state.jobsError) + "</div>";
  if (!state.jobs.length) return '<div class="empty-inline">ยังไม่มีงานบนเครื่องนี้<br>กด “เริ่มงานใหม่” เพื่อส่งคำสั่ง</div>';
  return state.jobs.map(job => '<button class="job-row ' + (state.job?.id === job.id ? "active" : "") + '" data-action="select-job" data-id="' + esc(job.id) + '"><div>' + badge(job.status) + "<small>" + esc(timestamp(job.createdAt)) + "</small></div><code>" + esc(job.script) + "</code></button>").join("");
}
function renderJobDetail() {
  const job = state.job;
  if (!job) return '<div class="empty-inline">เลือกงานเพื่อดูผล<br><small>stdout, stderr และ exit code เก็บไว้ที่เครื่องปลายทาง</small></div>';
  const running = ["running", "queued"].includes(job.status);
  return '<div class="job-head"><h3>Job <code title="Job ID ' + esc(job.id) + " · Master " + esc(job.masterId) + '">' + esc(job.id.slice(0, 8)) + "</code></h3>" + badge(job.status) +
    (job.exitCode != null ? '<span class="chip">Exit ' + esc(job.exitCode) + "</span>" : "") +
    (running ? '<button class="danger small right" data-action="cancel-job">ยกเลิกงาน</button>' : "") + "</div>" +
    '<div class="output light"><pre>' + esc(job.script) + "</pre></div>" +
    '<div class="job-meta"><span>' + icons.folder + "<code>" + esc(job.cwd) + "</code></span><span>" + esc(timestamp(job.startedAt || job.createdAt)) + "</span></div>" +
    '<div class="console"><div class="console-bar"><div class="segmented" role="group" aria-label="Log stream">' +
    ["stdout", "stderr"].map(stream => '<button data-action="log-stream" data-stream="' + stream + '" aria-pressed="' + (state.stream === stream) + '">' + stream + "</button>").join("") +
    '</div><button class="small" data-action="read-logs">' + (state.eof ? "โหลดเพิ่ม" : "หน้าถัดไป") + '</button><button class="small" data-action="copy-log">' + icons.copy + "คัดลอก</button>" +
    (running ? '<small><span class="dot good"></span>อัปเดตทุก 5 วินาที</small>' : "") +
    '</div><div class="output" id="log-output" tabindex="0" aria-label="Job logs"><pre>' + esc(state.log || "ยังไม่มี output") + "</pre></div></div>";
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
function renderFiles(item) {
  return '<div class="section-heading"><div><h2>ไฟล์บนเครื่องนี้</h2><p class="help">เปิดและแก้ไขไฟล์ UTF-8 หรือสร้างโฟลเดอร์ ด้วยสิทธิ์ของ service</p></div></div>' +
    '<form id="browse-files" class="path-bar"><button type="button" class="icon-button" data-action="parent-folder" aria-label="ขึ้นไปโฟลเดอร์ก่อนหน้า" title="ขึ้นไปโฟลเดอร์ก่อนหน้า">' + icons.up + "</button>" +
    '<input name="path" id="file-path" aria-label="Absolute path ของไฟล์หรือโฟลเดอร์" class="mono" required value="' + esc(state.file?.path || state.files?.path || (isWindows(item) ? "C:\\" : "/")) + '">' +
    '<button type="submit" class="primary">' + icons.folder + 'เปิดโฟลเดอร์</button><button type="button" data-action="read-file">' + icons.file + 'เปิดไฟล์</button><button type="button" data-action="mkdir">' + icons.plus + "สร้างโฟลเดอร์</button></form>" +
    '<div id="files-output">' + renderFileList() + '</div><div id="file-editor">' + renderFileEditor() + "</div>";
}
function renderFileList() {
  if (!state.files) return '<div class="empty-inline">ระบุ absolute path แล้วกด “เปิดโฟลเดอร์” หรือ “เปิดไฟล์”</div>';
  const entries = [...state.files.entries].sort((a, b) => (b.isDirectory ? 1 : 0) - (a.isDirectory ? 1 : 0) || String(a.name).localeCompare(String(b.name)));
  if (!entries.length) return '<div class="empty-inline">โฟลเดอร์นี้ว่าง</div>';
  return '<div class="table-wrap"><table><thead><tr><th>ชื่อ</th><th>ขนาด</th><th></th></tr></thead><tbody>' +
    entries.map(entry => '<tr><td><button class="row-button file-name' + (entry.isDirectory ? " folder" : "") + '" data-action="' + (entry.isDirectory ? "open-folder" : "open-file") + '" data-path="' + esc(entry.path) + '">' + (entry.isDirectory ? icons.folder : icons.file) + esc(entry.name) + "</button>" + (entry.isSymlink ? " " + chip("link") : "") +
      '</td><td class="num muted" title="' + esc(entry.size ?? "") + ' bytes">' + (entry.isDirectory ? "—" : esc(formatSize(entry.size))) + '</td><td class="actions"><button class="text-button danger" data-action="remove-file" data-path="' + esc(entry.path) + '">ลบ</button></td></tr>').join("") + "</tbody></table></div>";
}
function renderFileEditor() {
  if (!state.file) return "";
  return '<div class="file-editor surface"><div class="section-heading"><h3><code>' + esc(state.file.path) + '</code></h3><button class="icon-button" data-action="close-file" aria-label="ปิดไฟล์" title="ปิดไฟล์">' + icons.close + "</button></div>" +
    '<form id="save-file"><textarea name="content" aria-label="เนื้อหาไฟล์" class="mono" rows="14" spellcheck="false"' + (!state.file.editable ? " readonly" : "") + ">" + esc(state.file.content) + '</textarea><div class="form-actions"><button type="submit" class="primary"' + (!state.file.editable ? " disabled" : "") + '>บันทึกไฟล์</button><span class="help">' + (state.file.editable ? "ตรวจ SHA-256 ก่อนบันทึก กันการเขียนทับการแก้ไขจากเครื่องอื่น" : "แสดงบางส่วนหรือไม่ใช่ UTF-8 · ใช้ CLI สำหรับไฟล์ใหญ่หรือ binary") + "</span></div></form></div>";
}
function pairingState(item) {
  const labels = { awaiting_approval: "รออนุมัติที่เครื่องใหม่", needs_attention: "ติดตั้งแล้ว — รอตรวจการเชื่อมต่อ", ready: "พร้อมใช้งาน" };
  return labels[item?.state] || "กำลังเตรียม";
}
function pairingError(error) {
  const text = String(error?.message || error);
  const known = [
    ["SETUP_TEMPLATES_MISSING", "ชุดติดตั้งยังไม่ครบ ให้ติดตั้งหรืออัปเดตเครื่องหลักด้วยแพ็กเกจแบบเต็ม แล้วบันทึกชุดติดตั้งอีกครั้ง"],
    ["SETUP_TEMPLATE_MISMATCH", "ไฟล์ชุดติดตั้งไม่ตรงกับ release ที่ตรวจไว้ ให้ติดตั้งแพ็กเกจเครื่องหลักใหม่"],
    ["HOST_KEY_MISMATCH", "กุญแจยืนยันเครื่องไม่ตรงกับที่บันทึกไว้ ระบบหยุดเชื่อมต่อ กรุณาตรวจว่าเป็นเครื่องเดิมก่อน"],
    ["PAIRING", "รหัสจับคู่ใช้ไม่ได้กับคำขอนี้ ตรวจว่าคัดลอกครบจากเครื่องที่เพิ่งติดตั้งและใช้เครื่องหลักที่ถูกต้อง"],
    ["64 KiB", "รหัสยาวเกินขนาดที่รองรับ กรุณาคัดลอกเฉพาะรหัสจับคู่"],
  ];
  return known.find(([code]) => text.includes(code))?.[1] || text;
}
async function refreshPairings() {
  const rows = await rpc("pairing.status");
  if (!Array.isArray(rows)) throw new Error("ไม่สามารถอ่านรายการจับคู่ได้");
  state.pairings = rows;
  if (state.pairing) state.pairing = rows.find(item => item.requestId === state.pairing.requestId) || state.pairing;
}
function pairingStep(item) {
  if (!item) return 1;
  if (item.state === "ready") return 5;
  if (item.state === "needs_attention") return 4;
  return state.setupSaved?.requestId === item.requestId ? 3 : 2;
}
function renderStepper(step) {
  const steps = [["ตั้งค่าที่นี่", "กรอกข้อมูลเครื่องใหม่"], ["ส่งชุดติดตั้ง", "เปิดที่เครื่องใหม่แล้วกด Approve"], ["คัดลอกรหัส", "จากเครื่องใหม่หลังติดตั้งเสร็จ"], ["วางรหัสกลับ", "ตรวจแล้วพร้อมใช้งาน"]];
  return '<ol class="stepper">' + steps.map(([title, text], i) => {
    const n = i + 1, status = n < step ? "done" : n === step ? "current" : "";
    return '<li class="' + status + '"' + (status === "current" ? ' aria-current="step"' : "") + '><span class="step-num">' + (status === "done" ? icons.check : n) + "</span><div><strong>" + title + "</strong><span>" + text + "</span></div></li>";
  }).join("") + "</ol>";
}
function renderEnrollment() {
  const current = state.pairing;
  const groups = [...new Set(state.targets.map(item => item.tenant).filter(Boolean))];
  const requests = state.pairings.filter(item => item.state !== "ready");
  return heading("เพิ่มเครื่องใหม่", "สร้างชุดติดตั้ง ส่งไปเปิดที่เครื่องใหม่ แล้ววางรหัสจับคู่กลับมาที่นี่") + renderStepper(pairingStep(current)) +
    (current ? renderPairingProgress(current) : '<section class="surface"><h2>ข้อมูลเครื่องที่จะเพิ่ม</h2><form id="create-pairing" class="mt"><div class="form-grid">' +
      '<label>ชื่อเครื่อง<input name="label" required maxlength="256" placeholder="เช่น เครื่อง build"></label>' +
      '<label>กลุ่ม<input name="tenant" required value="' + esc(groups[0] || "lab") + '" list="pairing-groups" pattern="[A-Za-z0-9_\\-]+" maxlength="64"><datalist id="pairing-groups">' + groups.map(group => '<option value="' + esc(group) + '"></option>').join("") + "</datalist><small>ภาษาอังกฤษ ตัวเลข - หรือ _ เช่น lab, office, demo</small></label>" +
      '<label>ระบบปฏิบัติการ<select name="os"><option value="windows">Windows / Windows Server</option><option value="linux">Ubuntu 24.04 — Desktop หรือ Server</option></select></label>' +
      '<label>IP หรือชื่อเครื่อง<input name="address" required placeholder="เช่น 192.0.2.20 หรือชื่อใน Tailscale"><small>ที่อยู่ที่เครื่องหลักนี้เชื่อมต่อไปได้</small></label></div>' +
      '<details><summary>ตัวเลือกขั้นสูง</summary><div class="form-grid"><label>ชื่ออ้างอิงเครื่อง<input name="targetId" pattern="[A-Za-z0-9_\\-]+" maxlength="64" placeholder="สร้างให้อัตโนมัติ"></label><label>พอร์ต SSH<input name="port" type="number" value="22" min="1" max="65535" required></label><label>ประเภทเครือข่าย<select name="routeKind"><option value="">เลือกจากที่อยู่อัตโนมัติ</option><option value="lan">LAN</option><option value="tailscale">Tailscale</option><option value="public">Public IP</option></select></label><label>บัญชีเชื่อมต่อ<input name="user" value="sshcc-bridge" required></label><label class="full">เส้นทางสำรอง / ProxyJump<textarea name="routes" rows="4" class="mono" placeholder="เว้นว่างเพื่อใช้ค่าปกติ"></textarea><small>สำหรับผู้ดูแล: รายการเส้นทาง JSON ตามรูปแบบเดิม</small></label></div></details>' +
      '<div class="form-actions"><button type="submit" class="primary">สร้างชุดติดตั้งสำหรับเครื่องนี้</button></div><p class="help mt">ชุดติดตั้งมีเฉพาะข้อมูลสาธารณะ เครื่องใหม่ต้องกด Approve ก่อนจึงจะควบคุมได้ · ถ้าใช้ Tailscale ให้ลงชื่อเข้าใช้บนเครื่องใหม่ก่อน</p></form></section>') +
    '<section class="surface"><h2>วางรหัสจับคู่จากเครื่องใหม่</h2><p class="help">กด “คัดลอกรหัสจับคู่” ที่เครื่องใหม่ แล้ววางข้อความทั้งหมดที่นี่ ระบบจะจับคู่กับคำขอที่ถูกต้องให้เอง</p><form id="complete-pairing"><label>รหัสจับคู่<textarea name="code" class="mono" required maxlength="65536" rows="3" autocomplete="off" spellcheck="false" placeholder="SSHCC1.…"></textarea></label><div class="form-actions"><button type="submit" class="primary">ยืนยันและตรวจการเชื่อมต่อ</button></div></form></section>' +
    (requests.length ? '<section class="section mt"><div class="section-heading"><h2>คำขอที่ยังทำไม่เสร็จ</h2><button class="small" data-action="refresh-pairings">' + icons.refresh + 'รีเฟรช</button></div><div class="table-wrap"><table><thead><tr><th>เครื่อง</th><th>กลุ่ม</th><th>สถานะ</th><th></th></tr></thead><tbody>' + requests.map(item => "<tr><td><strong>" + esc(item.label || item.targetId) + "</strong><small>" + esc(timestamp(item.createdAt)) + "</small></td><td>" + chip(item.tenant) + '</td><td><span class="badge pending">' + esc(pairingState(item)) + '</span></td><td class="actions"><button class="small" data-action="resume-pairing" data-id="' + esc(item.requestId) + '">ทำต่อ</button></td></tr>').join("") + "</tbody></table></div></section>" : "") +
    '<details class="mt"><summary>สำหรับผู้ดูแล (นำเข้าด้วยไฟล์)</summary><div><p class="help">นำเข้า registration หรือ v1 registry ได้ โดยยังตรวจ host key เหมือนปกติ</p><div class="page-actions"><button data-action="import-registration">นำเข้าไฟล์ registration…</button><button data-action="import-v1-registry">นำเข้า v1 registry…</button></div><form id="paste-registration" class="mt"><label>Registration JSON<textarea name="document" class="mono" rows="4" required spellcheck="false"></textarea></label><div class="form-actions"><button type="submit">นำเข้า</button></div></form></div></details>';
}
function renderPairingProgress(item) {
  const env = item.envelope;
  const registered = state.targets.find(target => target.id === item.targetId);
  const routes = registered?.routes || env?.bundle?.routes || [];
  const ready = item.state === "ready", attention = item.state === "needs_attention";
  const failed = (item.diagnostics?.stages || []).filter(stage => !stage.ok);
  const master = env?.bundle?.master?.label || state.service?.label;
  return '<section class="surface"><div class="pairing-head"><h2>' + esc(item.label || item.targetId) + '</h2><span class="badge ' + (ready ? "good" : "pending") + '">' + esc(pairingState(item)) + "</span></div>" +
    '<dl class="facts"><dt>เครื่องหลัก</dt><dd>' + esc(master) + "</dd><dt>กลุ่ม</dt><dd>" + esc(item.tenant) + "</dd><dt>ที่อยู่เครื่อง</dt><dd>" + esc(routes.map(route => route.address).join(", ")) + "</dd></dl>" +
    (ready ? '<div class="callout good mt">' + icons.check + '<div><strong class="ready-message">ตรวจ service และตัวตนเครื่องผ่านแล้ว</strong><p>Agent ใช้งานเครื่องนี้ได้ทันที</p></div></div><div class="form-actions"><button class="primary" data-action="open-paired-target" data-id="' + esc(item.targetId) + '">เปิดเครื่องนี้</button><button data-action="new-pairing">' + icons.plus + "เพิ่มเครื่องถัดไป</button></div>" :
      attention ? '<div class="callout pending mt">' + icons.alert + "<div><strong>ติดตั้งแล้ว แต่ยังตรวจการเชื่อมต่อไม่ผ่าน</strong><p>ไม่ต้องติดตั้งใหม่ แก้ที่อยู่เครื่องแล้วตรวจอีกครั้ง</p>" + (failed.length ? "<ul>" + failed.map(stage => "<li>" + esc(pairingError(stage.message)) + "</li>").join("") + "</ul>" : "") + "</div></div>" +
        '<form id="retry-pairing" class="mt"><label>IP หรือชื่อเครื่อง<input name="address" required value="' + esc(routes[0]?.address || "") + '"><small>แก้เฉพาะเส้นทางเชื่อมต่อ โดยรักษาตัวตนและ host key เดิม</small></label><div class="form-actions"><button type="submit" class="primary">ตรวจอีกครั้ง</button><button type="button" data-action="new-pairing">เพิ่มเครื่องอื่น</button></div></form>' :
      '<p class="mt">บันทึกชุดติดตั้งแล้วส่งไปเครื่องใหม่ เมื่อเปิดจะเห็นชื่อเครื่องหลัก <strong>' + esc(master) + "</strong> ให้กด Approve แล้วคัดลอกรหัสกลับมาวางด้านล่าง</p>" +
        '<div class="form-actions"><button class="primary" data-action="save-setup">บันทึกชุดติดตั้งไฟล์เดียว…</button><button data-action="new-pairing">เพิ่มเครื่องอื่น</button></div>' +
        (state.setupSaved?.requestId === item.requestId ? '<div class="callout good mt">' + icons.check + "<div><strong>บันทึกแล้ว</strong><p class=\"wrap\">" + esc(state.setupSaved.path) + "</p></div></div>" : "") +
        '<p class="help mt">' + (item.os === "windows" ? "Windows: เปิดไฟล์ .exe แล้วกด Approve" : "Ubuntu: ติดตั้งไฟล์ .deb แล้วเปิด SSH Control Center Setup<br>Server: ใช้ sudo apt install --no-install-recommends ./ชื่อไฟล์.deb แล้วรัน sshcc-setup ใน terminal") + "</p>") +
    '<details><summary>รายละเอียดคำขอ</summary><p class="help">คำขอ: <code>' + esc(item.requestId) + "</code><br>การยกเลิกหรือปิดหน้าจอนี้ไม่ถอนสิทธิ์ที่เครื่องใหม่อนุมัติไปแล้ว</p></details></section>";
}
function renderMasters() {
  const rows = Array.isArray(state.masters) ? state.masters : [];
  return heading("Master ที่อนุญาต", "กำหนดว่าเครื่องไหนสั่งงานเครื่องนี้ได้ และรวมเครื่องลูกจาก Master อื่น", '<button data-action="refresh-masters">' + icons.refresh + "รีเฟรช</button>") +
    '<div class="stack"><section class="surface"><div class="card-head"><div><h2>Master ที่สั่งงานเครื่องนี้ได้</h2><p>มีผลเมื่อเครื่องนี้ทำหน้าที่ Target</p></div><button data-action="accept-enrollment" title="เลือก public enrollment bundle จาก Master อีกเครื่อง">' + icons.plus + "เพิ่ม Master…</button></div>" +
    (rows.length ? '<div class="table-wrap"><table><thead><tr><th>Master</th><th>สิทธิ์</th><th></th></tr></thead><tbody>' +
      rows.map(master => "<tr><td><strong>" + esc(master.label || master.id) + "</strong><small><code>" + esc(master.id) + '</code></small></td><td><span class="badge ' + (master.revoked ? "" : "good") + '">' + (master.revoked ? "ถอนสิทธิ์แล้ว" : "อนุญาต") + '</span></td><td class="actions">' + (master.revoked ? "" : '<button class="danger small" data-action="revoke-master" data-id="' + esc(master.id) + '">ถอนสิทธิ์</button>') + "</td></tr>").join("") +
      "</tbody></table></div>" : '<div class="empty-inline">ยังไม่มี Master ที่ลงทะเบียนบนเครื่องนี้</div>') +
    '<p class="help mt">ถอนสิทธิ์มีผลกับคำขอใหม่ งานที่เริ่มแล้วให้ยกเลิกจากหน้างานแยกต่างหาก</p></section>' +
    (state.service?.roles?.includes("master") ? renderMerges() : "") + renderRelays() + "</div>";
}
function mergeStatus(targetId) {
  if (state.merging.has(targetId)) return '<span class="badge running">กำลัง merge…</span>';
  const merge = state.merges.find(row => row.targetId === targetId);
  if (!merge) return '<span class="badge">ยังไม่ merge</span>';
  const count = (merge.children || []).length;
  const labels = { merged: ["good", "Merged · " + count + " เครื่อง"], outdated: ["bad", "ต้องอัปเกรด Master ปลายทาง"], "not-master": ["bad", "เครื่องนี้ไม่ได้เป็น Master"], disabled: ["bad", "Master ปลายทางปิดการ merge"], error: ["bad", "ผิดพลาด"] };
  const [style, text] = labels[merge.status] || ["bad", merge.status];
  const detail = merge.status === "merged" ? "sync ล่าสุด " + timestamp(merge.syncedAt) : (merge.error || "") + (count ? " · ยังใช้รายชื่อเดิม " + count + " เครื่อง" : "");
  return '<span class="badge ' + style + '">' + esc(text) + '</span><small class="wrap">' + esc(detail) + "</small>";
}
function reverseStatus(merge) {
  const running = state.twoWay.get(merge.targetId);
  if (running) return '<span class="badge running">กำลังเปิดสองทาง…</span><small class="wrap">' + esc(running.message) + "</small>";
  const reverse = merge.reverse, peer = state.targets.find(item => item.id === merge.targetId);
  const relay = state.relays.find(row => row.masterId === peer?.nodeId);
  if (isTwoWay(merge)) return '<span class="badge good">สองทาง</span><small>' + esc(merge.label || merge.targetId) + " สั่งเครื่องลูกของเครื่องนี้ได้</small>";
  if (reverse?.status === "two-way" && relay?.revoked) return '<span class="badge">ทางเดียว</span><small>ปิดการ merge ไว้ที่เครื่องนี้</small>';
  const labels = { "two-way": ["pending", "รอ merge กลับ"], "needs-attention": ["bad", "จับคู่แล้ว แต่ merge กลับไม่สำเร็จ"], error: ["bad", "เปิดสองทางไม่สำเร็จ"], pending: ["pending", "ทำไม่เสร็จ"] };
  const [style, text] = labels[reverse?.status] || ["", "ทางเดียว"];
  return '<span class="badge ' + style + '">' + text + "</span>" + (reverse?.error ? '<small class="wrap">' + esc(reverse.error) + "</small>" : !reverse ? "<small>เครื่องนี้สั่งเครื่องลูกของเครื่องนั้นได้ฝั่งเดียว</small>" : "");
}
function renderMerges() {
  const discovered = (state.discovered || []).filter(row => row.isMaster);
  const ids = [...new Set([...state.merges.map(row => row.targetId), ...discovered.map(row => row.targetId)])];
  const name = id => state.merges.find(row => row.targetId === id)?.label || discovered.find(row => row.targetId === id)?.label || state.targets.find(row => row.id === id)?.label || id;
  const unreachable = (state.discovered || []).filter(row => row.error).length;
  return '<section class="surface"><div class="card-head"><div><h2>รวมเครื่องลูกจาก Master อื่น</h2><p>Merge เพื่อสั่งเครื่องลูกของ Master นั้นผ่านเครื่องนั้น</p></div><button data-action="discover-masters">' + icons.search + "ค้นหา Master</button></div>" +
    (ids.length ? '<div class="table-wrap"><table><thead><tr><th>Master</th><th>สถานะ</th><th>ทิศทาง</th><th></th></tr></thead><tbody>' +
      ids.map(id => {
        const merge = state.merges.find(row => row.targetId === id), busyRow = state.merging.has(id) || state.twoWay.has(id);
        const canPair = merge?.status === "merged" && !isTwoWay(merge) && !(merge.reverse?.status === "two-way" && state.relays.some(row => row.masterId === state.targets.find(item => item.id === id)?.nodeId && row.revoked));
        const pair = canPair ? '<button class="primary small" data-action="open-two-way" data-id="' + esc(id) + '"' + (busyRow ? " disabled" : !/Windows/.test(navigator.userAgent) ? ' disabled title="ตอนนี้รองรับเฉพาะเครื่องหลักที่เป็น Windows"' : "") + ">" + (merge.reverse ? "ลองอีกครั้ง" : "เปิดสองทาง") + "</button>" : "";
        const actions = merge ? pair + '<button class="small" data-action="sync-merge" data-id="' + esc(id) + '"' + (busyRow ? " disabled" : "") + '>Sync</button><button class="danger small" data-action="remove-merge" data-id="' + esc(id) + '"' + (busyRow ? " disabled" : "") + ">ยกเลิก merge</button>" : '<button class="primary small" data-action="merge-master" data-id="' + esc(id) + '"' + (busyRow ? " disabled" : "") + ">Merge</button>";
        return "<tr><td><strong>" + esc(name(id)) + "</strong><small><code>" + esc(id) + "</code></small></td><td>" + mergeStatus(id) + "</td><td>" + (merge ? reverseStatus(merge) : '<span class="muted">—</span>') + '</td><td class="actions"><div class="target-actions">' + actions + "</div></td></tr>";
      }).join("") + "</tbody></table></div>"
      : '<div class="empty-inline">' + (state.discovered ? "ไม่พบเครื่องที่เป็น Master ในเครื่องที่เชื่อมต่อ" : "กด “ค้นหา Master” เพื่อตรวจเครื่องที่เชื่อมต่ออยู่") + "</div>") +
    (state.discovered ? '<p class="info-line">ตรวจ ' + state.discovered.length + " เครื่อง · เป็น Master " + discovered.length + " เครื่อง" + (unreachable ? " · ติดต่อไม่ได้ " + unreachable + " เครื่อง" : "") + "</p>" : "") +
    '<details><summary>merge ทำงานอย่างไร</summary><p class="help">เครื่องลูกที่ merge แล้วจะอยู่ในหน้าเครื่องทั้งหมด ใช้ ID แบบ <code>master/เครื่องลูก</code> · คำสั่งส่งผ่าน Master นั้นด้วยสิทธิ์ของ Master นั้น<br>เปิดสองทางเพื่อให้ Master นั้นสั่งเครื่องลูกของเครื่องนี้ได้ด้วย</p></details></section>';
}
function renderRelays() {
  if (!state.relays.length) return "";
  return '<section class="surface"><div class="card-head"><div><h2>Master ที่ merge เครื่องนี้</h2><p>Master เหล่านี้สั่งเครื่องลูกของเครื่องนี้ผ่านเครื่องนี้ได้</p></div></div><div class="table-wrap"><table><thead><tr><th>Master</th><th>ใช้ล่าสุด</th><th>สถานะ</th><th></th></tr></thead><tbody>' +
    state.relays.map(relay => "<tr><td><strong>" + esc(relay.label) + "</strong><small><code>" + esc(relay.masterId) + "</code></small></td><td>" + esc(timestamp(relay.lastUsedAt)) + '</td><td><span class="badge ' + (relay.revoked ? "bad" : "good") + '">' + (relay.revoked ? "ปิดการ merge" : "Merged") + '</span></td><td class="actions">' +
      (relay.revoked ? '<button class="small" data-action="allow-relay" data-id="' + esc(relay.masterId) + '">อนุญาตอีกครั้ง</button>' : '<button class="danger small" data-action="revoke-relay" data-id="' + esc(relay.masterId) + '">ปิดการ merge</button>') + "</td></tr>").join("") +
    "</tbody></table></div></section>";
}
function copyRow(command) {
  return '<div class="copy-row"><code>' + esc(command) + '</code><button class="icon-button" data-action="copy" data-copy="' + esc(command) + '" aria-label="คัดลอก ' + esc(command) + '" title="คัดลอก">' + icons.copy + "</button></div>";
}
function linkBadge(status) {
  const styles = { connected: "good", pathMismatch: "pending", disconnected: "", missingClient: "" };
  const labels = { connected: "เชื่อมแล้ว", pathMismatch: "path ไม่ตรง", disconnected: "ยังไม่เชื่อม", missingClient: "ไม่พบโปรแกรม" };
  return '<span class="badge ' + (styles[status] || "") + '">' + esc(labels[status] || status) + "</span>";
}
function renderClientLink(link) {
  const canConnect = link.detected && link.status === "disconnected";
  const canUpdate = link.detected && link.status === "pathMismatch";
  const actions =
    (canConnect ? '<button class="primary small" data-action="connect-client" data-id="' + esc(link.id) + '">เชื่อม</button>' : "") +
    (canUpdate ? '<button class="small" data-action="update-client-path" data-id="' + esc(link.id) + '">อัปเดต path</button>' : "") +
    (link.status === "connected" ? '<span class="help">' + icons.check + "พร้อมใช้</span>" : "") +
    (!link.detected ? '<button class="primary small" disabled title="ไม่พบโปรแกรมบนเครื่องนี้">เชื่อม</button>' : "");
  return '<article class="client-link surface">' +
    '<div class="card-head"><div><h3>' + esc(link.label) + "</h3><p>" + esc(link.message) + "</p></div>" + linkBadge(link.status) + "</div>" +
    '<dl class="facts"><dt>Config</dt><dd><code>' + esc(link.configPath) + "</code></dd>" +
    (link.command ? "<dt>Command ปัจจุบัน</dt><dd><code>" + esc(link.command) + "</code></dd>" : "") +
    "<dt>Command ที่ต้องการ</dt><dd><code>" + esc(link.expectedCommand) + "</code></dd></dl>" +
    '<div class="form-actions">' + actions + "</div>" +
    "<details><summary>คัดลอก snippet</summary><div>" + copyRow(link.snippet) + "</div></details>" +
    "</article>";
}
function renderClientLinks() {
  const body = state.clientLinksError
    ? '<div class="callout bad">' + icons.alert + "<div><strong>อ่านสถานะการเชื่อม MCP ไม่ได้</strong><p>" + esc(state.clientLinksError) + "</p></div></div>"
    : (state.clientLinks.length
      ? '<div class="client-link-grid">' + state.clientLinks.map(renderClientLink).join("") + "</div>"
      : '<p class="help">กำลังตรวจ Cursor, Claude Code และ Codex…</p>');
  return '<section class="section mt"><div class="section-heading"><div><h2>เชื่อม AI client</h2><p class="help">ลงทะเบียน MCP <code>ssh_control_center</code> ระดับผู้ใช้ · ใช้ local service เดียวกันผ่าน <code>sshcc mcp</code></p></div></div>' +
    body +
    '<details class="mt"><summary>CLI ทั่วไป</summary><div><p class="help">ใช้ตรวจ service โดยตรงโดยไม่ผ่าน AI client</p>' +
    copyRow("sshcc request system.status") + copyRow("sshcc mcp") + "</div></details></section>";
}
function renderService() {
  const service = state.service;
  const head = heading("Local service", "Service เริ่มตั้งแต่บูต และทำงานแยกจากหน้าต่างนี้", '<button data-action="refresh-service">' + icons.refresh + "ตรวจอีกครั้ง</button>");
  if (!service) {
    return head + '<div class="callout bad">' + icons.alert + "<div><strong>ติดต่อ service ไม่ได้</strong><p>ตรวจว่าติดตั้ง SSH Control Center แล้ว และเปิด tray ด้วยบัญชี owner ที่เลือกตอนติดตั้ง</p><ul><li>Windows: ดู service ชื่อ SSHControlCenter ใน Services</li><li>Ubuntu: ตรวจด้วย <code>systemctl status sshcc</code></li></ul></div></div>" +
      '<details><summary>ข้อความผิดพลาด</summary><div class="output light"><pre>' + esc(state.serviceError || "—") + "</pre></div></details>" +
      renderClientLinks();
  }
  return head + '<div class="stat-grid">' +
    stat("สถานะ", '<span class="dot good"></span>พร้อมใช้งาน') +
    stat("ชื่อเครื่อง", esc(service.label)) +
    stat("บทบาท", esc(service.roles?.join(" + ") || "—")) +
    stat("ทำงานต่อเนื่อง", esc(formatUptime(service.uptimeSeconds)), service.version ? "เวอร์ชัน " + service.version : "") +
    "</div>" +
    '<div class="disclosure-list"><details><summary>ข้อมูลเพิ่มเติม <small class="muted">Node ID, การเชื่อมต่อ</small></summary><div><dl class="facts"><dt>Node ID</dt><dd><code>' + esc(service.nodeId) + "</code></dd><dt>การเชื่อมต่อ</dt><dd>Local named pipe / Unix socket</dd><dt>หน้าต่างนี้</dt><dd>ปิดหน้าต่างเพื่อคืนหน่วยความจำ WebView หรือเลือกปิดไอคอน Tray เพื่อปิดเฉพาะ tray</dd></dl></div></details></div>" +
    renderClientLinks();
}
async function selectTarget(id) {
  state.selected = id; state.tab = "overview"; state.job = null; state.jobs = []; state.jobsError = "";
  state.files = null; state.file = null; state.pendingJob = null; state.newJob = false;
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
  if (state.page === "fleet" && state.tab === "jobs" && $("#job-list")) $("#job-list").innerHTML = renderJobRows();
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
  if (state.page === "fleet" && state.tab === "jobs" && $("#job-detail")) {
    const oldLog = $("#log-output"), scroll = oldLog?.scrollTop || 0;
    const wasAtBottom = !oldLog || oldLog.scrollHeight - scroll - oldLog.clientHeight < 40;
    $("#job-detail").innerHTML = renderJobDetail();
    if ($("#log-output")) $("#log-output").scrollTop = wasAtBottom ? $("#log-output").scrollHeight : scroll;
  }
}
async function importRegistration(registration) {
  const imported = await rpc("targets.import", { registration });
  await refreshFleet(); state.selected = imported.id || registration.id; state.tab = "overview"; state.page = "fleet";
  state.job = null; state.jobs = []; state.files = null; state.file = null;
  render(); notify("นำเข้า Target แล้ว");
}
async function browseFiles(path) {
  const targetId = state.selected;
  const result = await rpc("files.list", { targetId, path });
  if (targetId !== state.selected) return;
  state.files = { path, entries: Array.isArray(result) ? result : result.entries || [] };
  state.file = null;
  if (state.tab === "files" && state.page === "fleet") render();
}
async function readFile(path) {
  const targetId = state.selected;
  const result = await rpc("files.read", { targetId, path, offset: 0, limit: 262144 });
  if (targetId !== state.selected) return;
  const bytes = Uint8Array.from(atob(result.contentBase64), c => c.charCodeAt(0));
  let content, validUtf8 = true;
  try { content = new TextDecoder("utf-8", { fatal: true }).decode(bytes); }
  catch { validUtf8 = false; content = "ไฟล์นี้มีข้อมูล binary หรือไม่ใช่ UTF-8"; }
  if (content?.includes("\0")) { validUtf8 = false; content = "ไฟล์นี้มีข้อมูล binary"; }
  state.file = { path, content, sha256: result.sha256, editable: validUtf8 && result.eof, size: result.size };
  if (state.tab === "files" && state.page === "fleet") render();
}
function encodeBase64(text) {
  const bytes = new TextEncoder().encode(text);
  let raw = "";
  for (let i = 0; i < bytes.length; i += 8192) raw += String.fromCharCode(...bytes.subarray(i, i + 8192));
  return btoa(raw);
}
document.addEventListener("click", event => {
  if (event.target.closest("#notice-close")) { clearNotice(); return; }
  const nav = event.target.closest("[data-page]");
  if (nav) { void navigate(nav.dataset.page); return; }
  const button = event.target.closest("[data-action]");
  if (!button) return;
  void busy(button, async () => {
    const action = button.dataset.action;
    if (action === "open-shell") return openShell(button.dataset.id);
    if (action === "close-shell-dialog") { $("#shell-dialog").close(); return; }
    if (action === "close-two-way-dialog") { $("#two-way-dialog").close(); return; }
    if (action === "open-two-way") {
      const dialog = $("#two-way-dialog"), merge = state.merges.find(row => row.targetId === button.dataset.id);
      dialog.querySelectorAll(".two-way-peer").forEach(item => { item.textContent = merge?.label || button.dataset.id; });
      dialog.querySelector('[name="targetId"]').value = button.dataset.id;
      dialog.showModal(); return;
    }
    if (action === "copy") return copyText(button.dataset.copy);
    if (action === "copy-log") return copyText(state.log, "คัดลอก log แล้ว");
    if (action === "copy-diagnostics") return copyText(pretty(state.diagnostics.get(state.selected)), "คัดลอกผลตรวจแล้ว");
    if (action === "add") return navigate("enroll");
    if (action === "new-pairing") { state.pairing = null; state.setupSaved = null; render(); }
    if (action === "refresh-pairings") { await refreshPairings(); render(); }
    if (action === "resume-pairing") { state.pairing = state.pairings.find(item => item.requestId === button.dataset.id); render(); }
    if (action === "save-setup") {
      if (!state.pairing) return;
      const saved = await native("save_setup", { requestId: state.pairing.requestId });
      if (saved) { state.setupSaved = saved; render(); notify("บันทึกชุดติดตั้งแล้ว ส่งไฟล์นี้ไปเครื่องใหม่ได้เลย"); }
    }
    if (action === "open-paired-target") { state.page = "fleet"; await refreshFleet(); await selectTarget(button.dataset.id); }
    if (action === "back-fleet") { state.selected = null; render(); }
    if (action === "clear-filter") { state.query = ""; state.group = ""; state.status = ""; render(); }
    if (action === "status-filter") { state.status = button.dataset.status; $("#fleet-table").innerHTML = renderFleetView(); }
    if (action === "refresh-fleet") { await refreshFleet(); render(); await refreshHealth(true); }
    if (action === "fleet-view") {
      state.view = button.dataset.view; localStorage.setItem("sshcc.fleetView", state.view);
      document.querySelectorAll('[data-action="fleet-view"]').forEach(item => item.setAttribute("aria-pressed", String(item.dataset.view === state.view)));
      $("#fleet-table").innerHTML = renderFleetView();
    }
    if (action === "manage-merge") return navigate("masters");
    if (action === "select-target") await selectTarget(button.dataset.id);
    if (action === "target-tab") {
      state.tab = button.dataset.tab; render();
      if (state.tab === "jobs") await refreshJobs();
    }
    if (action === "toggle-new-job") { state.newJob = !state.newJob; render(); if (state.newJob) $('#start-job [name="cwd"]')?.focus(); }
    if (action === "diagnose") {
      const targetId = state.selected;
      const result = await rpc("targets.doctor", { targetId });
      state.diagnostics.set(targetId, { ...result, checkedAt: new Date().toISOString() });
      if (state.selected === targetId) { state.tab = "overview"; render(); }
      const selected = result?.result?.executor?.selected;
      const sessionNote = selected?.ownerSessionId ? " (session " + selected.ownerSessionId + ")" : "";
      notify(result.ok ? "เชื่อมต่อ Target service สำเร็จ" + sessionNote : "ตรวจพบปัญหา ดูรายละเอียดในผลตรวจการเชื่อมต่อ", !result.ok);
    }
    if (action === "remove-target") {
      const targetId = state.selected;
      if (!confirm("นำ Target " + targetId + " ออกจากทะเบียนของ Master นี้?")) return;
      await rpc("targets.remove", { targetId }); await refreshFleet(); render(); notify("นำ Target ออกจากทะเบียนแล้ว");
    }
    if (action === "import-registration") {
      const registration = await native("open_json");
      if (registration) await importRegistration(registration);
    }
    if (action === "import-v1-registry") {
      const document = await native("open_json");
      if (document) {
        const result = await rpc("targets.import_legacy", { document });
        await refreshFleet(); notify("นำเข้า v1 registry แล้ว: " + pretty(result));
      }
    }
    if (action === "save-enrollment") {
      const path = await native("save_json", { document: state.export });
      if (path) notify("บันทึก enrollment แล้ว: " + path);
    }
    if (action === "refresh-masters") { await refreshMasters(); render(); }
    if (action === "revoke-master") {
      if (!confirm("ถอนสิทธิ์ Master " + button.dataset.id + " จากเครื่องนี้?")) return;
      await rpc("masters.revoke", { masterId: button.dataset.id }); await refreshMasters(); render(); notify("ถอนสิทธิ์ Master แล้ว");
    }
    if (action === "discover-masters") {
      state.discovered = await rpc("merges.discover"); render();
      const found = state.discovered.filter(row => row.isMaster).length;
      notify(found ? "พบ Master " + found + " เครื่อง กด Merge เพื่อสั่งเครื่องลูกของเครื่องนั้น" : "ไม่พบเครื่องที่เป็น Master ในเครื่องที่เชื่อมต่อ");
    }
    if (action === "merge-master" || action === "sync-merge") {
      const targetId = button.dataset.id;
      state.merging.add(targetId); render();
      try {
        const merge = await rpc(action === "merge-master" ? "merges.add" : "merges.sync", { targetId });
        notify("Merge " + (merge.label || targetId) + " แล้ว · สั่งได้ " + merge.children.length + " เครื่อง");
      } finally {
        state.merging.delete(targetId);
        await refreshMasters(); await refreshFleet(); render();
      }
    }
    if (action === "remove-merge") {
      if (!confirm("ยกเลิก merge กับ " + button.dataset.id + "? เครื่องลูกจะหายจากรายการของ Master นี้")) return;
      await rpc("merges.remove", { targetId: button.dataset.id }); await refreshMasters(); await refreshFleet(); render(); notify("ยกเลิก merge แล้ว");
    }
    if (action === "revoke-relay" || action === "allow-relay") {
      const revoke = action === "revoke-relay";
      if (revoke && !confirm("ปิดการ merge ของ Master นี้? Master นั้นจะสั่งเครื่องลูกผ่านเครื่องนี้ไม่ได้จนกว่าจะอนุญาตอีกครั้ง")) return;
      await rpc(revoke ? "relays.revoke" : "relays.allow", { masterId: button.dataset.id }); await refreshMasters(); render();
      notify(revoke ? "ปิดการ merge แล้ว" : "อนุญาตการ merge อีกครั้งแล้ว");
    }
    if (action === "accept-enrollment") {
      const bundle = await native("open_json");
      if (!bundle) return;
      if (!confirm("อนุญาต Master " + (bundle.master?.label || bundle.master?.id || "") + " ให้สั่งงานด้วยสิทธิ์ระบบบนเครื่องนี้?")) return;
      await rpc("enrollment.accept", { bundle }); await refreshMasters(); render();
      notify("เพิ่ม Master แล้ว หากยังไม่มี SSH key ของ Master ใน authorized_keys ให้รัน installer ด้วย bundle นี้");
    }
    if (action === "refresh-service") { await refreshLocal(); render(); }
    if (action === "connect-client") {
      await native("client_connect", { client: button.dataset.id });
      await refreshLocal(); render();
      notify("เชื่อม " + (state.clientLinks.find(item => item.id === button.dataset.id)?.label || button.dataset.id) + " แล้ว · เปิด session ใหม่ของ client นั้นเพื่อโหลด tools");
    }
    if (action === "update-client-path") {
      await native("client_update_path", { client: button.dataset.id });
      await refreshLocal(); render();
      notify("อัปเดต path ของ " + (state.clientLinks.find(item => item.id === button.dataset.id)?.label || button.dataset.id) + " แล้ว · เปิด session ใหม่เพื่อโหลด tools");
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
      if (!confirm("ยกเลิกงาน " + jobId + " และ process ลูก?")) return;
      const job = await rpc("jobs.cancel", { targetId, jobId });
      if (targetId !== state.selected || jobId !== state.job?.id) return;
      state.job = job;
      await refreshJobs(); await refreshSelectedJob(); notify("ส่งคำสั่งยกเลิกแล้ว");
    }
    if (action === "parent-folder") await browseFiles(parentPath($("#file-path").value));
    if (action === "read-file") await readFile($("#file-path").value);
    if (action === "open-file") await readFile(button.dataset.path);
    if (action === "open-folder") await browseFiles(button.dataset.path);
    if (action === "mkdir") {
      const path = $("#file-path").value, targetId = state.selected;
      await rpc("files.mkdir", { targetId, path });
      if (targetId === state.selected) { await browseFiles(path); notify("สร้างโฟลเดอร์แล้ว"); }
    }
    if (action === "remove-file") {
      const path = button.dataset.path, targetId = state.selected, directory = state.files.path;
      if (!confirm("ลบ " + path + "? โฟลเดอร์ต้องว่าง")) return;
      await rpc("files.remove", { targetId, path });
      if (targetId === state.selected) { await browseFiles(directory); notify("ลบแล้ว"); }
    }
    if (action === "close-file") { state.file = null; $("#file-editor").innerHTML = ""; }
  });
});
document.addEventListener("input", event => {
  if (event.target.id === "fleet-search") { state.query = event.target.value; $("#fleet-table").innerHTML = renderFleetView(); }
});
document.addEventListener("change", event => {
  if (event.target.id === "group-filter") { state.group = event.target.value; $("#fleet-table").innerHTML = renderFleetView(); }
});
document.addEventListener("submit", event => {
  const form = event.target;
  event.preventDefault();
  const values = Object.fromEntries(new FormData(form));
  const submittedPairing = state.pairing;
  void busy($('button[type="submit"]', form), async () => {
    if (form.id === "open-shell-session") {
      $("#shell-dialog").close();
      return openShell(values.targetId, values.ownerSessionId);
    }
    if (form.id === "confirm-two-way") {
      $("#two-way-dialog").close();
      const targetId = values.targetId;
      state.twoWay.set(targetId, { step: "request", message: "กำลังเริ่ม" }); render();
      try {
        const result = await native("merge_two_way", { targetId });
        const label = state.merges.find(row => row.targetId === targetId)?.label || targetId;
        if (result.status === "two-way") notify("เปิดสองทางกับ " + label + " แล้ว · เครื่องนั้นสั่งเครื่องลูกของเครื่องนี้ได้ " + result.children + " เครื่อง");
        else notify("ลงทะเบียนกับ " + label + " แล้ว แต่ merge กลับยังไม่สำเร็จ: " + (result.error || result.status), true);
      } finally {
        state.twoWay.delete(targetId);
        await refreshMasters(); await refreshFleet(); render();
      }
    }
    if (form.id === "create-pairing") {
      const params = { label: values.label, tenant: values.tenant, os: values.os, address: values.address, user: values.user, port: Number(values.port) };
      if (values.targetId.trim()) params.targetId = values.targetId.trim();
      if (values.routeKind) params.routeKind = values.routeKind;
      if (values.routes.trim()) params.routes = JSON.parse(values.routes);
      state.pairing = await rpc("pairing.create", params);
      await refreshPairings(); render(); notify("คำขอพร้อมแล้ว บันทึกชุดติดตั้งเพื่อส่งไปเครื่องใหม่");
    }
    if (form.id === "complete-pairing") {
      state.pairing = await rpc("pairing.complete", { code: values.code.trim() });
      await refreshPairings(); await refreshFleet(); render();
      notify(state.pairing.state === "ready" ? "จับคู่สำเร็จ เครื่องพร้อมให้ agent ใช้งาน" : "ติดตั้งแล้ว แต่ยังเชื่อมต่อไม่ได้ ตรวจที่อยู่เครื่องแล้วลองอีกครั้ง", state.pairing.state !== "ready");
    }
    if (form.id === "retry-pairing") {
      if (!submittedPairing) return;
      const current = await rpc("targets.list");
      const registered = current.find(item => item.id === submittedPairing.targetId);
      const routes = (registered?.routes || submittedPairing.envelope.bundle.routes).map((route, i) => i === 0 ? { ...route, address: values.address.trim() } : route);
      const result = await rpc("pairing.status", { requestId: submittedPairing.requestId, retry: true, routes });
      if (state.pairing?.requestId === submittedPairing.requestId) state.pairing = result;
      await refreshPairings(); await refreshFleet(); render(); notify(result.state === "ready" ? "พร้อมใช้งานแล้ว" : "ยังตรวจไม่ผ่าน ดูรายละเอียดด้านบน", result.state !== "ready");
    }
    if (form.id === "edit-target") {
      const routes = JSON.parse(values.routes);
      if (!Array.isArray(routes) || !routes.length) throw new Error("Routes ต้องเป็น array ที่มีอย่างน้อยหนึ่งเส้นทาง");
      await rpc("targets.update", { target: { ...target(), label: values.label, routes } });
      await refreshFleet(); render(); notify("บันทึก Target แล้ว");
    }
    if (form.id === "paste-registration") await importRegistration(JSON.parse(values.document));
    if (form.id === "export-enrollment") {
      const routes = values.routes.trim() ? JSON.parse(values.routes) : [{ kind: values.kind, address: values.address, port: Number(values.port) }];
      if (!Array.isArray(routes) || !routes.length) throw new Error("Routes ต้องเป็น array ที่มีอย่างน้อยหนึ่งเส้นทาง");
      state.export = await rpc("enrollment.export", { targetId: values.targetId, label: values.label || values.targetId, tenant: values.tenant, os: values.os, user: values.user, routes });
      if (state.page === "enroll") {
        $("#enrollment-result").innerHTML = renderEnrollmentResult();
        $("#enrollment-result").scrollIntoView({ block: "nearest" });
      }
      notify("สร้าง public enrollment แล้ว");
    }
    if (form.id === "start-job") {
      const env = JSON.parse(values.env || "{}");
      if (!env || Array.isArray(env) || typeof env !== "object" || Object.values(env).some(value => typeof value !== "string")) throw new Error("Environment ต้องเป็น JSON object ที่มีค่าเป็น string");
      const params = { targetId: state.selected, script: values.script, cwd: values.cwd, shell: values.shell, env };
      if(values.ownerSessionId) params.ownerSessionId=values.ownerSessionId;
      if(values.asAdmin === 'on') params.asAdmin=true;
      const fingerprint = JSON.stringify(params);
      if (state.pendingJob?.fingerprint !== fingerprint) state.pendingJob = { fingerprint, jobId: crypto.randomUUID() };
      const jobId = state.pendingJob.jobId;
      let job;
      try { job = await rpc("jobs.start", { ...params, jobId }); }
      catch (error) { throw new Error(String(error) + "\nJob ID: " + jobId + " — ส่งคำสั่งเดิมซ้ำเพื่อเช็ก/รับงานเดิมโดยไม่รันซ้ำ"); }
      state.pendingJob = null;
      if (params.targetId !== state.selected) return;
      state.job = job; state.log = ""; state.offset = 0; state.eof = true; state.stream = "stdout"; state.newJob = false;
      render(); await refreshJobs(); await refreshSelectedJob(true); notify("เริ่มงาน " + job.id + " แล้ว");
    }
    if (form.id === "browse-files") await browseFiles(values.path);
    if (form.id === "save-file") {
      if (!state.file?.editable) return;
      const path = state.file.path, targetId = state.selected;
      await rpc("files.write", { targetId, path, contentBase64: encodeBase64(values.content), expectedSha256: state.file.sha256 });
      if (targetId === state.selected) { await readFile(path); notify("บันทึกไฟล์แล้ว"); }
    }
  });
});
$("#refresh-all").addEventListener("click", event => void busy(event.currentTarget, async () => {
  await refreshLocal();
  if (state.page === "fleet") await refreshFleet();
  if (state.page === "masters") await refreshMasters();
  if (state.page === "enroll") await refreshPairings();
  render();
}));
setInterval(async () => {
  if (document.hidden || state.polling || state.page !== "fleet" || state.tab !== "jobs" || !target()?.runnerInstalled) return;
  state.polling = true;
  try { await refreshJobs(); await refreshSelectedJob(); }
  catch (error) { notify("อัปเดตงานไม่ได้: " + error, true); }
  finally { state.polling = false; }
}, 5000);
setInterval(() => {
  if (document.hidden) return;
  void refreshLocal();
  refreshHealthIfDue();
}, 30000);
window.addEventListener("focus", refreshHealthIfDue);
window.__TAURI__?.event?.listen("merge-two-way", ({ payload }) => {
  if (!state.twoWay.has(payload.targetId) || payload.step === "done") return;
  state.twoWay.set(payload.targetId, payload);
  if (state.page === "masters") render();
});
window.__TAURI__?.event?.listen("management-window-shown", async () => {
  await refreshLocal();
  if (state.page !== "fleet" || target()) return;
  try { await refreshFleet(); if (state.page === "fleet" && !target()) render(); } catch (error) { notify(error, true); }
  void refreshHealth();
});
await refreshLocal();
try { await refreshFleet(); } catch (error) { notify(error, true); }
render();
void refreshHealth();
