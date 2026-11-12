"use strict";

const $ = (id) => document.getElementById(id);
const FORMATS = {
  best: "Vídeo · melhor", 1080: "Vídeo · 1080p", 720: "Vídeo · 720p", 480: "Vídeo · 480p",
  mp3: "Áudio · MP3", m4a: "Áudio · M4A",
};
let timer = null;

async function api(path, data) {
  const options = { method: data ? "POST" : "GET", headers: { "X-Requested-With": "videown" }, credentials: "same-origin" };
  if (data) options.body = new URLSearchParams(data);
  const response = await fetch(path, options);
  if (response.status === 401) {
    location.href = "/login";
    throw new Error("login");
  }
  const body = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(body.error || "Algo deu errado.");
  return body;
}

function bytes(n) {
  if (!n) return "";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
  return `${n.toFixed(n >= 100 || i === 0 ? 0 : 1)} ${units[i]}`;
}

function duration(s) {
  if (s < 0) return "";
  if (s < 60) return `${s} s`;
  if (s < 3600) return `${Math.round(s / 60)} min`;
  if (s < 86400) return `${Math.round(s / 3600)} h`;
  return `${Math.round(s / 86400)} dias`;
}

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (key === "class") node.className = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else node.setAttribute(key, value);
  }
  for (const child of children) if (child != null) node.append(child);
  return node;
}

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

function action(label, onClick) {
  return el("button", { type: "button", onclick: async (event) => {
    event.target.disabled = true;
    try { await onClick(); } catch (e) { showError(e.message); }
    refresh();
  } }, label);
}

function render(data) {
  const now = data.now;
  const list = $("lista");
  $("info").textContent =
    `Os arquivos ficam guardados por ${data.retention_hours} horas e depois são apagados.` +
    (data.ytdlp ? ` yt-dlp ${data.ytdlp}.` : "");
  if (!data.jobs.length) {
    list.replaceChildren(el("p", { class: "empty" }, "Nenhum download ainda. Cole um link acima."));
    return;
  }
  list.replaceChildren(...data.jobs.map((job) => {
    const thumb = job.thumbnail
      ? el("img", {
          class: "thumb", src: job.thumbnail, alt: "", loading: "lazy", referrerpolicy: "no-referrer",
          onerror: (e) => e.target.replaceWith(el("div", { class: "thumb" })),
        })
      : el("div", { class: "thumb" });
    const info = el("div", {},
      el("p", { class: "title" }, job.title || job.url),
      el("div", { class: "muted" }, FORMATS[job.format] || job.format),
      el("div", { class: `status ${job.status}` }, statusText(job, now)));

    if (job.status === "running" || job.status === "queued") {
      const percent = job.total ? Math.min(100, (job.downloaded / job.total) * 100) : 0;
      const bar = el("div", { class: "bar" + (percent ? "" : " indeterminate") }, el("div"));
      if (percent) bar.firstChild.style.width = `${percent}%`;
      info.append(bar);
    }

    const actions = el("div", { class: "actions" });
    if (job.status === "done") {
      actions.append(el("a", { href: `/files/${job.id}`, download: "" }, "Baixar arquivo"));
    }
    if (job.status === "running" || job.status === "queued") {
      actions.append(action("Cancelar", () => api(`/api/jobs/${job.id}/cancel`, {})));
    }
    if (job.status === "failed" || job.status === "canceled") {
      actions.append(action("Tentar de novo", async () => {
        await api("/api/jobs", { url: job.url, format: job.format });
        await api(`/api/jobs/${job.id}/delete`, {});
      }));
    }
    if (job.status === "done" || job.status === "failed" || job.status === "canceled") {
      actions.append(action(job.status === "done" ? "Apagar" : "Remover", () => {
        if (job.status === "done" && !confirm("Apagar este arquivo do servidor?")) return;
        return api(`/api/jobs/${job.id}/delete`, {});
      }));
    }
    info.append(actions);
    return el("article", { class: "card job" }, thumb, info);
  }));
}

function showError(message) {
  const box = $("erro");
  box.textContent = message || "";
  box.hidden = !message;
}

async function refresh() {
  clearTimeout(timer);
  let busy = false;
  try {
    const data = await api("/api/jobs");
    busy = data.jobs.some((j) => j.status === "running" || j.status === "queued");
    render(data);
  } catch (e) {
    if (e.message !== "login") showError("Sem conexão com o servidor. Tentando de novo…");
  }
  timer = setTimeout(refresh, document.hidden ? 15000 : busy ? 1000 : 5000);
}

$("novo").addEventListener("submit", async (event) => {
  event.preventDefault();
  const button = $("baixar");
  button.disabled = true;
  showError("");
  try {
    await api("/api/jobs", { url: $("url").value.trim(), format: $("format").value });
    $("url").value = "";
    try { localStorage.setItem("videown-format", $("format").value); } catch (e) { /* sem armazenamento local */ }
  } catch (e) {
    showError(e.message);
  }
  button.disabled = false;
  refresh();
});

$("sair").addEventListener("click", async () => {
  try { await api("/logout", {}); } catch (e) { /* já saiu */ }
  location.href = "/login";
});

document.addEventListener("visibilitychange", () => { if (!document.hidden) refresh(); });

try {
  const saved = localStorage.getItem("videown-format");
  if (saved && FORMATS[saved]) $("format").value = saved;
} catch (e) { /* sem armazenamento local */ }

refresh();
