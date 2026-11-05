// Writes a side-by-side EN / ไทย review page of every on-screen string (desktop/ui/strings.js) to
// artifacts/language-ui/strings-review.html. A review tool, not part of the app: open the file in a browser, write a
// fix next to any Thai string, then "Copy my fixes" and paste them back. Strings with values show the values as
// {name}. Run: node desktop/tests/strings-review.cjs
const fs = require("node:fs");
const path = require("node:path");
const { pathToFileURL } = require("node:url");
const out = path.resolve(__dirname, "../../artifacts/language-ui/strings-review.html");

// Where each section of strings.js shows up, in Thai and English.
const PLACES = {
  side: "แถบด้านข้างและกรอบหน้าต่าง · Sidebar and window frame", common: "ใช้ร่วมกัน (ปุ่ม แจ้งเตือน) · Shared", dialogs: "หัวข้อหน้าต่างเลือกไฟล์ · Native file dialogs",
  shell: "หน้าต่างเลือก session · Session dialog", status: "ป้ายสถานะ · Status badges", inv: "เครื่องทั้งหมด ผังเครื่อง ผังเครือข่าย · Inventory, tree, topology",
  host: "หน้าเครื่อง / Master / โฟลเดอร์ / ดาต้าเซ็นเตอร์ · Host, Master, folder and datacenter pages", check: "ผลตรวจการเชื่อมต่อ · Connection check",
  jobs: "แท็บงานและล็อก · Jobs & logs tab", files: "แท็บไฟล์ อัปโหลด ลบไฟล์ · Files tab, upload, delete dialog", service: "หน้าบริการและ AI client · Service page",
  containers: "แท็บคอนเทนเนอร์ · Containers tab", monitor: "แท็บติดตามการใช้งาน · Monitoring tab", tree: "แจ้งเตือนและยืนยันในผังเครื่อง · Tree notices and confirmations",
  menu: "เมนูคลิกขวา · Right-click menus", twoWayDialog: "หน้าต่างเปิดสองทาง · Enable two-way dialog", errors: "ประโยคแจ้งข้อผิดพลาด · Error sentences",
  add: "เพิ่มเครื่อง · Add host", masters: "หน้าเครื่องหลัก (Masters) · Masters page",
};
const esc = value => String(value ?? "").replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
/** A string, or a function shown with its parameters as {name}. */
function show(value) {
  if (typeof value !== "function") return String(value);
  const names = (/^\s*(?:function[^(]*)?\(?([^)=]*)\)?\s*=>/.exec(value.toString())?.[1] || "").split(",").map(name => name.trim()).filter(Boolean);
  try { return String(value(...names.map(name => "{" + name + "}"))); } catch { return value.toString(); }
}
/** Every leaf as [key, value]; error tables are [code, sentence] pairs keyed by their code. */
function leaves(node, prefix = "") {
  if (Array.isArray(node) && node.every(row => Array.isArray(row) && row.length === 2)) return node.map(([code, text]) => [prefix + "." + code, text]);
  if (node && typeof node === "object" && !Array.isArray(node)) return Object.entries(node).flatMap(([key, value]) => leaves(value, prefix ? prefix + "." + key : key));
  if (Array.isArray(node)) return node.map((value, i) => [prefix + "." + i, value]);
  return [[prefix, node]];
}

(async () => {
  const { strings } = await import(pathToFileURL(path.resolve(__dirname, "../ui/strings.js")));
  const thai = new Map(leaves(strings.th));
  const rows = leaves(strings.en).filter(([key]) => key !== "locale").map(([key, en]) => ({ key, place: PLACES[key.split(".")[0]] || key.split(".")[0], en: show(en), th: thai.has(key) ? show(thai.get(key)) : null }));
  let section = "";
  const body = rows.map(row => {
    const head = row.place !== section ? '<tr class="section"><th colspan="4">' + esc((section = row.place)) + "</th></tr>" : "";
    return head + '<tr data-key="' + esc(row.key) + '"><td class="key"><code>' + esc(row.key) + "</code></td><td lang=\"en\">" + esc(row.en) + "</td>" +
      (row.th === null ? '<td class="missing">— ยังไม่มีคำแปล —</td>' : '<td lang="th">' + esc(row.th) + "</td>") +
      '<td><textarea rows="1" placeholder="แก้เป็น…" aria-label="แก้คำแปลของ ' + esc(row.key) + '"></textarea></td></tr>';
  }).join("\n");
  const page = `<!doctype html>
<html lang="th">
<head>
<meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>รีวิวคำแปล EN / ไทย</title>
<style>
  :root { --bg: #2B1A6B; --panel: #331F7A; --rule: #4B3A8F; --white: #F4F1FF; --lilac: #B7AEE0; --acid: #A6F26C; --rose: #FF8FA3; color-scheme: dark; }
  body { margin: 0; background: var(--bg); color: var(--white); font: 15px/1.5 system-ui, "Segoe UI", "Noto Sans Thai", sans-serif; }
  header { position: sticky; top: 0; z-index: 1; background: var(--bg); border-bottom: 1px solid var(--rule); padding: 16px; display: flex; flex-wrap: wrap; gap: 12px; align-items: center; }
  h1 { margin: 0; font-size: 22px; font-weight: 600; flex: 1 1 260px; }
  header p { margin: 0; color: var(--lilac); font-size: 13px; flex-basis: 100%; }
  input[type=search] { flex: 1 1 220px; padding: 8px 10px; background: transparent; color: inherit; border: 1px solid var(--rule); font: inherit; }
  button { padding: 8px 14px; background: var(--acid); color: #1A1033; border: 0; font: inherit; font-weight: 600; cursor: pointer; }
  main { padding: 0 16px 40px; overflow-x: auto; }
  table { width: 100%; border-collapse: collapse; min-width: 760px; }
  td, th { text-align: left; vertical-align: top; padding: 8px; border-bottom: 1px solid var(--rule); }
  tr.section th { padding-top: 22px; color: var(--acid); font-size: 14px; font-weight: 600; border-bottom-color: var(--acid); }
  td.key { width: 16%; } td.key code { color: var(--lilac); font-size: 12px; overflow-wrap: anywhere; }
  td[lang] { width: 30%; } td.missing { color: var(--rose); }
  textarea { width: 100%; box-sizing: border-box; min-height: 36px; background: var(--panel); color: inherit; border: 1px solid var(--rule); font: inherit; resize: vertical; }
  textarea:not(:placeholder-shown) { border-color: var(--acid); }
  #count { color: var(--lilac); font-size: 13px; }
</style>
</head>
<body>
<header>
  <h1>รีวิวคำแปล EN / ไทย · ${rows.length} ข้อความ</h1>
  <input type="search" id="filter" placeholder="ค้นหา (อังกฤษ ไทย หรือ key)…" aria-label="ค้นหา">
  <span id="count"></span>
  <button type="button" id="copy">คัดลอกที่แก้</button>
  <p>ข้อความที่มีค่าแทรก จะแสดงค่าเป็น {name} · พิมพ์คำแปลที่ถูกในช่องขวาสุด แล้วกด “คัดลอกที่แก้” เพื่อวางกลับมาให้ Claude ใส่ลงโค้ด · ที่พิมพ์ไว้จะจำไว้ในเบราว์เซอร์นี้</p>
</header>
<main><table><thead><tr><th>Key</th><th>English</th><th>ไทย (ตอนนี้)</th><th>แก้เป็น</th></tr></thead><tbody>
${body}
</tbody></table></main>
<script>
  const KEY = "sshcc.stringsReview";
  let saved = {}; try { saved = JSON.parse(localStorage.getItem(KEY) || "{}"); } catch {}
  const rows = [...document.querySelectorAll("tr[data-key]")];
  for (const row of rows) { const box = row.querySelector("textarea"); box.value = saved[row.dataset.key] || ""; }
  const count = () => { document.getElementById("count").textContent = "แก้แล้ว " + rows.filter(row => row.querySelector("textarea").value.trim()).length + " ข้อ"; };
  count();
  document.addEventListener("input", event => {
    if (event.target.id === "filter") {
      const query = event.target.value.toLowerCase();
      for (const row of rows) row.hidden = query && !row.textContent.toLowerCase().includes(query) && !row.querySelector("textarea").value.toLowerCase().includes(query);
      return;
    }
    const row = event.target.closest("tr[data-key]"); if (!row) return;
    const value = event.target.value.trim(); if (value) saved[row.dataset.key] = value; else delete saved[row.dataset.key];
    try { localStorage.setItem(KEY, JSON.stringify(saved)); } catch {}
    count();
  });
  document.getElementById("copy").addEventListener("click", async () => {
    const text = rows.filter(row => row.querySelector("textarea").value.trim()).map(row => row.dataset.key + "\\n  EN: " + row.children[1].textContent + "\\n  TH: " + row.children[2].textContent + "\\n  แก้เป็น: " + row.querySelector("textarea").value.trim()).join("\\n\\n");
    try { await navigator.clipboard.writeText(text || "(ยังไม่ได้แก้)"); document.getElementById("copy").textContent = "คัดลอกแล้ว"; } catch { prompt("คัดลอกข้อความนี้", text); }
  });
</script>
</body>
</html>
`;
  fs.mkdirSync(path.dirname(out), { recursive: true });
  fs.writeFileSync(out, page);
  const missing = rows.filter(row => row.th === null).length;
  console.log(out + " · " + rows.length + " strings" + (missing ? " · " + missing + " without Thai" : ""));
})().catch(error => { console.error(error); process.exit(1); });
