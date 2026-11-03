// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: original
//
// Mica's web dashboard. Draws what the server sends; every decision is made on the server.
"use strict";

const HEADER = { "X-Mica-Dashboard": "1" };
const $ = (id) => document.getElementById(id);

function el(tag, props, ...children) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props || {})) {
    if (k === "class") e.className = v;
    else if (k === "text") e.textContent = v;
    else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else e.setAttribute(k, v);
  }
  for (const c of children) if (c != null) e.append(c);
  return e;
}

async function api(path, options) {
  const r = await fetch(path, options);
  const body = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(body.error || "The dashboard did not answer (" + r.status + ").");
  return body;
}

function duration(s) {
  if (s == null) return null;
  s = Math.round(s);
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60);
  if (h > 0) return h + " h " + m + " min";
  if (m > 0) return m + " min";
  return s + " s";
}

function degrees(t) {
  if (!t || t.current == null) return null;
  const now = Math.round(t.current) + "°";
  return t.target ? now + " / " + Math.round(t.target) + "°C" : now + "C";
}

function bytes(n) {
  if (n >= 1e9) return (n / 1e9).toFixed(1) + " GB";
  if (n >= 1e6) return (n / 1e6).toFixed(1) + " MB";
  return Math.max(1, Math.round(n / 1e3)) + " KB";
}

// ---- printers --------------------------------------------------------------------------

let printers = [];
const cards = new Map();

function makeCard(p) {
  const c = { id: p.id };
  c.title = el("h2");
  c.kind = el("span", { class: "kind" });
  c.state = el("span", { class: "state" });
  c.error = el("div", { class: "error-text" });
  c.jobName = el("div", { class: "job-name" });
  c.barFill = el("div");
  c.bar = el("div", { class: "bar" }, c.barFill);
  c.jobRow = el("div", { class: "row" });
  c.temps = el("div", { class: "row" });
  c.filaments = el("div", { class: "chips" });
  c.img = el("img", { alt: "Camera" });
  c.camera = el("div", { class: "camera" }, c.img);
  c.camera.hidden = true;
  c.img.addEventListener("error", () => {
    if (!c.img.getAttribute("src")) return;
    c.cameraNote.hidden = false;
    c.cameraNote.textContent = "The camera is not available right now.";
  });
  c.cameraNote = el("div", { class: "muted" });
  c.cameraButton = el("button", { class: "button", text: "Show camera", onclick: () => toggleCamera(c) });
  c.file = el("input", { type: "file", accept: ".gcode,.bgcode,.gco,.g" });
  c.file.addEventListener("change", () => {
    if (c.file.files[0]) upload(c, c.file.files[0]);
    c.file.value = "";
  });
  c.start = el("input", { type: "checkbox" });
  c.uploadButton = el("button", { class: "button", text: "Upload G-code", onclick: () => c.file.click() });
  c.transfer = el("div", { class: "muted" });
  c.transferFill = el("div");
  c.transferBar = el("div", { class: "bar" }, c.transferFill);
  c.root = el("article", { class: "card" },
    el("div", { class: "card-head" }, c.title, c.kind, c.state),
    c.error, c.jobName, c.bar, c.jobRow, c.temps, c.filaments, c.camera, c.cameraNote,
    el("div", { class: "actions" }, c.cameraButton,
      ...(printing ? [c.uploadButton, el("label", { class: "check" }, c.start, "Start printing after upload"), c.file] : [])),
    c.transferBar, c.transfer);
  // Read-only dashboard (the default): no uploads, so no drop target either.
  if (!printing) return c;
  for (const ev of ["dragenter", "dragover"]) {
    c.root.addEventListener(ev, (e) => { e.preventDefault(); c.root.classList.add("drop"); });
  }
  c.root.addEventListener("dragleave", () => c.root.classList.remove("drop"));
  c.root.addEventListener("drop", (e) => {
    e.preventDefault();
    c.root.classList.remove("drop");
    const f = e.dataTransfer.files[0];
    if (f) upload(c, f);
  });
  return c;
}

function updateCard(c, p) {
  c.title.textContent = p.name;
  c.kind.textContent = p.model || p.kind;
  c.state.textContent = p.state_label;
  c.state.className = "state " + p.state;
  c.error.textContent = p.error || "";
  c.error.hidden = !p.error;
  const job = p.job;
  c.jobName.hidden = c.bar.hidden = c.jobRow.hidden = !job;
  if (job) {
    c.jobName.textContent = job.name || "Unnamed job";
    c.barFill.style.width = (job.progress || 0) + "%";
    c.jobRow.replaceChildren(...[
      job.progress != null ? ["", Math.round(job.progress) + "%"] : null,
      job.elapsed != null ? ["Elapsed ", duration(job.elapsed)] : null,
      job.remaining != null ? ["Left ", duration(job.remaining)] : null,
      job.layer != null ? ["Layer ", String(job.layer)] : null,
    ].filter(Boolean).map(([k, v]) => el("span", {}, k, el("b", { text: v }))));
  }
  const t = p.temperatures;
  const temps = [];
  if (t) {
    for (const h of t.hotends) {
      const d = degrees(h);
      if (d) temps.push([t.hotends.length > 1 ? "Tool " + h.tool + " " : "Nozzle ", d]);
    }
    if (degrees(t.bed)) temps.push(["Bed ", degrees(t.bed)]);
    if (degrees(t.chamber)) temps.push(["Chamber ", degrees(t.chamber)]);
  }
  c.temps.replaceChildren(...temps.map(([k, v]) => el("span", {}, k, el("b", { text: v }))));
  c.filaments.replaceChildren(...p.filaments.map((f) => {
    const sw = el("span", { class: "swatch" });
    if (f.colour) sw.style.background = f.colour.css;
    const words = [f.material, f.colour && f.colour.name].filter(Boolean).join(" · ") || "Loaded";
    return el("span", { class: "chip", title: "Tool " + f.tool }, sw,
      el("span", { class: "slot", text: String(f.tool) }), words);
  }));
  const cam = p.camera;
  c.cameraButton.hidden = !cam.available;
  if (!cam.available && !c.camera.hidden) toggleCamera(c);
  c.cameraNote.textContent = cam.error || cam.note || "";
  c.cameraNote.hidden = !(cam.error || cam.note) || (!cam.available && !cam.note);
  const tr = p.transfer;
  if (tr && !c.sending) {
    const pct = tr.total ? Math.round((100 * tr.sent) / tr.total) : 0;
    c.transferBar.hidden = tr.phase !== "sending";
    c.transferFill.style.width = pct + "%";
    c.transfer.hidden = false;
    c.transfer.className = tr.phase === "failed" ? "error-text" : "muted";
    c.transfer.textContent = tr.phase === "sending"
      ? "Sending " + tr.name + " to the printer… " + pct + "%"
      : tr.name + ": " + (tr.message || (tr.phase === "done" ? "Uploaded." : "Upload failed."));
  } else if (!c.sending) {
    c.transferBar.hidden = c.transfer.hidden = true;
  }
  c.uploadButton.disabled = !!(tr && tr.phase === "sending") || !!c.sending;
}

function toggleCamera(c) {
  const show = c.camera.hidden;
  c.camera.hidden = !show;
  c.cameraButton.textContent = show ? "Hide camera" : "Show camera";
  // Removing the source closes the stream; the server stops the camera with its last viewer.
  if (show) c.img.src = "api/printers/" + encodeURIComponent(c.id) + "/camera";
  else c.img.removeAttribute("src");
}

function upload(c, file) {
  const name = file.name;
  if (!/\.(b?gcode|gco|g)$/i.test(name)) {
    showTransfer(c, "Only G-code files (.gcode or .bgcode) can be sent to a printer.", true);
    return;
  }
  const xhr = new XMLHttpRequest();
  const url = "api/printers/" + encodeURIComponent(c.id) + "/upload?name=" +
    encodeURIComponent(name) + "&start=" + (c.start.checked ? "1" : "0");
  xhr.open("POST", url);
  xhr.setRequestHeader("X-Mica-Dashboard", "1");
  xhr.setRequestHeader("Content-Type", "application/octet-stream");
  c.sending = true;
  c.uploadButton.disabled = true;
  c.transferBar.hidden = false;
  xhr.upload.onprogress = (e) => {
    const pct = e.total ? Math.round((100 * e.loaded) / e.total) : 0;
    c.transferFill.style.width = pct + "%";
    showTransfer(c, "Sending " + name + " to Mica… " + pct + "% of " + bytes(file.size), false);
  };
  xhr.onload = () => {
    c.sending = false;
    let msg = null;
    try { msg = JSON.parse(xhr.responseText).error; } catch (_) { /* not JSON */ }
    if (xhr.status >= 300) showTransfer(c, msg || "The upload failed.", true);
    refreshPrinters();
  };
  xhr.onerror = () => {
    c.sending = false;
    showTransfer(c, "The upload was interrupted.", true);
  };
  xhr.send(file);
}

function showTransfer(c, text, failed) {
  c.transfer.hidden = false;
  c.transfer.className = failed ? "error-text" : "muted";
  c.transfer.textContent = text;
  if (failed) c.transferBar.hidden = true;
}

async function refreshPrinters() {
  let data;
  try {
    data = await api("api/printers");
  } catch (e) {
    note("printers-note", e.message, true);
    return;
  }
  printers = data.printers;
  if (data.error) note("printers-note", data.error, true);
  else if (!printers.length) note("printers-note", "No printers yet. Add them in Mica under My printers; they show up here.", false);
  else note("printers-note", null);
  const grid = $("printer-grid");
  const seen = new Set();
  for (const p of printers) {
    let c = cards.get(p.id);
    if (!c) {
      c = makeCard(p);
      cards.set(p.id, c);
    }
    seen.add(p.id);
    updateCard(c, p);
    grid.append(c.root);
  }
  for (const [id, c] of cards) {
    if (!seen.has(id)) {
      c.img.removeAttribute("src");
      c.root.remove();
      cards.delete(id);
    }
  }
  updatePrinterChoices();
}

function note(id, text, error) {
  const n = $(id);
  n.hidden = !text;
  n.textContent = text || "";
  n.className = "notice" + (error ? " error" : "");
}

// ---- library ---------------------------------------------------------------------------

let offset = 0;
let searchTimer = null;
const PAGE = 60;

function modelCard(m) {
  const thumb = el("div", { class: "thumb" });
  if (m.thumbnail) thumb.append(el("img", { alt: "", loading: "lazy", src: "api/library/" + m.id + "/thumbnail" }));
  else thumb.textContent = m.kind;
  const meta = [m.kind, duration(m.print_time), m.filament_g != null ? Math.round(m.filament_g) + " g" : null, m.printer]
    .filter(Boolean).join(" · ");
  const body = el("div", { class: "body" },
    el("div", { class: "name", text: m.name }),
    el("div", { class: "meta", text: meta }),
    m.folder ? el("div", { class: "meta", text: m.folder }) : null);
  if (m.colours.length) {
    body.append(el("div", { class: "chips" }, ...m.colours.map((c) => {
      const sw = el("span", { class: "swatch", title: c.name });
      sw.style.background = c.css;
      return sw;
    })));
  }
  if (m.gcode && printing) {
    const pick = el("select", { "aria-label": "Printer" });
    const start = el("input", { type: "checkbox" });
    const status = el("div", { class: "meta" });
    const send = el("button", { class: "button", text: "Send", onclick: async () => {
      send.disabled = true;
      status.className = "meta";
      status.textContent = "Sending…";
      try {
        await api("api/printers/" + encodeURIComponent(pick.value) + "/send", {
          method: "POST",
          headers: Object.assign({ "Content-Type": "application/json" }, HEADER),
          body: JSON.stringify({ model: m.id, start: start.checked }),
        });
        status.textContent = "Sent; see the printer's card for progress.";
      } catch (e) {
        status.className = "meta error-text";
        status.textContent = e.message;
      }
      send.disabled = false;
    } });
    pick.dataset.printers = "1";
    fillPrinters(pick);
    body.append(el("div", { class: "send" }, pick, send),
      el("label", { class: "check" }, start, "Start printing"), status);
  }
  return el("article", { class: "card model" }, thumb, body);
}

function fillPrinters(select) {
  const keep = select.value;
  select.replaceChildren(...printers.map((p) => el("option", { value: p.id, text: p.name })));
  if (keep) select.value = keep;
  select.disabled = !printers.length;
}

function updatePrinterChoices() {
  document.querySelectorAll("select[data-printers]").forEach(fillPrinters);
}

async function loadLibrary(more) {
  if (!more) offset = 0;
  const q = new URLSearchParams({
    q: $("search").value,
    gcode: $("gcode-only").checked ? "1" : "0",
    offset: String(offset),
    limit: String(PAGE),
  });
  let data;
  try {
    data = await api("api/library?" + q);
  } catch (e) {
    note("library-note", e.message, true);
    return;
  }
  const grid = $("model-grid");
  if (!more) grid.replaceChildren();
  grid.append(...data.models.map(modelCard));
  offset += data.models.length;
  $("more").hidden = offset >= data.total;
  if (data.error) note("library-note", data.error, true);
  else if (!data.count) note("library-note", "The library is empty. Add folders in Mica's Library tab; Mica keeps the index, this page reads it.", false);
  else if (!data.total) note("library-note", "Nothing matches.", false);
  else note("library-note", null);
}

// ---- page ------------------------------------------------------------------------------

function showTab(name) {
  for (const t of ["printers", "library"]) {
    $(t).hidden = t !== name;
    $("tab-" + t).setAttribute("aria-selected", String(t === name));
  }
  if (name === "library") loadLibrary(false);
}

$("tab-printers").onclick = () => showTab("printers");
$("tab-library").onclick = () => showTab("library");
$("search").addEventListener("input", () => {
  clearTimeout(searchTimer);
  searchTimer = setTimeout(() => loadLibrary(false), 250);
});
$("gcode-only").onchange = () => loadLibrary(false);
$("more").onclick = () => loadLibrary(true);

// Cameras stop while the page is hidden (a phone in a pocket); they come back when shown.
document.addEventListener("visibilitychange", () => {
  for (const c of cards.values()) {
    if (c.camera.hidden) continue;
    if (document.hidden) c.img.removeAttribute("src");
    else c.img.src = "api/printers/" + encodeURIComponent(c.id) + "/camera";
  }
  if (!document.hidden) refreshPrinters();
});

// Whether this server takes uploads and starts prints (`--allow-printing`); off by default.
let printing = false;

api("api/info").then((info) => {
  printing = info.printing === true;
  if (info.open_url) {
    const a = $("open-mica");
    a.href = info.open_url;
    a.hidden = false;
  }
}).catch(() => {}).finally(() => {
  refreshPrinters();
  setInterval(() => { if (!document.hidden) refreshPrinters(); }, 2000);
});
