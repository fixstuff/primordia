// LifeAPI viewer: a read-only spectator. Talks to /ws on the same host.
// Frame layout: see crates/server/src/snapshot.rs.
"use strict";

const $ = (id) => document.getElementById(id);
const HEADER = 32, ORG = 20;

const S = {
  meta: null,          // latest JSON meta
  frame: null,         // latest parsed binary frame
  history: [],         // [{tick,total,lineages:[[id,n],...]}]
  follow: null,        // lineage id or null
  zoom: 1,
  chem: true,
  field: document.createElement("canvas"), // chemical field at native res
  dirty: true,
};

// Lineage hues stay clear of green (sugar) and blue (mineral), so organisms
// never vanish into the chemical field. Past 10 lineages, hues repeat at
// shifted lightness (see col).
const HUES = [330, 45, 280, 15, 60, 300, 0, 30, 265, 345];
const hue = (l) => HUES[(l - 1 + HUES.length) % HUES.length];
const col = (l, a = 1, light = 60) => `hsla(${hue(l)}, 75%, ${light}%, ${a})`;

// ---- connection -----------------------------------------------------------

function setConn(text, cls) {
  const c = $("conn");
  c.textContent = text;
  c.className = cls || "";
}

function connect() {
  const proto = location.protocol === "https:" ? "wss://" : "ws://";
  const ws = new WebSocket(proto + location.host + "/ws");
  ws.binaryType = "arraybuffer";
  ws.onopen = () => setConn("live", "live");
  ws.onclose = () => {
    setConn("reconnecting…", "down");
    setTimeout(connect, 1000);
  };
  ws.onmessage = (ev) => {
    if (typeof ev.data === "string") onMeta(JSON.parse(ev.data));
    else onFrame(ev.data);
  };
}

function onMeta(m) {
  if (m.history_reset) S.history = [];
  for (const p of m.history) S.history.push(p);
  S.meta = m;
  $("tick").textContent = m.tick.toLocaleString();
  $("pop").textContent = m.world.population.toLocaleString();
  $("births").textContent = m.world.births.toLocaleString();
  $("deaths").textContent = m.world.deaths.toLocaleString();
  renderLineages();
  S.dirty = true;
}

function onFrame(buf) {
  const dv = new DataView(buf);
  const magic = String.fromCharCode(dv.getUint8(0), dv.getUint8(1), dv.getUint8(2), dv.getUint8(3));
  if (magic !== "LAPI" || dv.getUint8(4) !== 1) {
    setConn("unknown frame format", "down");
    return;
  }
  const fw = dv.getUint16(16, true), fh = dv.getUint16(18, true);
  const n = dv.getUint32(28, true);
  const f = {
    tick: Number(dv.getBigUint64(8, true)),
    fw, fh,
    ww: dv.getFloat32(20, true), wh: dv.getFloat32(24, true),
    n,
    x: new Float32Array(n), y: new Float32Array(n), h: new Float32Array(n),
    lin: new Uint32Array(n), e: new Float32Array(n), r: new Float32Array(n),
  };
  const sugar = new Uint8Array(buf, HEADER, fw * fh);
  const mineral = new Uint8Array(buf, HEADER + fw * fh, fw * fh);
  let o = HEADER + 2 * fw * fh;
  for (let i = 0; i < n; i++, o += ORG) {
    f.x[i] = dv.getFloat32(o, true);
    f.y[i] = dv.getFloat32(o + 4, true);
    f.h[i] = dv.getFloat32(o + 8, true);
    f.lin[i] = dv.getUint32(o + 12, true);
    f.e[i] = dv.getUint16(o + 16, true) / 65535;
    f.r[i] = dv.getUint16(o + 18, true) / 100;
  }
  // Paint the chemical field once per frame at native resolution.
  const fc = S.field;
  fc.width = fw; fc.height = fh;
  const ctx = fc.getContext("2d");
  const img = ctx.createImageData(fw, fh);
  for (let i = 0; i < fw * fh; i++) {
    const s = sugar[i], m = mineral[i];
    img.data[i * 4] = 8 + (m * 0.10);
    img.data[i * 4 + 1] = 10 + s * 0.62;
    img.data[i * 4 + 2] = 18 + m * 0.75;
    img.data[i * 4 + 3] = 255;
  }
  ctx.putImageData(img, 0, 0);
  S.frame = f;
  $("empty").style.display = n === 0 && !S.meta?.lineages?.length ? "grid" : "none";
  if (n === 0) $("empty").textContent = "The pool is empty. Release a lineage through the API.";
  S.dirty = true;
}

// ---- drawing --------------------------------------------------------------

function fit(canvas) {
  const dpr = window.devicePixelRatio || 1;
  const w = Math.round(canvas.clientWidth * dpr), h = Math.round(canvas.clientHeight * dpr);
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
  return dpr;
}

const wrapDelta = (a, b, size) => {
  let d = b - a;
  if (d > size / 2) d -= size;
  else if (d < -size / 2) d += size;
  return d;
};

// Circular mean on a torus: where a lineage "is", even across the wrap.
function centre(f, lineage) {
  let sx = 0, cx = 0, sy = 0, cy = 0, k = 0;
  for (let i = 0; i < f.n; i++) {
    if (f.lin[i] !== lineage) continue;
    const ax = (f.x[i] / f.ww) * 2 * Math.PI, ay = (f.y[i] / f.wh) * 2 * Math.PI;
    sx += Math.sin(ax); cx += Math.cos(ax); sy += Math.sin(ay); cy += Math.cos(ay); k++;
  }
  if (!k) return null;
  const mx = ((Math.atan2(sx, cx) / (2 * Math.PI)) + 1) % 1;
  const my = ((Math.atan2(sy, cy) / (2 * Math.PI)) + 1) % 1;
  return [mx * f.ww, my * f.wh];
}

function drawPool() {
  const c = $("pool"), f = S.frame;
  fit(c);
  const g = c.getContext("2d");
  g.fillStyle = "#000";
  g.fillRect(0, 0, c.width, c.height);
  if (!f) return;
  let [vx, vy] = [f.ww / 2, f.wh / 2];
  if (S.follow !== null) {
    const m = centre(f, S.follow);
    if (m) [vx, vy] = m;
  }
  const scale = (c.width / f.ww) * S.zoom;
  const toX = (x) => c.width / 2 + wrapDelta(vx, x, f.ww) * scale;
  const toY = (y) => c.height / 2 + wrapDelta(vy, y, f.wh) * scale;

  if (S.chem) {
    g.imageSmoothingEnabled = true;
    const tw = f.ww * scale, th = f.wh * scale;
    const ox = c.width / 2 - vx * scale, oy = c.height / 2 - vy * scale;
    for (let tx = -1; tx <= 1; tx++)
      for (let ty = -1; ty <= 1; ty++)
        g.drawImage(S.field, ox + tx * tw, oy + ty * th, tw, th);
  }

  const dimOthers = S.follow !== null;
  for (let pass = 0; pass < 2; pass++) {
    for (let i = 0; i < f.n; i++) {
      const mine = f.lin[i] === S.follow;
      if (dimOthers && (pass === 0) === mine) continue; // others first, followed on top
      const x = toX(f.x[i]), y = toY(f.y[i]);
      const r = Math.max(1.5, f.r[i] * scale);
      if (x < -r || y < -r || x > c.width + r || y > c.height + r) continue;
      const a = dimOthers && !mine ? 0.22 : 0.95;
      g.fillStyle = col(f.lin[i], a, 30 + 40 * f.e[i]);
      g.beginPath();
      g.arc(x, y, r, 0, 2 * Math.PI);
      g.fill();
      if (r >= 3) {
        g.strokeStyle = `rgba(255,255,255,${a * 0.8})`;
        g.lineWidth = Math.max(1, r / 5);
        g.beginPath();
        g.moveTo(x, y);
        g.lineTo(x + Math.cos(f.h[i]) * r * 1.4, y + Math.sin(f.h[i]) * r * 1.4);
        g.stroke();
      }
    }
  }
}

function drawGraph() {
  const c = $("graph");
  const dpr = fit(c);
  const g = c.getContext("2d");
  g.clearRect(0, 0, c.width, c.height);
  const H = S.history;
  const css = getComputedStyle(document.documentElement);
  const muted = css.getPropertyValue("--muted").trim(), line = css.getPropertyValue("--line").trim();
  const padL = 44 * dpr, padB = 18 * dpr, padT = 6 * dpr, padR = 8 * dpr;
  const W = c.width - padL - padR, Hh = c.height - padT - padB;
  g.font = `${11 * dpr}px system-ui, sans-serif`;
  g.fillStyle = muted;
  if (H.length < 2) {
    g.fillText("Waiting for history…", padL, padT + 14 * dpr);
    return;
  }
  const t0 = H[0].tick, t1 = H[H.length - 1].tick;
  let ymax = 1;
  for (const p of H) ymax = Math.max(ymax, p.total);
  ymax = niceCeil(ymax);
  const X = (t) => padL + ((t - t0) / Math.max(1, t1 - t0)) * W;
  const Y = (v) => padT + Hh - (v / ymax) * Hh;
  // grid + labels
  g.strokeStyle = line;
  g.lineWidth = 1;
  for (const v of [0, ymax / 2, ymax]) {
    g.beginPath(); g.moveTo(padL, Y(v)); g.lineTo(padL + W, Y(v)); g.stroke();
    g.fillText(Math.round(v).toLocaleString(), 4 * dpr, Y(v) + 4 * dpr);
  }
  g.fillText(`tick ${t0.toLocaleString()}`, padL, c.height - 4 * dpr);
  const right = `tick ${t1.toLocaleString()}`;
  g.fillText(right, padL + W - g.measureText(right).width, c.height - 4 * dpr);
  // total
  path(g, H.map((p) => [X(p.tick), Y(p.total)]), "rgba(230,237,243,.55)", 1.5 * dpr);
  // per lineage
  const ids = new Set();
  for (const p of H) for (const [id] of p.lineages) ids.add(id);
  for (const id of ids) {
    const pts = H.map((p) => {
      const e = p.lineages.find(([l]) => l === id);
      return [X(p.tick), Y(e ? e[1] : 0)];
    });
    const on = S.follow === null || S.follow === id;
    path(g, pts, col(id, on ? 1 : 0.3), (on && S.follow === id ? 2.5 : 1.5) * dpr);
  }
}

function path(g, pts, stroke, w) {
  g.strokeStyle = stroke;
  g.lineWidth = w;
  g.lineJoin = "round";
  g.beginPath();
  pts.forEach(([x, y], i) => (i ? g.lineTo(x, y) : g.moveTo(x, y)));
  g.stroke();
}

function niceCeil(v) {
  const p = Math.pow(10, Math.floor(Math.log10(v)));
  for (const m of [1, 2, 2.5, 5, 10]) if (m * p >= v) return m * p;
  return 10 * p;
}

// ---- side panel -----------------------------------------------------------

function renderLineages() {
  const ul = $("lineages");
  const all = S.meta?.lineages || [];
  // Arenas create dozens of lineages: group by player (most alive first),
  // and fold extinct ones away behind a toggle.
  const alive = new Map();
  for (const l of all) alive.set(l.owner || "", (alive.get(l.owner || "") || 0) + l.alive);
  const extinct = all.filter((l) => l.alive === 0).length;
  const ls = all
    .filter((l) => S.showExtinct || l.alive > 0)
    .sort((a, b) => (alive.get(b.owner || "") - alive.get(a.owner || "")) ||
      (a.owner || "").localeCompare(b.owner || "") || b.alive - a.alive || a.lineage - b.lineage);
  const rows = [];
  let owner = null;
  for (const l of ls) {
    if ((l.owner || "") !== owner) {
      owner = l.owner || "";
      const h = document.createElement("li");
      h.className = "owner";
      h.textContent = `${owner || "unowned"} · ${alive.get(owner).toLocaleString()} alive`;
      rows.push(h);
    }
    rows.push(l);
  }
  if (extinct) {
    const t = document.createElement("li");
    t.className = "owner toggle";
    t.textContent = S.showExtinct ? `Hide ${extinct} extinct` : `Show ${extinct} extinct`;
    t.onclick = () => { S.showExtinct = !S.showExtinct; renderLineages(); };
    rows.push(t);
  }
  ul.replaceChildren(
    ...rows.map((l) => {
      if (l instanceof HTMLElement) return l;
      const li = document.createElement("li");
      li.className = (S.follow === l.lineage ? "on " : "") + (l.alive === 0 ? "dead" : "");
      li.title = l.alive === 0 ? "extinct" : "follow this lineage";
      const sw = document.createElement("span");
      sw.className = "sw";
      sw.style.background = col(l.lineage);
      const name = document.createElement("span");
      name.className = "ln";
      name.textContent = `#${l.lineage} ${l.species_name || ""} `;
      const owner = document.createElement("small");
      owner.textContent = l.owner ? `· ${l.owner}` : "";
      name.append(owner);
      const n = document.createElement("span");
      n.className = "n";
      n.textContent = l.alive === 0 ? "extinct" : l.alive.toLocaleString();
      li.append(sw, name, n);
      li.onclick = () => {
        S.follow = S.follow === l.lineage ? null : l.lineage;
        renderLineages();
        S.dirty = true;
      };
      return li;
    })
  );
  if (!ls.length) {
    const li = document.createElement("li");
    li.className = "dead";
    li.textContent = "No lineages yet.";
    ul.append(li);
  }
}

for (const b of document.querySelectorAll("[data-zoom]")) {
  b.onclick = () => {
    S.zoom = Number(b.dataset.zoom);
    for (const o of document.querySelectorAll("[data-zoom]")) o.setAttribute("aria-pressed", String(o === b));
    S.dirty = true;
  };
}
$("chem").onclick = (e) => {
  S.chem = !S.chem;
  e.currentTarget.setAttribute("aria-pressed", String(S.chem));
  S.dirty = true;
};
window.addEventListener("resize", () => (S.dirty = true));

function loop() {
  if (S.dirty) {
    S.dirty = false;
    drawPool();
    drawGraph();
  }
  requestAnimationFrame(loop);
}

// Link parameters: ?follow=<lineage>&zoom=<1|2|4>&chem=0
{
  const q = new URLSearchParams(location.search);
  if (q.has("follow")) S.follow = Number(q.get("follow"));
  if (["1", "2", "4"].includes(q.get("zoom"))) {
    S.zoom = Number(q.get("zoom"));
    for (const o of document.querySelectorAll("[data-zoom]")) o.setAttribute("aria-pressed", String(o.dataset.zoom === q.get("zoom")));
  }
  if (q.get("chem") === "0") {
    S.chem = false;
    $("chem").setAttribute("aria-pressed", "false");
  }
}

renderLineages();
connect();
loop();
