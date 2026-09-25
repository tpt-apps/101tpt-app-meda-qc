"use strict";

const tauri = globalThis.__TAURI__;
const invoke = tauri?.core?.invoke;
const dialog = tauri?.dialog;
const webview = tauri?.webview?.getCurrentWebview?.();

const state = {
  profiles: [],
  jobs: [],
  paused: false,
  maxConcurrent: 2,
  selectedJobId: null,
  selectedFinding: 0,
  nextId: 1,
  view: "dashboard",
};

const titles = {
  dashboard: ["OVERVIEW", "QC dashboard"],
  queue: ["PIPELINE", "Job queue"],
  inspector: ["ANALYSIS", "Asset inspector"],
};

const $ = (selector) => document.querySelector(selector);
const $$ = (selector) => [...document.querySelectorAll(selector)];
const terminal = new Set(["pass", "warn", "fail", "inconclusive", "error", "cancelled"]);
const fileName = (path) => path.replaceAll("\\", "/").split("/").pop() || path;
const escapeHtml = (value) => String(value ?? "").replace(/[&<>'"]/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", "'": "&#39;", '"': "&quot;" })[char]);
const jsonValue = (value) => value == null ? "—" : typeof value === "object" ? JSON.stringify(value) : String(value);

function formatBytes(bytes) {
  if (!Number.isFinite(bytes) || bytes <= 0) return "—";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit += 1; }
  return `${value.toFixed(unit ? 1 : 0)} ${units[unit]}`;
}

function formatDuration(ms) {
  if (!Number.isFinite(ms) || ms < 0) return "—";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds.toFixed(seconds < 10 ? 2 : 1)} s`;
  return `${Math.floor(seconds / 60)}m ${Math.round(seconds % 60)}s`;
}

function formatTimecode(ms) {
  if (!Number.isFinite(ms) || ms < 0) return "—";
  const millis = Math.round(ms);
  const hours = Math.floor(millis / 3_600_000);
  const minutes = Math.floor((millis % 3_600_000) / 60_000);
  const seconds = Math.floor((millis % 60_000) / 1000);
  const fraction = millis % 1000;
  return `${String(hours).padStart(2, "0")}:${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}.${String(fraction).padStart(3, "0")}`;
}

function toast(message, kind = "info", timeout = 4200) {
  const item = document.createElement("div");
  item.className = `toast ${kind}`;
  item.textContent = message;
  $("#toastStack").append(item);
  window.setTimeout(() => item.remove(), timeout);
}

function requireTauri() {
  if (!invoke || !dialog) {
    toast("Tauri APIs are unavailable. Run this page through the desktop binary.", "error", 8000);
    return false;
  }
  return true;
}

function setView(view) {
  state.view = view;
  $$("[data-panel]").forEach((panel) => panel.classList.toggle("active", panel.dataset.panel === view));
  $$(".nav-item").forEach((button) => button.classList.toggle("active", button.dataset.view === view));
  const [eyebrow, title] = titles[view];
  $("#viewEyebrow").textContent = eyebrow;
  $("#viewTitle").textContent = title;
  render();
}

async function loadProfiles() {
  if (!requireTauri()) return;
  try {
    state.profiles = await invoke("list_profiles");
    const select = $("#profileSelect");
    select.innerHTML = state.profiles.map((profile) => `<option value="${escapeHtml(profile.id)}">${escapeHtml(profile.name)} · v${profile.version}</option>`).join("");
  } catch (error) {
    toast(`Could not load profiles: ${error}`, "error");
  }
}

async function pickMedia(kind) {
  if (!requireTauri()) return;
  const recursive = $("#recursiveToggle").checked;
  try {
    const selected = await dialog.open(kind === "folder" ? { directory: true, multiple: false } : {
      multiple: true,
      filters: [{ name: "Media", extensions: ["mov", "mp4", "mxf", "m4v", "mkv", "ts", "mts", "m2ts", "wav", "aac", "w64", "ac3", "eac3", "mp3", "flac", "opus", "webm", "avi"] }],
    });
    if (!selected) return;
    await importPaths(Array.isArray(selected) ? selected : [selected], recursive);
  } catch (error) {
    toast(`Import failed: ${error}`, "error");
  }
}

async function importPaths(paths, recursive) {
  if (!requireTauri()) return;
  try {
    const expanded = await invoke("expand_media_paths", { paths, recursive });
    const profile = $("#profileSelect").value || "generic";
    const quick = $("#quickToggle").checked;
    for (const path of expanded) {
      if (state.jobs.some((job) => job.path === path && !terminal.has(job.status))) continue;
      state.jobs.unshift({
        id: state.nextId++,
        path,
        name: fileName(path),
        profile,
        quick,
        status: "queued",
        progress: 0,
        failures: 0,
        durationMs: 0,
        throughput: 0,
        result: null,
        error: "",
      });
    }
    toast(expanded.length ? `Added ${expanded.length} asset${expanded.length === 1 ? "" : "s"} to the queue.` : "No supported media found.", expanded.length ? "info" : "warn");
    render();
    pumpQueue();
  } catch (error) {
    toast(`Could not expand import paths: ${error}`, "error");
  }
}

async function runJob(job) {
  job.status = "running";
  job.progress = 8;
  job.startedAt = performance.now();
  render();
  const progressTimer = window.setInterval(() => {
    if (job.status === "running") {
      job.progress = Math.min(88, job.progress + (90 - job.progress) * 0.08);
      renderQueue();
    }
  }, 650);
  try {
    const result = await invoke("scan_asset", { path: job.path, profile: job.profile, quick: job.quick });
    job.result = result;
    job.status = result.verdict;
    job.failures = result.counts.fail;
    job.durationMs = performance.now() - job.startedAt;
    const bytes = result.asset?.size_bytes ?? 0;
    job.throughput = job.durationMs > 0 ? (bytes * 8) / job.durationMs : 0;
    job.progress = 100;
    if (!state.selectedJobId) selectJob(job.id);
  } catch (error) {
    job.status = "error";
    job.error = String(error);
    job.durationMs = performance.now() - job.startedAt;
    job.failures = 1;
    toast(`${job.name}: ${error}`, "error", 7000);
  } finally {
    window.clearInterval(progressTimer);
    render();
    pumpQueue();
  }
}

function pumpQueue() {
  if (state.paused) { render(); return; }
  const running = state.jobs.filter((job) => job.status === "running");
  const capacity = Math.max(0, state.maxConcurrent - running.length);
  state.jobs.filter((job) => job.status === "queued").slice(-capacity).forEach((job) => void runJob(job));
  render();
}

function retryJob(id) {
  const job = state.jobs.find((candidate) => candidate.id === id);
  if (!job) return;
  job.status = "queued";
  job.progress = 0;
  job.error = "";
  job.result = null;
  render();
  pumpQueue();
}

function cancelJob(id) {
  const job = state.jobs.find((candidate) => candidate.id === id);
  if (!job || job.status !== "queued") return;
  job.status = "cancelled";
  render();
}

function selectJob(id) {
  const job = state.jobs.find((candidate) => candidate.id === id);
  if (!job?.result) {
    if (job?.status === "queued" || job?.status === "running") setView("queue");
    return;
  }
  state.selectedJobId = id;
  state.selectedFinding = 0;
  setView("inspector");
}

function completedJobs() {
  return state.jobs.filter((job) => ["pass", "warn", "fail", "inconclusive"].includes(job.status));
}

function renderDashboard() {
  const completed = completedJobs();
  const pass = completed.filter((job) => job.status === "pass").length;
  const warn = completed.filter((job) => ["warn", "inconclusive"].includes(job.status)).length;
  const fail = completed.filter((job) => job.status === "fail").length;
  const queued = state.jobs.filter((job) => job.status === "queued").length;
  const running = state.jobs.filter((job) => job.status === "running").length;
  $("#metricPass").textContent = pass;
  $("#metricWarn").textContent = warn;
  $("#metricFail").textContent = fail;
  $("#metricQueued").textContent = queued;
  $("#metricRunning").textContent = `${running} processing`;

  const recent = state.jobs.slice(0, 8);
  $("#recentJobs").innerHTML = recent.length ? recent.map((job) => `
    <tr data-job="${job.id}">
      <td class="asset-cell"><strong>${escapeHtml(job.name)}</strong><small>${escapeHtml(job.path)}</small></td>
      <td>${escapeHtml(job.profile)}</td><td><span class="status ${escapeHtml(job.status)}">${escapeHtml(job.status)}</span></td>
      <td>${formatDuration(job.durationMs)}</td>
    </tr>`).join("") : `<tr class="empty-row"><td colspan="4">No analyses yet. Import media to begin.</td></tr>`;

  const failed = completed.filter((job) => job.status === "fail");
  $("#failedAssets").innerHTML = failed.length ? failed.slice(0, 5).map((job) => `
    <div class="failed-item" data-job="${job.id}"><div><strong>${escapeHtml(job.name)}</strong><span>${job.failures} failing finding${job.failures === 1 ? "" : "s"}</span></div><span>→</span></div>`).join("") : `<div class="evidence-empty">No failed assets in this session.</div>`;

  const totalMs = completed.reduce((sum, job) => sum + job.durationMs, 0);
  const perHour = totalMs > 0 ? (completed.length * 3_600_000) / totalMs : 0;
  $("#throughputValue").textContent = perHour.toFixed(1);
  $("#throughputMeter").style.width = `${Math.min(100, perHour * 3)}%`;
  $("#throughputDetail").textContent = completed.length ? `${completed.length} completed · average ${formatDuration(totalMs / completed.length)}` : "No completed jobs yet";
}


function renderQueue() {
  const queued = state.jobs.filter((job) => job.status === "queued").length;
  const running = state.jobs.filter((job) => job.status === "running").length;
  $("#queueSummary").textContent = `${queued} queued · ${running} processing${state.paused ? " · queue paused" : ""}`;
  $("#pauseQueue").textContent = state.paused ? "Paused" : "Pause";
  $("#pauseQueue").disabled = state.paused;
  $("#resumeQueue").disabled = !state.paused;
  const rows = state.jobs;
  $("#queueRows").innerHTML = rows.length ? rows.map((job) => {
    const progressClass = job.status === "running" ? "indeterminate" : "";
    let actions = "";
    if (job.status === "queued") actions += `<button class="action-button" data-action="cancel" data-id="${job.id}">Cancel</button>`;
    if (["error", "cancelled", "fail", "warn", "inconclusive"].includes(job.status)) actions += `<button class="action-button" data-action="retry" data-id="${job.id}">Retry</button>`;
    if (job.result) actions += `<button class="action-button" data-action="report" data-id="${job.id}">Open report</button>`;
    return `<tr data-job="${job.id}">
      <td class="asset-cell"><strong>${escapeHtml(job.name)}</strong><small>${escapeHtml(job.error || job.path)}</small></td>
      <td>${escapeHtml(job.profile)}</td>
      <td><div class="progress ${progressClass}"><i style="width:${job.progress}%"></i></div></td>
      <td>${job.throughput ? `${(job.throughput / 1e6).toFixed(1)} Mb/s` : "—"}</td>
      <td><span class="status ${escapeHtml(job.status)}">${escapeHtml(job.status)}</span></td>
      <td>${job.failures || "—"}</td><td>${formatDuration(job.durationMs)}</td><td>${actions || "—"}</td>
    </tr>`;
  }).join("") : `<tr class="empty-row"><td colspan="8">Queue is empty. Import files or folders to start.</td></tr>`;
}

function streamDescription(stream) {
  if (stream.kind === "video") return [stream.width && stream.height ? `${stream.width}×${stream.height}` : null, stream.pixel_format, stream.frame_rate ? `${stream.frame_rate} fps` : null].filter(Boolean).join(" · ") || "No video details";
  if (stream.kind === "audio") return [stream.codec_profile, stream.channel_layout || (stream.channels ? `${stream.channels} channels` : null), stream.sample_rate ? `${stream.sample_rate} Hz` : null, stream.bit_depth ? `${stream.bit_depth}-bit` : null].filter(Boolean).join(" · ") || "No audio details";
  return [stream.codec, stream.language].filter(Boolean).join(" · ") || stream.kind;
}

function measurementCards(result) {
  const cards = [];
  const container = result.inspection.container;
  if (container.bitrate_bps) cards.push(["Container bitrate", `${(container.bitrate_bps / 1e6).toFixed(2)} Mb/s`]);
  if (container.duration) cards.push(["Container duration", formatTimecode(container.duration)]);
  if (container.timestamps_contiguous != null) cards.push(["Timestamps", container.timestamps_contiguous ? "Contiguous" : "Discontinuous"]);
  if (container.timecode_present != null) cards.push(["Start timecode", container.timecode_present ? "Present" : "Absent"]);
  for (const video of result.inspection.video) {
    if (video.decoded_frame_count != null) cards.push([`Video s${video.stream_idx} frames`, video.decoded_frame_count]);
    if (video.luma) cards.push([`Video s${video.stream_idx} luma mean`, video.luma.mean.toFixed(1)]);
    if (video.decode_errors) cards.push([`Video s${video.stream_idx} errors`, video.decode_errors]);
  }
  for (const audio of result.inspection.audio) {
    if (audio.decoded_frame_count != null) cards.push([`Audio s${audio.stream_idx} frames`, audio.decoded_frame_count]);
    if (audio.peak_db != null) cards.push([`Audio s${audio.stream_idx} peak`, `${audio.peak_db.toFixed(1)} dBFS`]);
    if (audio.phase_correlation != null) cards.push([`Audio s${audio.stream_idx} phase`, audio.phase_correlation.toFixed(3)]);
    if (audio.dc_offset_percent != null) cards.push([`Audio s${audio.stream_idx} DC`, `${audio.dc_offset_percent.toFixed(2)}%`]);
  }
  return cards.slice(0, 12);
}

function renderTimeline(result) {
  const duration = result.report.asset_duration_ms || result.inspection.container.duration || 0;
  const ranged = result.report.findings.map((finding, index) => ({ finding, index })).filter(({ finding }) => finding.time_range);
  if (!duration || !ranged.length) return `<article class="panel"><header class="section-title"><div><p class="eyebrow">EVIDENCE</p><h3>Timeline</h3></div></header><div class="evidence-empty">No time-ranged findings or media duration available.</div></article>`;
  return `<article class="panel"><header class="section-title"><div><p class="eyebrow">EVIDENCE</p><h3>Timeline</h3></div><span class="status ${escapeHtml(result.verdict)}">${escapeHtml(result.verdict)}</span></header><div class="timeline">
    <div class="timeline-scale"><span>${formatTimecode(0)}</span><span>${formatTimecode(duration / 2)}</span><span>${formatTimecode(duration)}</span></div>
    <div class="timeline-track">${ranged.map(({ finding, index }) => {
      const start = Math.max(0, Math.min(100, (finding.time_range.start_ms / duration) * 100));
      const width = Math.max(0.35, Math.min(100 - start, ((finding.time_range.end_ms - finding.time_range.start_ms) / duration) * 100));
      return `<button class="timeline-marker ${escapeHtml(finding.status)}" style="left:${start}%;width:${width}%" data-finding="${index}" title="${escapeHtml(finding.rule_id)} · ${formatTimecode(finding.time_range.start_ms)}"></button>`;
    }).join("")}</div>
    <div class="timeline-lane"><span>VIDEO</span><div class="timeline-lane-bar"></div></div><div class="timeline-lane"><span>AUDIO</span><div class="timeline-lane-bar"></div></div>
  </div></article>`;
}

function renderFindingDetail(result) {
  const finding = result.report.findings[state.selectedFinding] || result.report.findings[0];
  if (!finding) return `<div class="evidence-empty">No findings were returned.</div>`;
  const evidence = finding.evidence?.length ? finding.evidence.map((item) => {
    const detail = item.payload?.Text || (item.payload?.Json ? jsonValue(item.payload.Json) : item.payload?.Bytes ? "Binary evidence attached" : item.description || "");
    return `<div class="evidence-card"><strong>${escapeHtml(item.label)}</strong><p>${escapeHtml(item.description || item.kind)}${detail ? ` · ${escapeHtml(detail)}` : ""}</p></div>`;
  }).join("") : `<div class="evidence-empty">No frame, waveform, or metadata excerpt is attached. The measured and expected rule values remain available.</div>`;
  return `<div class="finding-detail"><h4>${escapeHtml(finding.rule_id)}</h4>
    <div class="detail-row"><span>Status</span><code>${escapeHtml(finding.status)} / ${escapeHtml(finding.severity)}</code></div>
    <div class="detail-row"><span>Message</span><code>${escapeHtml(finding.message)}</code></div>
    <div class="detail-row"><span>Time</span><code>${finding.time_range ? `${formatTimecode(finding.time_range.start_ms)} → ${formatTimecode(finding.time_range.end_ms)}` : "Not time-bounded"}</code></div>
    <div class="detail-row"><span>Frame</span><code>${finding.frame_range ? `${finding.frame_range.start} → ${finding.frame_range.end}` : "Not frame-bounded"}</code></div>
    <div class="detail-row"><span>Measured</span><code>${escapeHtml(jsonValue(finding.measured))}</code></div>
    <div class="detail-row"><span>Expected</span><code>${escapeHtml(jsonValue(finding.expected))}</code></div>
  </div><div class="evidence-list">${evidence}</div>`;
}



function renderInspector() {
  const job = state.jobs.find((candidate) => candidate.id === state.selectedJobId);
  const empty = $("#inspectorEmpty");
  const content = $("#inspectorContent");
  if (!job?.result) { empty.style.display = "grid"; content.innerHTML = ""; return; }
  empty.style.display = "none";
  const result = job.result;
  const asset = result.asset;
  const info = [
    ["Filename", fileName(asset.path)], ["File size", formatBytes(asset.size_bytes)], ["SHA-256", asset.fingerprint.sha256],
    ["Container", result.inspection.container.format || "Unknown"], ["Duration", asset.duration ? `${asset.duration} s` : "Unknown"], ["Profile", `${result.profile_name} v${result.profile_version}`],
    ["Bitrate", result.inspection.container.bitrate_bps ? `${(result.inspection.container.bitrate_bps / 1e6).toFixed(2)} Mb/s` : "Unknown"], ["Resolution", result.report.asset_resolution || "No video"], ["Verdict", result.verdict],
  ];
  const streams = (asset.streams || []).map((stream) => `<div class="stream-card ${escapeHtml(stream.kind)}"><div class="stream-icon">${escapeHtml(stream.kind.slice(0, 2))}</div><div><strong>Stream ${stream.index} · ${escapeHtml(stream.codec || stream.kind)}</strong><span>${escapeHtml(streamDescription(stream))}</span></div><div class="stream-side">${stream.language ? escapeHtml(stream.language) : ""}<br>${stream.metadata ? `${Object.keys(stream.metadata).length} tags` : "0 tags"}</div></div>`).join("");
  const measurements = measurementCards(result);
  const findings = result.report.findings;
  const findingRows = findings.map((finding, index) => `<div class="finding-row ${index === state.selectedFinding ? "selected" : ""}" data-finding="${index}"><span class="status ${escapeHtml(finding.status)}">${escapeHtml(finding.status)}</span><div><strong>${escapeHtml(finding.rule_id)}</strong><span>${escapeHtml(finding.message)}</span></div><span class="finding-time">${finding.time_range ? formatTimecode(finding.time_range.start_ms) : "—"}</span></div>`).join("");
  content.innerHTML = `<div class="inspector-hero"><div class="inspector-title"><p class="eyebrow">${escapeHtml(result.profile_name)} · ANALYSIS ${escapeHtml(result.report.analysis_id)}</p><h2>${escapeHtml(fileName(asset.path))}</h2><p>${escapeHtml(asset.path)}</p></div><div class="inspector-actions"><button class="button secondary" data-export="pdf">Export PDF</button><button class="button ghost" data-export="json">JSON</button><button class="button ghost" data-export="html">HTML</button></div></div>
    <div class="inspector-grid"><div class="inspector-main">
      <article class="panel"><header class="section-title"><div><p class="eyebrow">ASSET</p><h3>File information</h3></div></header><div class="info-grid">${info.map(([label, value]) => `<div class="info-cell"><span>${escapeHtml(label)}</span><strong>${escapeHtml(value)}</strong></div>`).join("")}</div></article>
      <article class="panel"><header class="section-title"><div><p class="eyebrow">STREAMS</p><h3>Codec and stream metadata</h3></div><span class="diagnostic">${asset.streams.length} stream${asset.streams.length === 1 ? "" : "s"}</span></header><div class="stream-list">${streams || `<div class="evidence-empty">No stream metadata returned.</div>`}</div></article>
      ${renderTimeline(result)}
    </div><div class="inspector-side">
      <article class="panel"><header class="section-title"><div><p class="eyebrow">MEASUREMENTS</p><h3>Technical values</h3></div></header><div class="measurement-grid">${measurements.map(([label, value]) => `<div class="measurement"><span>${escapeHtml(label)}</span><strong>${escapeHtml(value)}</strong></div>`).join("") || `<div class="evidence-empty">No decoded measurements available.</div>`}</div></article>
      <article class="panel"><header class="section-title"><div><p class="eyebrow">FINDINGS</p><h3>Rule results</h3></div><span class="diagnostic">${findings.length}</span></header><div class="finding-list">${findingRows || `<div class="evidence-empty">No findings.</div>`}</div><div id="findingDetail">${renderFindingDetail(result)}</div></article>
    </div></div>`;
}


function render() {
  renderDashboard();
  renderQueue();
  renderInspector();
}

async function exportReport(format) {
  if (!requireTauri()) return;
  const job = state.jobs.find((candidate) => candidate.id === state.selectedJobId);
  if (!job?.result) return;
  try {
    const extension = format === "html" ? "html" : format;
    const selected = await dialog.save({ defaultPath: `${fileName(job.path)}.tpt-qc.${extension}`, filters: [{ name: format.toUpperCase(), extensions: [extension] }] });
    if (!selected) return;
    const path = await invoke("export_report", { run: job.result, path: selected, format });
    await invoke("open_path", { path });
    toast(`${format.toUpperCase()} report exported.`, "info");
  } catch (error) {
    toast(`Report export failed: ${error}`, "error");
  }
}

async function loadCustomProfile() {
  if (!requireTauri()) return;
  try {
    const selected = await dialog.open({ multiple: false, filters: [{ name: "YAML profile", extensions: ["yaml", "yml"] }] });
    if (!selected) return;
    const option = new Option(fileName(selected), selected, true, true);
    $("#profileSelect").add(option);
    toast(`Profile '${fileName(selected)}' selected for new jobs.`, "info");
  } catch (error) {
    toast(`Could not load profile: ${error}`, "error");
  }
}

function bindEvents() {
  $$(".nav-item").forEach((button) => button.addEventListener("click", () => setView(button.dataset.view)));
  $$("[data-go]").forEach((button) => button.addEventListener("click", () => setView(button.dataset.go)));
  $$("[data-pick]").forEach((button) => button.addEventListener("click", () => void pickMedia(button.dataset.pick)));
  $("#importButton").addEventListener("click", () => void pickMedia("files"));
  $("#loadProfileButton").addEventListener("click", () => void loadCustomProfile());
  $("#dropCard").addEventListener("click", (event) => { if (!event.target.closest("button,label,input")) void pickMedia("files"); });
  $("#pauseQueue").addEventListener("click", () => { state.paused = true; render(); toast("Queue paused. Running scans continue.", "warn"); });
  $("#resumeQueue").addEventListener("click", () => { state.paused = false; render(); pumpQueue(); });
  $("#cancelQueued").addEventListener("click", () => { state.jobs.filter((job) => job.status === "queued").forEach((job) => { job.status = "cancelled"; }); render(); });
  $("#clearCompleted").addEventListener("click", () => { state.jobs = state.jobs.filter((job) => !terminal.has(job.status)); render(); });

  document.addEventListener("click", async (event) => {
    const action = event.target.closest("[data-action]");
    if (action) {
      const id = Number(action.dataset.id);
      if (action.dataset.action === "cancel") cancelJob(id);
      if (action.dataset.action === "retry") retryJob(id);
      if (action.dataset.action === "report") {
        state.selectedJobId = id;
        await exportReport("html");
      }
      return;
    }
    const jobNode = event.target.closest("[data-job]");
    if (jobNode) { selectJob(Number(jobNode.dataset.job)); return; }
    const findingNode = event.target.closest("[data-finding]");
    if (findingNode) {
      state.selectedFinding = Number(findingNode.dataset.finding);
      renderInspector();
      return;
    }
    const exportNode = event.target.closest("[data-export]");
    if (exportNode) await exportReport(exportNode.dataset.export);
  });

  if (webview?.onDragDropEvent) {
    void webview.onDragDropEvent(({ payload }) => {
      const overlay = $("#dropOverlay");
      overlay.classList.toggle("visible", payload.type === "enter" || payload.type === "over");
      if (payload.type === "drop") {
        overlay.classList.remove("visible");
        void importPaths(payload.paths || [], $("#recursiveToggle").checked);
      }
      if (payload.type === "leave") overlay.classList.remove("visible");
    });
  }
}

void loadProfiles();
bindEvents();
render();
