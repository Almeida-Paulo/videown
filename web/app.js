"use strict";

const $ = (id) => document.getElementById(id);
const QUALITIES = {
  video: [["best", "Melhor"], ["1080", "1080p"], ["720", "720p"], ["480", "480p"]],
  audio: [["mp3", "MP3"], ["m4a", "M4A"]],
};
const FORMAT_LABEL = {
  best: "Vídeo · melhor", 1080: "Vídeo · 1080p", 720: "Vídeo · 720p", 480: "Vídeo · 480p",
  mp3: "Áudio · MP3", m4a: "Áudio · M4A",
};
const ICONS = {
  download: '<path d="M12 4v11m0 0-4-4m4 4 4-4M5 20h14"/>',
  cancel: '<path d="M7 7l10 10M17 7 7 17"/>',
  retry: '<path d="M4 12a8 8 0 0 1 14-5.3L20 8M20 4v4h-4M20 12a8 8 0 0 1-14 5.3L4 16m0 4v-4h4"/>',
  trash: '<path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3"/>',
};

const state = {
  mode: "video",
  quality: "best",
  preview: null,      // {kind, title, uploader, thumbnail, count, truncated, entries}
  previewUrl: "",
  loading: false,
  previewError: "",
  selected: new Set(),
  seq: 0,
  abort: null,
};
let timer = null;

// ---------- utilidades ----------

async function api(path, data, signal) {
  const options = { method: data ? "POST" : "GET", headers: { "X-Requested-With": "videown" }, credentials: "same-origin", signal };
  if (data) options.body = data instanceof URLSearchParams ? data : new URLSearchParams(data);
  const response = await fetch(path, options);
  if (response.status === 401) {
    location.href = "/login";
    throw new Error("login");
  }
  const body = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(body.error || "Algo deu errado.");
  return body;
}

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value == null || value === false) continue;
    if (key === "class") node.className = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else node.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children) if (child != null) node.append(child);
  return node;
}

function icon(name) {
  const span = document.createElement("span");
  span.innerHTML = `<svg viewBox="0 0 24 24" aria-hidden="true">${ICONS[name]}</svg>`;
  return span.firstChild;
}

function thumb(src, cls = "thumb") {
  if (!src) return el("div", { class: cls });
  return el("img", {
    class: cls, src, alt: "", loading: "lazy", referrerpolicy: "no-referrer",
    onerror: (e) => e.target.replaceWith(el("div", { class: cls })),
  });
}

function bytes(n) {
  if (!n) return "";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return `${n.toFixed(n >= 100 || i === 0 ? 0 : 1)} ${units[i]}`;
}

function clock(s) {
  if (s == null) return "";
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), sec = s % 60;
  const mm = h ? String(m).padStart(2, "0") : m;
  return `${h ? h + ":" : ""}${mm}:${String(sec).padStart(2, "0")}`;
}

function duration(s) {
  if (s < 0) return "";
  if (s < 60) return `${s} s`;
  if (s < 3600) return `${Math.round(s / 60)} min`;
  if (s < 86400) return `${Math.round(s / 3600)} h`;
  return `${Math.round(s / 86400)} dias`;
}

function looksLikeUrl(text) {
  return /^https?:\/\/[^\s/]+\.[^\s]+/i.test(text.trim());
}

function unavailable(entry) {
  return /^\[(private|deleted|unavailable)/i.test(entry.title || "");
}

function showError(message) {
  $("erro").textContent = message || "";
  $("erro").hidden = !message;
}

function save(key, value) {
  try { localStorage.setItem(key, value); } catch (e) { /* sem armazenamento local */ }
}

function load(key) {
  try { return localStorage.getItem(key); } catch (e) { return null; }
}

// ---------- formato ----------

function renderQuality() {
  const select = $("qualidade");
  select.replaceChildren(...QUALITIES[state.mode].map(([value, label]) => el("option", { value }, label)));
  if (!QUALITIES[state.mode].some(([v]) => v === state.quality)) state.quality = QUALITIES[state.mode][0][0];
  select.value = state.quality;
  for (const b of document.querySelectorAll(".segmented button")) {
    b.setAttribute("aria-checked", String(b.dataset.mode === state.mode));
  }
}

for (const b of document.querySelectorAll(".segmented button")) {
  b.addEventListener("click", () => {
    state.mode = b.dataset.mode;
    state.quality = load(`videown-q-${state.mode}`) || QUALITIES[state.mode][0][0];
    save("videown-mode", state.mode);
    renderQuality();
  });
}

$("qualidade").addEventListener("change", (e) => {
  state.quality = e.target.value;
  save(`videown-q-${state.mode}`, state.quality);
});

// ---------- prévia ----------

function renderPreview() {
  const box = $("previa");
  const url = $("url").value.trim();
  $("limpar").hidden = !url;

  if (state.loading) {
    box.hidden = false;
    box.replaceChildren(el("div", { class: "preview-msg" }, el("span", { class: "spinner" }), "Lendo o link…"));
  } else if (state.previewError) {
    box.hidden = false;
    box.replaceChildren(el("div", { class: "preview-msg erro-box" }, state.previewError));
  } else if (state.preview) {
    box.hidden = false;
    box.replaceChildren(...previewContent(state.preview));
  } else {
    box.hidden = true;
    box.replaceChildren();
  }
  renderButton();
}

function previewContent(p) {
  const meta = [];
  if (p.uploader) meta.push(p.uploader);
  if (p.kind === "playlist") meta.push(`${p.count} ${p.count === 1 ? "item" : "itens"}`);
  else if (p.entries[0] && p.entries[0].duration != null) meta.push(clock(p.entries[0].duration));

  const head = el("div", { class: "preview-head" },
    thumb(p.thumbnail),
    el("div", {},
      el("p", { class: "preview-title" }, p.title || "Sem título"),
      el("div", { class: "muted" }, meta.join(" · "))));
  if (p.kind !== "playlist") return [head];

  const count = el("span", { class: "count" });
  const rows = p.entries.map((entry, i) => {
    const row = el("button", { type: "button", class: "item", role: "checkbox" },
      el("span", { class: "check" }),
      el("span", { class: "num" }, String(i + 1)),
      el("span", { class: "name", title: entry.title }, entry.title || entry.url),
      el("span", { class: "len" }, clock(entry.duration)));
    row.addEventListener("click", () => {
      if (state.selected.has(i)) state.selected.delete(i); else state.selected.add(i);
      paint();
    });
    if (unavailable(entry)) row.classList.add("off");
    return row;
  });
  const paint = () => {
    rows.forEach((row, i) => {
      const on = state.selected.has(i);
      row.classList.toggle("on", on);
      row.setAttribute("aria-checked", String(on));
    });
    count.textContent = `${state.selected.size} de ${p.entries.length} selecionados`;
    renderButton();
  };
  const bar = el("div", { class: "pick-bar" }, count,
    el("button", { type: "button", onclick: () => { p.entries.forEach((_, i) => state.selected.add(i)); paint(); } }, "Todos"),
    el("button", { type: "button", onclick: () => { state.selected.clear(); paint(); } }, "Nenhum"));
  paint();

  const parts = [head, bar, el("div", { class: "items" }, ...rows)];
  if (p.truncated) parts.push(el("div", { class: "note" }, `Mostrando os primeiros ${p.entries.length} itens da playlist.`));
  return parts;
}

function renderButton() {
  const button = $("baixar");
  const url = $("url").value.trim();
  const p = state.preview;
  if (p && p.kind === "playlist") {
    const n = state.selected.size;
    button.textContent = n ? `Baixar ${n}` : "Baixar";
    button.disabled = !n;
  } else {
    button.textContent = "Baixar";
    button.disabled = !looksLikeUrl(url) || state.loading;
  }
}

async function fetchPreview() {
  const url = $("url").value.trim();
  if (!looksLikeUrl(url) || url === state.previewUrl) return;
  if (state.abort) state.abort.abort();
  const seq = ++state.seq;
  state.abort = new AbortController();
  Object.assign(state, { previewUrl: url, preview: null, previewError: "", loading: true });
  state.selected.clear();
  showError("");
  renderPreview();
  try {
    const p = await api("/api/info", { url }, state.abort.signal);
    if (seq !== state.seq) return;
    state.preview = p;
    p.entries.forEach((entry, i) => { if (!unavailable(entry)) state.selected.add(i); });
  } catch (e) {
    if (seq !== state.seq || e.name === "AbortError" || e.message === "login") return;
    state.previewError = e.message;
  }
  state.loading = false;
  renderPreview();
}

function resetPreview() {
  if (state.abort) state.abort.abort();
  state.seq++;
  Object.assign(state, { preview: null, previewUrl: "", previewError: "", loading: false, abort: null });
  state.selected.clear();
  renderPreview();
}

let typing = null;
$("url").addEventListener("input", () => {
  clearTimeout(typing);
  const url = $("url").value.trim();
  if (url !== state.previewUrl) resetPreview();
  if (looksLikeUrl(url)) typing = setTimeout(fetchPreview, 450);
});
$("url").addEventListener("paste", () => setTimeout(fetchPreview, 0));
$("limpar").addEventListener("click", () => {
  $("url").value = "";
  resetPreview();
  showError("");
  $("url").focus();
});
$("busca").addEventListener("submit", (e) => {
  e.preventDefault();
  if (state.preview && !$("baixar").disabled) startDownload();
  else fetchPreview();
});

// ---------- download ----------

async function startDownload() {
  const url = $("url").value.trim();
  const p = state.preview;
  let items;
  if (p && p.kind === "playlist") items = p.entries.filter((_, i) => state.selected.has(i));
  else if (p) items = [p.entries[0]];
  else items = [{ url, title: "", thumbnail: "" }];
  if (!items.length) return;

  const body = new URLSearchParams({ format: state.quality });
  for (const item of items) {
    body.append("url", item.url);
    body.append("title", item.title || "");
    body.append("thumbnail", item.thumbnail || "");
  }
  const button = $("baixar");
  button.disabled = true;
  showError("");
  try {
    await api("/api/jobs", body);
    $("url").value = "";
    resetPreview();
  } catch (e) {
    showError(e.message);
    renderButton();
  }
  refresh();
}
$("baixar").addEventListener("click", startDownload);

// ---------- lista de downloads ----------

function statusText(job, now) {
  switch (job.status) {
    case "queued": return "Na fila";
    case "running": {
      if (job.stage !== "Baixando") return job.stage || "Começando…";
      const parts = [];
      if (job.total) parts.push(`${Math.floor((job.downloaded / job.total) * 100)}% de ${bytes(job.total)}`);
      else if (job.downloaded) parts.push(bytes(job.downloaded));
      if (job.speed) parts.push(`${bytes(job.speed)}/s`);
      if (job.eta >= 0) parts.push(`faltam ${duration(job.eta)}`);
      return parts.join(" · ") || "Baixando";
    }
    case "done": {
      const left = job.expires - now;
      return `Pronto · ${bytes(job.size)} · ${left > 0 ? `apagado em ${duration(left)}` : "será apagado em breve"}`;
    }
    case "failed": return job.error || "Falhou";
    case "canceled": return "Cancelado";
  }
  return job.status;
}

function iconButton(name, label, onClick) {
  return el("button", { type: "button", class: "icon-btn", title: label, "aria-label": label, onclick: async (event) => {
    event.currentTarget.disabled = true;
    try { await onClick(); } catch (e) { showError(e.message); }
    refresh();
  } }, icon(name));
}

function renderJobs(data) {
  const now = data.now;
  $("info").textContent = `Arquivos ficam ${data.retention_hours} h no servidor` + (data.ytdlp ? ` · yt-dlp ${data.ytdlp}` : "");
  const list = $("lista");
  if (!data.jobs.length) {
    list.replaceChildren(el("p", { class: "empty" }, "Nenhum download ainda."));
    return;
  }
  list.replaceChildren(...data.jobs.map((job) => {
    const active = job.status === "running" || job.status === "queued";
    const info = el("div", { class: "body" },
      el("p", { class: "title", title: job.title || job.url }, job.title || job.url),
      el("div", { class: `status ${job.status}` }, `${FORMAT_LABEL[job.format] || job.format} · ${statusText(job, now)}`));
    if (job.status === "running") {
      const percent = job.total ? Math.min(100, (job.downloaded / job.total) * 100) : 0;
      const bar = el("div", { class: "bar" + (percent ? "" : " indeterminate") }, el("div"));
      if (percent) bar.firstChild.style.width = `${percent}%`;
      info.append(bar);
    }

    const actions = el("div", { class: "actions" });
    if (job.status === "done") {
      actions.append(el("a", { class: "icon-btn solid", href: `/files/${job.id}`, download: "", title: "Baixar arquivo", "aria-label": "Baixar arquivo" }, icon("download")));
    }
    if (active) actions.append(iconButton("cancel", "Cancelar", () => api(`/api/jobs/${job.id}/cancel`, {})));
    if (job.status === "failed" || job.status === "canceled") {
      actions.append(iconButton("retry", "Tentar de novo", async () => {
        await api("/api/jobs", { url: job.url, format: job.format, title: job.title, thumbnail: job.thumbnail });
        await api(`/api/jobs/${job.id}/delete`, {});
      }));
    }
    if (!active) {
      actions.append(iconButton("trash", job.status === "done" ? "Apagar arquivo" : "Remover", () => {
        if (job.status === "done" && !confirm("Apagar este arquivo do servidor?")) return;
        return api(`/api/jobs/${job.id}/delete`, {});
      }));
    }
    return el("article", { class: "job" }, thumb(job.thumbnail), info, actions);
  }));
}

async function refresh() {
  clearTimeout(timer);
  let busy = false;
  try {
    const data = await api("/api/jobs");
    busy = data.jobs.some((j) => j.status === "running" || j.status === "queued");
    renderJobs(data);
  } catch (e) {
    if (e.message !== "login") showError("Sem conexão com o servidor. Tentando de novo…");
  }
  timer = setTimeout(refresh, document.hidden ? 15000 : busy ? 1000 : 5000);
}

$("sair").addEventListener("click", async () => {
  try { await api("/logout", {}); } catch (e) { /* já saiu */ }
  location.href = "/login";
});
document.addEventListener("visibilitychange", () => { if (!document.hidden) refresh(); });

state.mode = load("videown-mode") === "audio" ? "audio" : "video";
state.quality = load(`videown-q-${state.mode}`) || QUALITIES[state.mode][0][0];
renderQuality();
renderPreview();
refresh();
