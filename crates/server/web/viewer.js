// LifeAPI viewer: a read-only spectator. Talks to /ws on the same host.
// Frame layout: see crates/server/src/snapshot.rs.
"use strict";

const $ = (id) => document.getElementById(id);
const HEADER = 32, ORG = 36;
const FRAME_V = 4;

// A server restarted onto a newer build sends a newer frame format than this
// page understands: reload to get the matching viewer (at most once a minute,
// so a real mismatch can't loop).
function staleViewer(serverV) {
  let last = 0;
  try { last = Number(sessionStorage.getItem("lifeapi-reload") || 0); } catch (_) {}
  if (Date.now() - last > 60000) {
    try { sessionStorage.setItem("lifeapi-reload", String(Date.now())); } catch (_) {}
    location.reload();
    return;
  }
  setConn(`viewer out of date (server frame v${serverV}): reload the page`, "down");
  $("empty").style.display = "grid";
  $("empty").textContent = "This page is older than the server. Reload the page (Ctrl+Shift+R).";
}
const ZOOM_MIN = 1, ZOOM_MAX = 32;

const S = {
  meta: null,          // latest JSON meta
  frame: null,         // latest parsed binary frame
  history: [],         // [{tick,total,lineages:[[id,n],...]}]
  follow: null,        // lineage id or null
  track: null,         // one organism's id (low 32 bits) or null
  bursts: [],          // [{x, y, t0}] where an organism was killed
  events: [],          // newest first: html strings for the event feed
  seenSpecies: new Set(),
  insp: null,          // latest /inspect reply for the tracked organism
  pan: null,           // [x, y] world point the user dragged to, or null
  view: null,          // last drawn transform, for mouse -> world
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
  if (m.frame_version && m.frame_version !== FRAME_V) return staleViewer(m.frame_version);
  renderSplash(m);
  if (m.history_reset) S.history = [];
  for (const p of m.history) S.history.push(p);
  S.meta = m;
  // Body plan per species (bodies can evolve, so draw each organism's own).
  S.bodies = new Map();
  for (const s of m.species || []) {
    S.bodies.set(s.species, s.body);
    if (s.origin && !S.seenSpecies.has(s.species) && S.meta) event(`<b class="evo">new body</b> ${s.name}: ${s.origin.replace(/^[^:]*: /, "")}`);
    S.seenSpecies.add(s.species);
  }
  $("kills").textContent = (m.world.kills || 0).toLocaleString();
  $("species").textContent = `${m.world.species_alive || 0}` + (m.world.evolved_alive ? ` (${m.world.evolved_alive} evolved)` : "");
  $("settings").textContent = `combat ${m.world.combat || "off"} · evolution ${m.world.body_evolution ? Math.round(m.world.body_evolution * 100) + "%" : "off"}`;
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
  if (magic !== "LAPI" || dv.getUint8(4) !== FRAME_V) {
    if (magic === "LAPI") staleViewer(dv.getUint8(4));
    else setConn("unknown frame format", "down");
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
    id: new Uint32Array(n), sp: new Uint32Array(n),
    flags: new Uint32Array(n), target: new Uint32Array(n),
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
    f.id[i] = dv.getUint32(o + 20, true);
    f.sp[i] = dv.getUint32(o + 24, true);
    f.flags[i] = dv.getUint32(o + 28, true);
    f.target[i] = dv.getUint32(o + 32, true);
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
  noteFights(S.frame, f);
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

// Organisms at least this many pixels in radius are drawn with their parts.
const DETAIL_PX = 4;
const SENSOR_COL = { sugar: "#4ade80", mineral: "#60a5fa", organism: "#fde047" };

// One organism drawn from its genome: wiggling flagella out the back, a cilia
// fringe, the membrane (lineage colour, brightness = energy), vacuoles inside,
// mouths as dark notches at the front, and chemo-sensor antennae at their real
// angles, tipped green (sugar) or blue (mineral).
function drawOrganism(g, x, y, r, h, body, lin, e, a, seed) {
  const t = performance.now() / 1000;
  const lw = Math.max(1, r / 6);
  const at = (ang, d) => [x + Math.cos(ang) * d, y + Math.sin(ang) * d];

  // Flagella: one tail each behind the body, longer with power, waving.
  const nf = body.flagella.length;
  g.strokeStyle = col(lin, a * 0.9, 75);
  g.lineWidth = lw;
  body.flagella.forEach((pw, k) => {
    const base = h + Math.PI + (k - (nf - 1) / 2) * 0.45;
    const len = r * (0.8 + 1.8 * pw);
    const [bx, by] = at(base, r * 0.95);
    const px = -Math.sin(base), py = Math.cos(base);
    g.beginPath();
    g.moveTo(bx, by);
    for (let s = 1; s <= 8; s++) {
      const u = s / 8, d = len * u;
      const wv = Math.sin(t * 9 + seed + u * 6 + k) * r * 0.28 * u;
      g.lineTo(bx + Math.cos(base) * d + px * wv, by + Math.sin(base) * d + py * wv);
    }
    g.stroke();
  });

  // Cilia: a fringe of short hairs, denser with more cilia power.
  const cp = body.cilia.reduce((s, v) => s + v, 0);
  if (body.cilia.length) {
    const nh = Math.round(8 + 12 * Math.min(cp, 2));
    g.strokeStyle = `rgba(255,255,255,${a * 0.45})`;
    g.lineWidth = Math.max(0.6, lw * 0.6);
    g.beginPath();
    for (let k = 0; k < nh; k++) {
      const ang = h + (k / nh) * 2 * Math.PI;
      const sway = Math.sin(t * 7 + seed + k) * 0.35;
      const [ax, ay] = at(ang, r);
      g.moveTo(ax, ay);
      g.lineTo(ax + Math.cos(ang + sway) * r * 0.3, ay + Math.sin(ang + sway) * r * 0.3);
    }
    g.stroke();
  }

  // Spikes: triangles out from the rim, longer with power.
  if (body.spikes > 0) {
    const ns = 12, len = r * (0.25 + 0.35 * body.spikes);
    g.fillStyle = `rgba(226,232,240,${a * 0.85})`;
    g.beginPath();
    for (let k = 0; k < ns; k++) {
      const ang = h + ((k + 0.5) / ns) * 2 * Math.PI, w = Math.PI / ns * 0.6;
      const [p1x, p1y] = at(ang - w, r * 0.95), [p2x, p2y] = at(ang + w, r * 0.95), [tx, ty] = at(ang, r + len);
      g.moveTo(p1x, p1y); g.lineTo(tx, ty); g.lineTo(p2x, p2y);
    }
    g.fill();
  }

  // Membrane. Camouflage makes the body faint.
  const ba = body.camouflage ? a * 0.35 : a;
  g.fillStyle = col(lin, ba, 30 + 40 * e);
  g.beginPath();
  g.arc(x, y, r, 0, 2 * Math.PI);
  g.fill();
  g.strokeStyle = col(lin, ba, 80);
  g.lineWidth = Math.max(0.8, r / 10);
  g.stroke();
  // Shell: a thick hard rim.
  if (body.shell > 0) {
    g.strokeStyle = `rgba(214,188,140,${ba * 0.95})`;
    g.lineWidth = Math.max(1.5, r * (0.08 + 0.2 * body.shell));
    g.beginPath();
    g.arc(x, y, r * 0.92, 0, 2 * Math.PI);
    g.stroke();
  }
  // Toxin: a warning ring of bright dashes.
  if (body.toxin > 0) {
    g.strokeStyle = `rgba(163,230,53,${ba * (0.5 + 0.5 * body.toxin)})`;
    g.lineWidth = Math.max(1, r / 8);
    g.setLineDash([Math.max(2, r / 3), Math.max(2, r / 4)]);
    g.beginPath();
    g.arc(x, y, r * 0.6, 0, 2 * Math.PI);
    g.stroke();
    g.setLineDash([]);
  }

  // Vacuoles: pale blobs inside, bigger with capacity.
  const nv = Math.min(body.vacuoles.length, 8);
  g.fillStyle = `rgba(255,255,255,${a * 0.35})`;
  for (let k = 0; k < nv; k++) {
    const ang = h + Math.PI + (k + 0.5) * (2 * Math.PI / nv) * 0.5 - Math.PI / 4 * (nv > 1 ? 1 : 0);
    const [vx, vy] = at(ang, nv > 1 ? r * 0.45 : r * 0.2);
    g.beginPath();
    g.arc(vx, vy, r * (0.1 + 0.18 * Math.min(body.vacuoles[k], 100) / 100), 0, 2 * Math.PI);
    g.fill();
  }

  // Mouths: dark notches at the front.
  g.fillStyle = `rgba(0,0,0,${a * 0.7})`;
  for (let k = 0; k < body.mouths; k++) {
    const [mx, my] = at(h + (k - (body.mouths - 1) / 2) * 0.55, r * 0.78);
    g.beginPath();
    g.arc(mx, my, r * 0.22, 0, 2 * Math.PI);
    g.fill();
  }

  // Jaw: two mandibles at the front that snap open and shut.
  if (body.jaw > 0) {
    const len = r * (0.5 + 0.6 * body.jaw), open = 0.35 + 0.25 * Math.abs(Math.sin(t * 6 + seed));
    g.strokeStyle = `rgba(254,226,226,${a})`;
    g.lineWidth = Math.max(1.2, r / 5);
    g.lineCap = "round";
    for (const side of [-1, 1]) {
      const [bx, by] = at(h + side * 0.5, r * 0.9);
      const tipA = h + side * open * 0.5;
      g.beginPath();
      g.moveTo(bx, by);
      g.quadraticCurveTo(bx + Math.cos(h + side * open) * len, by + Math.sin(h + side * open) * len,
                         x + Math.cos(tipA) * (r + len), y + Math.sin(tipA) * (r + len));
      g.stroke();
    }
    g.lineCap = "butt";
  }
  // Stinger: a sharp barb straight ahead.
  if (body.stinger > 0) {
    const len = r * (0.6 + 0.8 * body.stinger), w = r * 0.18;
    const [bx, by] = at(h, r * 0.85), [tx, ty] = at(h, r + len);
    const px = -Math.sin(h) * w, py = Math.cos(h) * w;
    g.fillStyle = `rgba(251,191,36,${a})`;
    g.beginPath();
    g.moveTo(bx + px, by + py); g.lineTo(tx, ty); g.lineTo(bx - px, by - py);
    g.fill();
  }

  // Sensors: antennae at their angle, tip coloured by chemical.
  for (const [deg, chem] of body.sensors) {
    const ang = h + (deg * Math.PI) / 180;
    const [sx, sy] = at(ang, r);
    const [tx, ty] = at(ang, r * 1.7);
    g.strokeStyle = `rgba(255,255,255,${a * 0.7})`;
    g.lineWidth = Math.max(0.7, lw * 0.7);
    g.beginPath();
    g.moveTo(sx, sy);
    g.lineTo(tx, ty);
    g.stroke();
    g.fillStyle = SENSOR_COL[chem] || "#fff";
    g.globalAlpha = a;
    g.beginPath();
    g.arc(tx, ty, Math.max(1.2, r * 0.16), 0, 2 * Math.PI);
    g.fill();
    g.globalAlpha = 1;
  }
}

function drawPool() {
  const c = $("pool"), f = S.frame;
  fit(c);
  const g = c.getContext("2d");
  g.fillStyle = "#000";
  g.fillRect(0, 0, c.width, c.height);
  if (!f) return;
  let [vx, vy] = S.pan || [f.ww / 2, f.wh / 2];
  const ti = S.track === null ? -1 : f.id.indexOf(S.track);
  if (ti >= 0) [vx, vy] = [f.x[ti], f.y[ti]];
  else if (S.follow !== null) {
    const m = centre(f, S.follow);
    if (m) [vx, vy] = m;
  }
  const scale = (c.width / f.ww) * S.zoom;
  S.view = { vx, vy, scale, w: c.width, h: c.height };
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
      const body = S.bodies?.get(f.sp[i]);
      if (body && r >= DETAIL_PX) {
        drawOrganism(g, x, y, r, f.h[i], body, f.lin[i], f.e[i], a, f.x[i] + f.y[i]);
        S.animating = true;
        continue;
      }
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
  drawFights(g, f, toX, toY, scale);
  drawTracked(g, f, toX, toY, scale);
}

// ---- combat and evolution, visible at any zoom ------------------------------

// Between two frames: an organism that was under attack and is now gone was
// killed. Leave a burst where it fell, and log it.
function noteFights(prev, f) {
  if (!prev) return;
  const now = new Set(f.id);
  for (let i = 0; i < prev.n; i++) {
    if (!(prev.flags[i] & 2) || now.has(prev.id[i])) continue;
    S.bursts.push({ x: prev.x[i], y: prev.y[i], t0: performance.now() });
    // Who was biting it last frame?
    let killer = -1;
    for (let k = 0; k < prev.n; k++) if (prev.target[k] === prev.id[i]) { killer = k; break; }
    const victim = nameOf(prev, i);
    event(killer >= 0 ? `<b class="atk">kill</b> ${nameOf(prev, killer)} killed ${victim}` : `<b class="atk">kill</b> ${victim} died fighting`);
  }
  if (S.bursts.length > 200) S.bursts.splice(0, S.bursts.length - 200);
}
function nameOf(f, i) {
  const l = S.meta?.lineages?.find((x) => x.lineage === f.lin[i]);
  const sp = S.meta?.species?.find((x) => x.species === f.sp[i]);
  return `<span style="color:${col(f.lin[i])}">${sp?.name || l?.species_name || "#" + f.lin[i]}</span>` + (l?.owner ? ` <small>(${l.owner})</small>` : "");
}
function event(html) {
  S.events.unshift(html);
  S.events.length = Math.min(S.events.length, 40);
  const ul = $("events");
  if (!ul) return;
  ul.innerHTML = S.events.map((e) => `<li>${e}</li>`).join("");
}

function drawFights(g, f, toX, toY, scale) {
  const now = performance.now();
  const idx = new Map();
  for (let i = 0; i < f.n; i++) idx.set(f.id[i], i);
  let any = false;
  for (let i = 0; i < f.n; i++) {
    const fl = f.flags[i];
    if (!fl) continue;
    const x = toX(f.x[i]), y = toY(f.y[i]), r = Math.max(2, f.r[i] * scale);
    if (fl & 4) {
      // Evolved body: a gold halo.
      g.strokeStyle = "rgba(250,204,21,0.85)";
      g.lineWidth = Math.max(1, r / 6);
      g.beginPath();
      g.arc(x, y, r * 1.35 + 1, 0, 2 * Math.PI);
      g.stroke();
    }
    if (fl & 1) {
      const k = idx.get(f.target[i]);
      if (k !== undefined) {
        g.strokeStyle = "rgba(239,68,68,0.9)";
        g.lineWidth = Math.max(2, r / 3);
        g.beginPath();
        g.moveTo(x, y);
        g.lineTo(toX(f.x[k]), toY(f.y[k]));
        g.stroke();
      }
      any = true;
    }
    if (fl & 2) {
      const p = 1 + 0.3 * Math.sin(now / 80 + i);
      g.strokeStyle = "rgba(239,68,68,0.95)";
      g.lineWidth = Math.max(1.5, r / 4);
      g.beginPath();
      g.arc(x, y, r * 1.6 * p + 2, 0, 2 * Math.PI);
      g.stroke();
      any = true;
    }
  }
  // Kill bursts fade over a second and a half.
  S.bursts = S.bursts.filter((b) => now - b.t0 < 1500);
  for (const b of S.bursts) {
    const u = (now - b.t0) / 1500, x = toX(b.x), y = toY(b.y);
    const rr = (6 + 30 * u) * Math.max(1, Math.sqrt(S.zoom));
    g.strokeStyle = `rgba(248,113,113,${1 - u})`;
    g.lineWidth = 3;
    g.beginPath();
    g.arc(x, y, rr, 0, 2 * Math.PI);
    g.stroke();
    g.beginPath();
    g.moveTo(x - rr / 2, y - rr / 2); g.lineTo(x + rr / 2, y + rr / 2);
    g.moveTo(x + rr / 2, y - rr / 2); g.lineTo(x - rr / 2, y + rr / 2);
    g.stroke();
    any = true;
  }
  if (any) S.animating = true;
}

// The tracked organism: a ring, and its sensors' sample points with what they
// read (brighter = more), so you can see what it "smells".
function drawTracked(g, f, toX, toY, scale) {
  const ti = S.track === null ? -1 : f.id.indexOf(S.track);
  if (ti < 0) return;
  const x = toX(f.x[ti]), y = toY(f.y[ti]), r = Math.max(3, f.r[ti] * scale);
  const pulse = 1 + 0.15 * Math.sin(performance.now() / 200);
  g.strokeStyle = "rgba(255,255,255,0.9)";
  g.lineWidth = 2;
  g.setLineDash([6, 4]);
  g.beginPath();
  g.arc(x, y, r * 1.9 * pulse, 0, 2 * Math.PI);
  g.stroke();
  g.setLineDash([]);
  const i = S.insp;
  if (i && !i.gone && i.id % 4294967296 === S.track) {
    const h = f.h[ti];
    for (const s of i.sensors) {
      const a = h + (s.angle_deg * Math.PI) / 180;
      const reach = (f.r[ti] + 2) * scale;
      const sx = x + Math.cos(a) * reach, sy = y + Math.sin(a) * reach;
      const rgb = { sugar: "74,222,128", mineral: "96,165,250", organism: "253,224,71" }[s.chemical] || "255,255,255";
      g.strokeStyle = `rgba(${rgb},${0.3 + 0.7 * s.input})`;
      g.lineWidth = 1 + 3 * s.input;
      g.beginPath();
      g.moveTo(x, y);
      g.lineTo(sx, sy);
      g.stroke();
    }
    // A fight: red line to whoever it is biting, orange from whoever bites it.
    for (const [other, colr] of [[i.fight.target, "rgba(248,113,113,0.95)"], [i.fight.attacker, "rgba(251,146,60,0.95)"]]) {
      if (!other) continue;
      const k = f.id.indexOf(other % 4294967296);
      if (k < 0) continue;
      g.strokeStyle = colr;
      g.lineWidth = 3;
      g.beginPath();
      g.moveTo(x, y);
      g.lineTo(toX(f.x[k]), toY(f.y[k]));
      g.stroke();
    }
    // Where the brain is steering: an arrow, length = next tick's speed.
    const na = h + (i.next.turn_deg * Math.PI) / 180;
    const len = r + Math.max(0, i.next.speed) * scale * 12;
    g.strokeStyle = "rgba(255,255,255,0.95)";
    g.lineWidth = 2;
    g.beginPath();
    g.moveTo(x, y);
    g.lineTo(x + Math.cos(na) * len, y + Math.sin(na) * len);
    g.stroke();
  }
  S.animating = true;
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
      const extra = [];
      if (l.kills) extra.push(`⚔ ${l.kills} kills`);
      if (l.killed) extra.push(`☠ ${l.killed} lost`);
      if (l.bodies > 1) extra.push(`${l.bodies} bodies`);
      if (extra.length) {
        const ex = document.createElement("small");
        ex.className = "ex";
        ex.textContent = extra.join(" · ");
        name.append(document.createElement("br"), ex);
      }
      li.append(sw, name, n);
      li.onclick = () => {
        S.follow = S.follow === l.lineage ? null : l.lineage;
        S.track = null; S.pan = null; renderTrack();
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

function setZoom(z) {
  S.zoom = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, z));
  for (const o of document.querySelectorAll("[data-zoom]"))
    o.setAttribute("aria-pressed", String(Math.abs(Number(o.dataset.zoom) - S.zoom) < 0.01));
  $("zoomval").textContent = S.zoom < 10 ? S.zoom.toFixed(1) + "×" : Math.round(S.zoom) + "×";
  S.dirty = true;
}
for (const b of document.querySelectorAll("[data-zoom]")) b.onclick = () => setZoom(Number(b.dataset.zoom));

// ---- mouse: wheel zoom toward the cursor, drag to pan, click to track -----

const pool = $("pool");
function mouseWorld(ev) {
  const v = S.view, f = S.frame;
  if (!v || !f) return null;
  const dpr = window.devicePixelRatio || 1;
  const px = ev.offsetX * dpr, py = ev.offsetY * dpr;
  const wrap = (a, n) => ((a % n) + n) % n;
  return [wrap(v.vx + (px - v.w / 2) / v.scale, f.ww), wrap(v.vy + (py - v.h / 2) / v.scale, f.wh), px, py];
}
pool.addEventListener("wheel", (ev) => {
  ev.preventDefault();
  const m = mouseWorld(ev), v = S.view;
  const z0 = S.zoom;
  setZoom(z0 * Math.pow(1.0015, -ev.deltaY));
  // Keep the point under the cursor fixed (unless something is being followed).
  if (m && v && S.track === null && S.follow === null) {
    const k = v.scale * (S.zoom / z0);
    S.pan = [m[0] - (m[2] - v.w / 2) / k, m[1] - (m[3] - v.h / 2) / k];
  }
}, { passive: false });

let drag = null;
pool.addEventListener("pointerdown", (ev) => {
  const v = S.view;
  if (!v) return;
  drag = { x: ev.offsetX, y: ev.offsetY, vx: v.vx, vy: v.vy, moved: false };
  pool.setPointerCapture(ev.pointerId);
});
pool.addEventListener("pointermove", (ev) => {
  if (!drag || !S.view) return;
  const dx = ev.offsetX - drag.x, dy = ev.offsetY - drag.y;
  if (!drag.moved && Math.hypot(dx, dy) < 5) return;
  drag.moved = true;
  const dpr = window.devicePixelRatio || 1;
  S.track = null; S.follow = null;
  S.pan = [drag.vx - (dx * dpr) / S.view.scale, drag.vy - (dy * dpr) / S.view.scale];
  renderTrack();
  renderLineages();
  S.dirty = true;
});
pool.addEventListener("pointerup", (ev) => {
  const d = drag;
  drag = null;
  if (!d || d.moved) return;
  // A click: track the organism under the cursor (nearest within reach).
  const m = mouseWorld(ev), f = S.frame;
  if (!m || !f) return;
  const reach = 12 / S.view.scale;
  let best = -1, bd = Infinity;
  for (let i = 0; i < f.n; i++) {
    const dx = wrapDelta(m[0], f.x[i], f.ww), dy = wrapDelta(m[1], f.y[i], f.wh);
    const dd = Math.hypot(dx, dy) - f.r[i];
    if (dd < bd) { bd = dd; best = i; }
  }
  if (best >= 0 && bd <= reach) {
    startTracking(f.id[best]);
    if (S.zoom < 6) setZoom(6);
  }
});
window.addEventListener("keydown", (ev) => { if (ev.key === "Escape") stopTracking(); });

// ---- tracking one organism ------------------------------------------------

function startTracking(id) {
  S.track = id;
  S.insp = null;
  S.follow = null;
  S.pan = null;
  renderLineages();
  pollInspect();
  S.dirty = true;
}
function stopTracking() {
  S.track = null;
  S.insp = null;
  renderTrack();
  S.dirty = true;
}
$("untrack").onclick = stopTracking;

let inspTimer = null;
async function pollInspect() {
  clearTimeout(inspTimer);
  if (S.track === null) return;
  const id = S.track;
  try {
    const r = await fetch(`/inspect?id=${id}`);
    const j = await r.json();
    if (S.track !== id) return;
    if (j.gone) {
      if (S.insp && !S.insp.gone) S.insp = { ...S.insp, gone: true };
    } else S.insp = j;
    renderTrack();
  } catch (_) { /* server busy or restarting: try again */ }
  inspTimer = setTimeout(pollInspect, 250);
}

const pct = (a, b) => Math.max(0, Math.min(100, (100 * a) / (b || 1)));
function bar(label, frac, cls, note) {
  return `<div class="tb"><span>${label}</span><i class="${cls}"><b style="width:${frac}%"></b></i><em>${note}</em></div>`;
}
function signedBar(label, v, note) {
  const w = Math.min(50, Math.abs(v) * 50), left = v < 0 ? 50 - w : 50;
  return `<div class="tb"><span>${label}</span><i class="sg"><b style="left:${left}%;width:${w}%"></b></i><em>${note}</em></div>`;
}

// Plain-language summary of what the organism is doing right now.
function doing(i) {
  const out = [];
  const fi = i.fight || {};
  if (fi.target) out.push(`<b class="atk">attacking #${fi.target}</b>: took ${fi.dealt.toFixed(2)} energy this tick`);
  if (fi.attacker) out.push(`<b class="hit">under attack by #${fi.attacker}</b>: lost ${fi.taken.toFixed(2)} this tick`);
  if (fi.hurt > 0) out.push(`<b class="hit">hurt by its prey's spikes or toxin</b>: −${fi.hurt.toFixed(2)}`);
  const e = i.energy / i.capacity;
  if (i.energy >= i.split_energy && i.material >= i.child_material) out.push("about to divide");
  if (i.brain.thrust <= 0) out.push("holding still (its brain says stop)");
  else out.push(`swimming at ${i.next.speed.toFixed(2)} per tick`);
  if (Math.abs(i.next.turn_deg) >= 0.5) out.push(`turning ${i.next.turn_deg > 0 ? "right" : "left"} ${Math.abs(i.next.turn_deg).toFixed(1)}° a tick`);
  if (i.mouths === 0 && !fi.target) out.push("has no mouth: it eats only by attacking");
  else if (i.sugar_here > 0.05) out.push(`eating: sugar under it ${i.sugar_here.toFixed(2)}`);
  else out.push("no sugar under it");
  if (e < 0.2) out.push(`starving: about ${Math.round(i.energy / i.upkeep)} ticks of energy left`);
  return out;
}

function renderTrack() {
  const sec = $("track");
  if (S.track === null) { sec.hidden = true; return; }
  sec.hidden = false;
  const i = S.insp, el = $("trackbody");
  if (!i) { el.innerHTML = `<p class="hint">Finding organism…</p>`; return; }
  const head = `<p class="tt"><b style="color:${col(i.lineage)}">#${i.id}</b> ${i.species || ""} · ${i.owner || "sandbox"}<br>
    <span class="hint">lineage ${i.lineage} · generation ${i.generation} · age ${i.age} ticks</span></p>`;
  if (i.gone) {
    el.innerHTML = head + `<p class="gone">It's gone: it died (starved) after tick ${i.tick}.</p>`;
    return;
  }
  const sensors = i.sensors.map((s) =>
    bar(`${s.angle_deg > 0 ? "+" : ""}${s.angle_deg}° ${s.chemical}`, 100 * s.input, s.chemical, s.reading.toFixed(2))).join("")
    || `<p class="hint">No sensors: it moves blind.</p>`;
  el.innerHTML = head +
    `<ul class="doing">${doing(i).map((d) => `<li>${d}</li>`).join("")}</ul>` +
    `<h3>Body</h3>` +
    bar("energy", pct(i.energy, i.capacity), "en", `${i.energy.toFixed(0)} / ${i.capacity.toFixed(0)} (divides at ${i.split_energy.toFixed(0)})`) +
    bar("material", pct(i.material, i.child_material), "ma", `${i.material.toFixed(1)} / ${i.child_material.toFixed(1)} for a child`) +
    `<h3>Senses</h3>` + sensors +
    `<h3>Brain → next tick</h3>` +
    signedBar("thrust", i.brain.thrust, i.brain.thrust.toFixed(2)) +
    signedBar("turn", i.brain.turn, i.brain.turn.toFixed(2));
}
$("chem").onclick = (e) => {
  S.chem = !S.chem;
  e.currentTarget.setAttribute("aria-pressed", String(S.chem));
  S.dirty = true;
};
window.addEventListener("resize", () => (S.dirty = true));

let lastAnim = 0;
function loop(now) {
  if (S.dirty) {
    S.dirty = false;
    S.animating = false;
    drawPool();
    drawGraph();
  } else if (S.animating && now - lastAnim > 33) {
    // Detailed organisms wave their flagella and cilia: redraw ~30 times a second.
    lastAnim = now;
    S.animating = false;
    drawPool();
  }
  requestAnimationFrame(loop);
}

// ---- splash: the title screen ----------------------------------------------

function renderSplash(m) {
  const sp = $("splash");
  if (!sp || sp.classList.contains("gone")) return;
  const w = m.world || {};
  const bits = [w.name ? `world ${w.name}` : null, `tick ${m.tick.toLocaleString()}`,
    `combat ${w.combat || "off"}`, `evolution ${w.body_evolution ? Math.round(w.body_evolution * 100) + "%" : "off"}`];
  $("sp-match").textContent = bits.filter(Boolean).join("  ·  ");
  const names = (w.players || []).filter(Boolean);
  $("sp-players").innerHTML = names.map((n) => `<li>${n.replace(/[<&]/g, "")}</li>`).join("");
}
function closeSplash() {
  const sp = $("splash");
  if (!sp) return;
  sp.classList.add("gone");
  try { sessionStorage.setItem("primordia-splash", "1"); } catch (_) {}
  setTimeout(() => sp.remove(), 600);
}
$("sp-enter").onclick = closeSplash;
window.addEventListener("keydown", (ev) => {
  if ((ev.key === "Enter" || ev.key === " ") && $("splash") && !$("splash").classList.contains("gone")) {
    ev.preventDefault();
    closeSplash();
  }
});

// Link parameters: ?follow=<lineage>&track=<organism id>&zoom=<1..32>&chem=0
{
  const q = new URLSearchParams(location.search);
  // Skip the title screen when asked (?splash=0), when a link opens straight
  // onto something (?track/?follow), or once per browser tab.
  let seen = false;
  try { seen = sessionStorage.getItem("primordia-splash") === "1"; } catch (_) {}
  if (q.get("splash") === "0" || q.has("track") || q.has("follow") || seen) {
    $("splash")?.remove();
  }
  if (q.has("follow")) S.follow = Number(q.get("follow"));
  const z = Number(q.get("zoom"));
  if (z >= ZOOM_MIN && z <= ZOOM_MAX) setZoom(z);
  if (q.has("track")) startTracking(Number(q.get("track")));
  if (q.get("chem") === "0") {
    S.chem = false;
    $("chem").setAttribute("aria-pressed", "false");
  }
}

renderLineages();
connect();
loop();
