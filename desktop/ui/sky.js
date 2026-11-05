/* The Inventory sky (DESIGN.md "Relic Night"). Built once, then only updated, so the halo's turn or the parallax
   survives the window's re-renders. app.js hands it a snapshot of the fleet; the sky never asks the Master itself.
   The fleet is the Stardust Halo: a ring of glitter around this Master (a silver relic on its hill), one bright
   spark per host. Real 3D points projected by hand on canvas 2D (no library, no WebGL, so it also draws in WebKitGTK
   without a GPU). It draws only while something moves: the arrival, a drag, its inertia, a host changing. Motion budget
   otherwise: the glint and a few sparks; the clock repaints the sky once a minute. CSP: no style="" anywhere, custom
   properties go through CSSOM. */
const REDUCED = matchMedia("(prefers-reduced-motion: reduce)").matches;
const MAX_SPARKS = 48; // hosts named on the halo; the rest still glitter in it

// Sky colours (zenith → horizon), glow and far ridge per phase of the day, and the hours they peak at.
const PHASES = {
  night:   [["#04050C", "#0A0C1E", "#151A3C", "#26305E"], "#6C7CFF", "#0C0F22"],
  predawn: [["#04050D", "#0C0D24", "#2A2052", "#6E3E7E"], "#C98AD8", "#120F2A"],
  sunrise: [["#05071A", "#141A42", "#4C3274", "#E2805C"], "#FF9A5C", "#181634"],
  day:     [["#050B22", "#0E2052", "#1E4494", "#4E8AD6"], "#9AD0FF", "#0E1A40"],
  late:    [["#060A20", "#121E4E", "#2A3A8A", "#6C72CC"], "#B0A8FF", "#101A3C"],
  golden:  [["#06061A", "#1A183A", "#5C3252", "#E28C4C"], "#FF8A40", "#16132C"],
  dusk:    [["#050514", "#151232", "#3C215A", "#8C3E7A"], "#C86AC8", "#110E28"]
};
// hour · phase · glow strength · stars · rays
const HOURS = [[0, "night", .2, 1, 0], [4.3, "predawn", .55, .95, 0], [6.2, "sunrise", .95, .3, .2], [8.5, "day", .5, 0, .55], [13, "day", .42, 0, .7],
  [16.3, "late", .5, 0, .5], [18.1, "golden", .9, .15, .15], [19.4, "dusk", .55, .6, 0], [21, "night", .22, 1, 0], [24, "night", .2, 1, 0]];
const STATE_COLOUR = { online: "var(--led)", offline: "var(--rose)", hot: "var(--amber)" };
const SPARK_RGB = { online: "255,238,214", offline: "255,127,174", hot: "255,194,102", checking: "201,205,226" };
const GLITTER = 5200, TONES = [[240, 160, 96], [255, 232, 200], [191, 216, 255], [255, 140, 90]];
const REST = { yaw: .35, pitch: .27 };
const BLEED = 24; // the halo canvases run this far past the plate on each side, so the parallax never opens an edge

const hex = c => [1, 3, 5].map(i => parseInt(c.slice(i, i + 2), 16));
const mix = (a, b, t) => "#" + hex(a).map((v, i) => Math.round(v + (hex(b)[i] - v) * t).toString(16).padStart(2, "0")).join("");
const lerp = (a, b, t) => a + (b - a) * t;
const ease = t => t * t * (3 - 2 * t);
const px = v => v + "px";
const set = (el, props) => { for (const k in props) el.style.setProperty(k, props[k]); return el; };
const make = (tag, cls, parent) => { const el = document.createElement(tag); if (cls) el.className = cls; if (parent) parent.append(el); return el; };

export function createSky({ onSelect }) {
  const root = make("section", "sky");
  root.id = "sky"; root.hidden = true; root.setAttribute("aria-hidden", "true");
  // As the prototype: a fixed night (CSS), scattered stars, the halo on one hill, grain and a vignette, the film edge, the headline
  const stars = make("canvas", "sky-stars", root);
  const back = make("canvas", "sky-halo", root), front = make("canvas", "sky-halo", root), bloom = make("canvas", "sky-halo bloom", root);
  make("div", "sky-vignette", root); make("div", "sky-grain", root);
  const film = make("div", "sky-film", root); const frames = make("span", "frames", film);
  const hint = make("p", "sky-hint", root);
  const headline = make("p", "sky-headline", root);
  const callout = make("div", "sky-callout", root); make("i", "stem", callout);
  const cBody = make("div", "body", callout), cName = make("span", "name", cBody), cState = make("span", "state", cBody), cFacts = make("dl", "", cBody);
  const shoot = make("div", "", root); set(shoot, { inset: "0", "pointer-events": "none", overflow: "hidden" });

  let T = null, snap = null, layoutKey = "", ring = [], byKey = new Map();
  let focusId = null, last = new Map(), hour = 0, clock = 0, parallax = { x: 0, y: 0 };

  /* ---------- the sky clock ---------- */
  function paint(h) {
    hour = h;
    let i = HOURS.findIndex(k => k[0] > h) - 1; if (i < 0) i = HOURS.length - 2;
    const A = HOURS[i], B = HOURS[i + 1], t = ease((h - A[0]) / (B[0] - A[0])), a = PHASES[A[1]], b = PHASES[B[1]], st = document.body.style;
    a[0].forEach((c, n) => st.setProperty("--sky" + n, mix(c, b[0][n], t)));
    st.setProperty("--glow", mix(a[1], b[1], t)); st.setProperty("--ridge-far", mix(a[2], b[2], t));
    st.setProperty("--gs", lerp(A[2], B[2], t).toFixed(3)); st.setProperty("--stars", lerp(A[3], B[3], t).toFixed(3)); st.setProperty("--rays", lerp(A[4], B[4], t).toFixed(3));
  }
  const nowHour = () => { const d = new Date(); return d.getHours() + d.getMinutes() / 60; };
  // the clock runs on every page: the thin sky strip on the other pages reads the same colours
  const tick = () => { paint(nowHour()); if (!root.hidden) writeFilm(); };
  const startClock = () => { if (!clock) clock = setInterval(tick, 60000); };
  const stopClock = () => { clearInterval(clock); clock = 0; };
  document.addEventListener("visibilitychange", () => document.hidden ? stopClock() : (tick(), startClock()));

  /* ---------- the halo: glitter in ring space (angle, radial offset, height), one spark per host ---------- */
  const rand = (seed => () => ((seed = (seed * 16807) % 2147483647) - 1) / 2147483646)(7);
  const G = new Float32Array(GLITTER * 4), GC = [], FROM = new Float32Array(GLITTER * 2);
  for (let i = 0; i < GLITTER; i++) {
    G[i * 4] = rand() * Math.PI * 2; G[i * 4 + 1] = (rand() + rand() + rand() - 1.5) * .075; G[i * 4 + 2] = (rand() - .5) * .035;
    G[i * 4 + 3] = rand() < .985 ? .5 + rand() * 1.1 : 1.8 + rand() * 1.6;
    const t = rand(), c = TONES[t < .45 ? 0 : t < .75 ? 1 : t < .93 ? 2 : 3]; GC.push(`rgba(${c[0]},${c[1]},${c[2]},${(.25 + rand() * .75).toFixed(2)})`);
    FROM[i * 2] = rand(); FROM[i * 2 + 1] = rand();
  }
  const cam = { yaw: REST.yaw, pitch: REST.pitch, vy: 0, vp: 0 };
  let W = 0, H = 0, DPR = 1, cx = 0, cy = 0, R = 0, arrive = REDUCED ? 1 : 0, arriveAt = 0, hover = null, changed = new Map();
  const project = (a, off, h) => {
    const r = 1 + off, x = Math.cos(a) * r, z = Math.sin(a) * r;
    const x1 = x * Math.cos(cam.yaw) - z * Math.sin(cam.yaw), z1 = x * Math.sin(cam.yaw) + z * Math.cos(cam.yaw);
    const y2 = h * Math.cos(cam.pitch) - z1 * Math.sin(cam.pitch), z2 = h * Math.sin(cam.pitch) + z1 * Math.cos(cam.pitch), s = 3.2 / (3.2 + z2);
    return [cx + x1 * R * s, cy + y2 * R * s, z2, s];
  };
  // This Master: the hosts sit evenly round the ring, merged Masters and Masters below them larger
  function build(s) {
    const list = [...s.direct.map(h => [h.id, h, 1]), ...s.peers.flatMap(p => [[p.id, p, 1.6], ...p.hosts.map(h => [p.id + "\u0000" + h.id, h, h.master ? 1.3 : 1])])].slice(0, MAX_SPARKS);
    // hosts evenly round the ring; the Masters (planets) spread apart, each half a step between two hosts
    const hosts = list.filter(([, , size]) => size === 1), masters = list.filter(([, , size]) => size > 1), step = Math.PI * 2 / Math.max(1, hosts.length);
    ring = [...hosts.map(([key, , size], i) => ({ key, size, a: -Math.PI / 2 + i * step })),
      ...masters.map(([key, , size], j) => ({ key, size, a: -Math.PI / 2 + (Math.round(j * hosts.length / masters.length) + .5) * step + .35 }))];
    byKey = new Map(ring.map(sp => [sp.key, sp]));
  }
  function size() {
    DPR = Math.min(2, devicePixelRatio || 1); const plate = root.clientWidth; W = plate + BLEED * 2; H = root.clientHeight;
    cx = BLEED + plate * .56; cy = H * .5; R = Math.min(plate * .3, H * .46);
    // the hill meets the sidebar's copy of the sky at the plate's left edge
    document.body.style.setProperty("--hill-y", hillAt(BLEED).toFixed(0) + "px");
    for (const c of [back, front]) { c.width = W * DPR; c.height = H * DPR; }
    bloom.width = W * DPR / 4; bloom.height = H * DPR / 4;
    stars.width = W * DPR; stars.height = H * DPR; scatter(stars.getContext("2d"), W, H, DPR, 3);
    const side = document.querySelector(".app-side"), sw = side ? side.clientWidth : 0;
    // the ember haze, in the sidebar's own coordinates, so its copy of the sky carries the same warmth to the edge
    const bs = document.body.style; bs.setProperty("--haze-x", (sw + plate * .56).toFixed(0) + "px"); bs.setProperty("--haze-y", (H * .5).toFixed(0) + "px");
    bs.setProperty("--haze-rx", (plate * .6).toFixed(0) + "px"); bs.setProperty("--haze-ry", (H * .7).toFixed(0) + "px");
    // the sidebar's hill line, at the curve's own slope at the seam (so a parallax shift keeps the two meeting); its box bleeds 24 px each side
    const slope = (hillAt(BLEED - 8) - hillAt(BLEED)) / 8;
    bs.setProperty("--hill-l", (hillAt(BLEED) + slope * (sw + 24)).toFixed(1) + "px"); bs.setProperty("--hill-r", (hillAt(BLEED) - slope * 24).toFixed(1) + "px");
    if (sw) { const c = document.createElement("canvas"); c.width = sw * DPR; c.height = H * DPR; scatter(c.getContext("2d"), sw, H, DPR, 5);
      document.body.style.setProperty("--side-stars", `url(${c.toDataURL()})`); }
  }
  // the prototype's stars: scattered, uneven, a few larger; thinning toward the hill
  function scatter(x, w, h, d, seed) {
    const s = (v => () => ((v = (v * 16807) % 2147483647) - 1) / 2147483646)(seed);
    x.setTransform(d, 0, 0, d, 0, 0); x.clearRect(0, 0, w, h);
    for (let i = 0, n = Math.round(w * h / 1440); i < n; i++) { const y = s() * h * .82, a = (.15 + s() * .6) * (1 - y / h * .6);
      x.fillStyle = `rgba(230,235,255,${a.toFixed(2)})`; x.beginPath(); x.arc(s() * w, y, s() < .96 ? .6 : 1.3, 0, 7); x.fill(); }
  }
  const crest = () => cy + R * .3;
  // the hill: rises from the left edge to the relic's crest, falls away to the right (the prototype's curve)
  const hillAt = x => { const top = crest(); if (x <= cx) { const t = x / cx; return top + R * .36 * (1 - t) ** 1.6; } const t = (x - cx) / (W - cx); return top + R * .4 * t ** 1.4; };
  function hill(x) {
    const top = crest(), g = x.createLinearGradient(0, top - 20, 0, H); g.addColorStop(0, "#161624"); g.addColorStop(1, "#06060B");
    x.globalCompositeOperation = "source-over"; x.fillStyle = g; x.beginPath(); x.moveTo(0, H);
    for (let px = 0; px <= W; px += 8) x.lineTo(px, hillAt(px)); x.lineTo(W, H); x.fill();
  }
  function relic(x) {
    const w = R * .07, h = R * .36, base = crest() + 4, g = x.createLinearGradient(cx - w / 2, 0, cx + w / 2, 0);
    g.addColorStop(0, "#6E7CA8"); g.addColorStop(.35, "#F4F6FB"); g.addColorStop(.6, "#A6B4D8"); g.addColorStop(1, "#3A4166");
    x.globalCompositeOperation = "source-over"; x.fillStyle = g; x.beginPath(); x.roundRect(cx - w / 2, base - h, w, h, [w / 2, w / 2, 2, 2]); x.fill();
    const s = (seed => () => ((seed = (seed * 16807) % 2147483647) - 1) / 2147483646)(11);
    for (let i = 0; i < 200; i++) { x.fillStyle = `rgba(255,${230 + s() * 25},${200 + s() * 55},${s().toFixed(2)})`; x.fillRect(cx - w / 2 + s() * w, base - h + s() * h, 1.2, 1.2); }
  }
  function flare(x, px, py, r, rgb, a) {
    const g = x.createRadialGradient(px, py, 0, px, py, r * 3); g.addColorStop(0, `rgba(${rgb},${a})`); g.addColorStop(1, `rgba(${rgb},0)`);
    x.fillStyle = g; x.beginPath(); x.arc(px, py, r * 3, 0, 7); x.fill();
    x.strokeStyle = `rgba(${rgb},${a * .8})`; x.lineWidth = .8; x.beginPath(); x.moveTo(px - r * 4, py); x.lineTo(px + r * 4, py); x.moveTo(px, py - r * 4); x.lineTo(px, py + r * 4); x.stroke();
  }
  /* A Master as a planet: a lit silver-violet sphere, a tilted ring (its far arc behind, near arc in front) and a glow
     in its status colour; dimmer when it is on the far side of the halo */
  function planet(x, g, px, py, r, st, far) {
    const rgb = SPARK_RGB[st], tilt = -.32, rx = r * 1.9, ry = r * .5;
    x.globalCompositeOperation = "lighter"; flare(g, px, py, r * .9, rgb, .7);
    const halo = x.createRadialGradient(px, py, r * .8, px, py, r * 1.9); halo.addColorStop(0, `rgba(${rgb},.32)`); halo.addColorStop(1, `rgba(${rgb},0)`);
    x.fillStyle = halo; x.beginPath(); x.arc(px, py, r * 1.9, 0, 7); x.fill();
    x.globalCompositeOperation = "source-over"; x.globalAlpha = far ? .7 : 1;
    const ring = (from, to) => { x.strokeStyle = "rgba(255,232,200,.75)"; x.lineWidth = Math.max(1, r * .09); x.beginPath(); x.ellipse(px, py, rx, ry, tilt, from, to); x.stroke();
      x.strokeStyle = "rgba(240,160,96,.45)"; x.lineWidth = Math.max(.6, r * .05); x.beginPath(); x.ellipse(px, py, rx * .84, ry * .84, tilt, from, to); x.stroke(); };
    ring(Math.PI, Math.PI * 2);
    const sphere = x.createRadialGradient(px - r * .38, py - r * .42, r * .08, px, py, r);
    sphere.addColorStop(0, "#F4F6FB"); sphere.addColorStop(.35, st === "offline" ? "#C9A0B8" : "#B8C2E4"); sphere.addColorStop(.75, st === "offline" ? "#4A2A44" : "#3A4166"); sphere.addColorStop(1, "#14162A");
    x.fillStyle = sphere; x.beginPath(); x.arc(px, py, r, 0, 7); x.fill();
    // faint bands and a rim in its status colour
    x.save(); x.beginPath(); x.arc(px, py, r, 0, 7); x.clip(); x.strokeStyle = "rgba(255,255,255,.12)"; x.lineWidth = r * .12;
    for (const k of [-.35, .1, .5]) { x.beginPath(); x.ellipse(px, py + r * k, r * 1.1, r * .22, tilt * .6, 0, 7); x.stroke(); } x.restore();
    x.strokeStyle = `rgba(${rgb},.85)`; x.lineWidth = 1.2; x.beginPath(); x.arc(px, py, r, 0, 7); x.stroke();
    ring(0, Math.PI); x.globalAlpha = 1; x.globalCompositeOperation = "lighter";
  }
  const eased = t => 1 - Math.pow(1 - t, 3);
  function frame(now) {
    if (arrive < 1) arrive = arriveAt ? Math.min(1, (now - arriveAt) / 1800) : 0;
    const k = eased(arrive), b = back.getContext("2d"), f = front.getContext("2d"), g = bloom.getContext("2d");
    for (const [x, d] of [[b, DPR], [f, DPR], [g, DPR / 4]]) { x.setTransform(d, 0, 0, d, 0, 0); x.clearRect(0, 0, W, H); x.globalCompositeOperation = "lighter"; }
    hill(f); relic(f); f.globalCompositeOperation = "lighter";
    for (let i = 0; i < GLITTER; i++) {
      const [x0, y0, z, s] = project(G[i * 4], G[i * 4 + 1], G[i * 4 + 2]), x = x0 + (FROM[i * 2] * W - x0) * (1 - k), y = y0 + (FROM[i * 2 + 1] * H - y0) * (1 - k);
      const c = z > 0 ? b : f, sz = G[i * 4 + 3] * s; c.fillStyle = GC[i]; c.fillRect(x - sz / 2, y - sz / 2, sz, sz);
      if (sz > 1.6 || i % 9 === 0) { g.fillStyle = GC[i]; g.fillRect(x - sz, y - sz, sz * 2, sz * 2); }
    }
    let busy = false;
    if (k > .6) for (const sp of ring) {
      const h = hostByKey(sp.key); if (!h) continue;
      const [x, y, z, s] = project(sp.a, 0, 0), c = z > 0 ? b : f, st = podState(h), hit = changed.get(sp.key);
      const swell = hit ? Math.max(0, 1 - (now - hit) / 1400) : 0; if (swell) busy = true;
      const r = 2 * sp.size * s * (sp.key === hover || sp.key === focusId ? 1.8 : 1) * (1 + swell * 1.5) * (k - .6) / .4;
        sp.x = x; sp.y = y;
      // a Master (merged, or one below it) is a planet on the halo; every other host a spark
      if (sp.size > 1) { const pr = R * (sp.size > 1.4 ? .11 : .07) * s * (sp.key === hover || sp.key === focusId ? 1.18 : 1) * (1 + swell * .4) * (k - .6) / .4;
        sp.r = pr; planet(c, g, x, y, pr, st, z > 0); }
      else { sp.r = 0; flare(c, x, y, r, SPARK_RGB[st], 1); flare(g, x, y, r * 1.6, SPARK_RGB[st], .9); }
      if (swell) { c.globalCompositeOperation = "lighter"; c.strokeStyle = `rgba(${SPARK_RGB[st]},${swell.toFixed(2)})`; c.lineWidth = 1.5; c.beginPath(); c.arc(x, y, 6 + (1 - swell) * 30, 0, 7); c.stroke(); }
      if (sp.size > 1 || st === "offline" || st === "hot" || sp.key === hover) {
        c.globalCompositeOperation = "source-over"; c.font = "400 10px 'Space Mono', monospace"; c.globalAlpha = z > 0 ? .55 : 1;
        c.fillStyle = st === "offline" ? "#FF7FAE" : st === "hot" ? "#FFC266" : "#DCE0F0";
        const text = h.label.toUpperCase();
        if (sp.r) { c.font = "700 11px 'Space Mono', monospace"; const w = c.measureText(text).width, right = x + sp.r * 1.9 + 6;
          // a planet near the plate's right edge names itself on its left
          c.fillText(text, right + w > W - BLEED - 12 ? x - sp.r * 1.9 - 6 - w : right, y + 4); } else c.fillText(text, x + 11, y - 9);
        c.globalAlpha = 1; c.globalCompositeOperation = "lighter";
      }
    }
    return busy || arrive < 1 && arriveAt > 0 || Math.abs(cam.vy) > .0004 || Math.abs(cam.vp) > .0004;
  }
  let raf = 0, dragging = false;
  const kick = () => { if (!raf && !document.hidden && !root.hidden) raf = requestAnimationFrame(step); };
  function step(now) {
    raf = 0; cam.yaw += cam.vy; cam.pitch = Math.max(.1, Math.min(.34, cam.pitch + cam.vp)); cam.vy *= .94; cam.vp *= .9;
    const more = frame(now); if (focusId) place(); if (more || dragging) kick();
  }
  document.addEventListener("visibilitychange", kick);
  // drag turns the halo, a point names a spark, a click opens its host
  // a planet under the pointer wins over a spark near it; otherwise the nearest spark within 22 px
  const pick = (x, y) => {
    const on = ring.find(sp => sp.r && sp.x != null && Math.hypot(sp.x - x, sp.y - y) < sp.r + 8); if (on) return on.key;
    let best = null, d = 22; for (const sp of ring) { if (sp.x == null) continue; const dd = Math.hypot(sp.x - x, sp.y - y); if (dd < d) { d = dd; best = sp.key; } } return best;
  };
  let lastX = 0, lastY = 0, moved = 0;
  const local = e => { const r = root.getBoundingClientRect(); return [e.clientX - r.left + BLEED - parallax.x, e.clientY - r.top - parallax.y]; };
  root.addEventListener("pointerdown", e => { if (e.button !== 0 || e.target.closest(".sky-callout")) return; if (arrive < 1) { arrive = 1; kick(); }
    dragging = true; moved = 0; lastX = e.clientX; lastY = e.clientY; root.setPointerCapture(e.pointerId); root.classList.add("is-dragging"); });
  root.addEventListener("pointermove", e => {
    if (dragging) { const dx = e.clientX - lastX, dy = e.clientY - lastY; moved += Math.abs(dx) + Math.abs(dy); lastX = e.clientX; lastY = e.clientY;
      if (REDUCED) { cam.yaw -= dx * .004; cam.pitch = Math.max(.1, Math.min(.34, cam.pitch + dy * .0025)); } else { cam.vy = -dx * .004; cam.vp = dy * .0025; } kick(); return; }
    const key = pick(...local(e)); if (key !== hover) { hover = key; root.classList.toggle("is-pointing", !!key); if (key) point(key.split("\u0000").pop()); kick(); }
  });
  root.addEventListener("pointerup", e => { dragging = false; root.classList.remove("is-dragging"); if (moved < 6) { const key = pick(...local(e)); if (key) onSelect(key.split("\u0000").pop()); } });
  root.addEventListener("dblclick", () => { cam.yaw = REST.yaw; cam.pitch = REST.pitch; cam.vy = cam.vp = 0; kick(); });

  /* ---------- state on the sparks, the headline and the film edge ---------- */
  const hostByKey = key => {
    if (!snap) return null;
    const [a, b] = key.split("\u0000");
    if (b) return snap.peers.find(p => p.id === a)?.hosts.find(h => h.id === b) || null;
    return snap.direct.find(h => h.id === a) || snap.peers.find(p => p.id === a) || null;
  };
  const podState = h => h.hot ? "hot" : h.state === "online" ? "online" : h.state === "offline" ? "offline" : "checking";
  function writeHeadline() {
    const parts = T.sky.headline(snap.online, snap.total);
    headline.replaceChildren();
    make("span", "n", headline).textContent = parts.n; make("span", "of", headline).textContent = parts.of; make("span", "on", headline).textContent = parts.on;
  }
  function writeFilm() {
    if (!snap || !T) return;
    const t = T.sky, two = n => String(n).padStart(2, "0");
    const items = [t.film.app(snap.master.version), "▸ " + t.film.online(two(snap.online), two(snap.total)), snap.master.label, snap.master.address,
      snap.checkedAt ? t.film.polled(snap.checkedAt) : t.film.checking, "◂ " + t.film.off(two(snap.offline))].filter(Boolean);
    // version, Master name and address are data, not wording (marked like #app-version for the language checks)
    const raw = new Set([items[0], snap.master.label, snap.master.address]);
    frames.replaceChildren(...items.map((text, i) => { const s = make("span", i === 1 ? "on" : ""); s.textContent = text; if (raw.has(text)) s.dataset.raw = ""; return s; }));
  }

  /* ---------- the callout follows the pointed host (or the first host that needs a look) ---------- */
  function point(id) {
    if (!snap) return;
    const key = [...byKey.keys()].find(k => k.split("\u0000").pop() === id);
    if (!key) return;
    focusId = key; kick();
    const h = hostByKey(key), s = podState(h);
    cName.textContent = h.label;
    cState.textContent = s === "hot" ? T.sky.hot : h.stateLabel; set(cState, { "--c": STATE_COLOUR[s] || "var(--lilac)" });
    const facts = [[T.sky.facts.address, h.address], [T.sky.facts.system, h.os], [T.sky.facts.response, h.response], [T.sky.facts.online, h.online], [T.sky.facts.checked, h.checked]].filter(([, v]) => v);
    cFacts.replaceChildren(...facts.flatMap(([k, v]) => { const dt = make("dt"); dt.textContent = k; const dd = make("dd"); dd.textContent = v; return [dt, dd]; }));
    requestAnimationFrame(place);
  }
  function place() {
    const sp = byKey.get(focusId); if (!sp || sp.x == null) return;
    const x = sp.x - BLEED; callout.classList.toggle("down", sp.y < 220); callout.classList.toggle("left", x > W - BLEED * 2 - 330);
    callout.style.transform = `translate(${x.toFixed(0)}px, ${sp.y.toFixed(0)}px)`;
    callout.classList.add("is-shown");
  }
  const defaultFocus = () => {
    const all = [...snap.direct, ...snap.peers.flatMap(p => p.hosts)];
    return (all.find(h => h.hot || h.state === "offline") || all[0])?.id;
  };

  /* ---------- events: a host that changes swells and rings once; a recovery sends a shooting star ---------- */
  function signal(key) { changed.set(key, performance.now()); kick(); }
  function shootingStar() {
    const s = make("i", "sky-star", shoot);
    set(s, { position: "absolute", left: (45 + Math.random() * 30) + "%", top: px(40 + Math.random() * 90) });
    s.animate([{ transform: "rotate(-16deg) translateX(0) scaleX(.15)", opacity: 0 }, { transform: "rotate(-16deg) translateX(-200px) scaleX(1)", opacity: 1, offset: .3 },
      { transform: "rotate(-16deg) translateX(-520px) scaleX(.5)", opacity: 0 }], { duration: 1700, easing: "cubic-bezier(.3,.1,.3,1)" }).finished.then(() => s.remove());
  }
  function flex() {
    const n = headline.querySelector(".n"); if (!n) return;
    n.animate([{ fontVariationSettings: '"SOFT" 0, "WONK" 0, "opsz" 144, "wght" 900', transform: "skewX(0)" },
      { fontVariationSettings: '"SOFT" 100, "WONK" 1, "opsz" 9, "wght" 500', transform: "skewX(-10deg)", offset: .4 },
      { fontVariationSettings: '"SOFT" 0, "WONK" 0, "opsz" 144, "wght" 900', transform: "skewX(0)" }], { duration: 900, easing: "cubic-bezier(.3,.7,.2,1)" });
  }
  function react(prev) {
    if (!prev || REDUCED || root.hidden) return;
    let recovered = false;
    for (const [key] of byKey) {
      const h = hostByKey(key); if (!h) continue;
      const before = last.get(key), now = podState(h);
      if (before && before !== now) { signal(key); if (before !== "online" && now === "online") recovered = true; }
    }
    if (recovered) setTimeout(shootingStar, 700);
    if (prev.online !== snap.online) flex();
  }

  /* ---------- parallax: one write per pointer move, eased by CSS, nothing while still ---------- */
  const LAYERS = [[stars, 4], [back, 13], [front, 13], [bloom, 13], [callout, 13]];
  let pending = 0;
  function drift(nx, ny) {
    for (const [el, d] of LAYERS) el.style.translate = `${(nx * d).toFixed(1)}px ${(ny * d * .5).toFixed(1)}px`;
    parallax = { x: nx * 13, y: ny * 6.5 };
    // the sidebar's copy of the sky moves with it: its stars as the plate's stars, its hill as the halo and hill
    const bs = document.body.style;
    bs.setProperty("--sx", (nx * 4).toFixed(1) + "px"); bs.setProperty("--sy", (ny * 2).toFixed(1) + "px");
    bs.setProperty("--hx", (nx * 13).toFixed(1) + "px"); bs.setProperty("--hy", (ny * 6.5).toFixed(1) + "px");
  }
  if (!REDUCED) {
    root.addEventListener("pointermove", e => {
      if (pending) return;
      pending = requestAnimationFrame(() => { const r = root.getBoundingClientRect(); drift(((e.clientX - r.left) / r.width - .5) * -2, ((e.clientY - r.top) / r.height - .5) * -2); pending = 0; });
    });
    root.addEventListener("pointerleave", () => drift(0, 0));
  }
  addEventListener("resize", () => { if (!root.hidden) { paint(hour); size(); kick(); } });

  paint(nowHour()); startClock();
  // The sidebar is sticky; its copy of the sky follows the page's scroll so the two never part (one write per frame, only while scrolling)
  let scrolling = 0;
  addEventListener("scroll", () => { if (scrolling) return; scrolling = requestAnimationFrame(() => { document.body.style.setProperty("--scroll", (-scrollY).toFixed(0) + "px"); scrolling = 0; }); }, { passive: true });

  /* ---------- public ---------- */
  function update(next, strings) {
    T = strings;
    document.body.classList.toggle("has-sky", Boolean(next?.visible));
    if (!next?.visible) { root.hidden = true; return; }
    const prev = snap, wasHidden = root.hidden; snap = next; root.hidden = false;
    const key = JSON.stringify([next.direct.map(h => h.id), next.peers.map(p => [p.id, p.hosts.map(h => h.id)])]);
    if (key !== layoutKey) { layoutKey = key; build(next); focusId = null; }
    if (wasHidden) { size(); if (!arriveAt && !REDUCED) arriveAt = performance.now(); }
    writeHeadline(); writeFilm(); hint.textContent = T.sky.hint; kick();
    if (wasHidden) { paint(nowHour()); startClock(); }
    react(wasHidden ? null : prev);
    // a host that just went offline or hot takes the callout
    const turned = wasHidden || !prev ? null : [...byKey.keys()].find(k => { const h = hostByKey(k), now = h && podState(h); return now && (now === "offline" || now === "hot") && last.get(k) !== now; });
    last = new Map([...byKey.keys()].map(k => { const h = hostByKey(k); return [k, h ? podState(h) : "checking"]; }));
    const keep = turned ? turned.split("\u0000").pop() : focusId && hostByKey(focusId) ? focusId.split("\u0000").pop() : defaultFocus();
    if (keep) point(keep); else callout.classList.remove("is-shown");
  }
  // paint(hour) also lets a test show any hour; the clock repaints the real one within a minute
  // keys() lists the hosts on the halo (for UI tests)
  return { el: root, update, point, paint, keys: () => [...byKey.keys()] };
}
