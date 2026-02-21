import type { BuildInfo, P2PShareApi, StartTransferPayload, TransferEvent } from "../shared/ipc";
import { evaluateSchemaCompatibility, TRANSFER_EVENT_SCHEMA_VERSION } from "../shared/schema";

const api: P2PShareApi | undefined = window.p2pShareApi;

type UiMode = "send" | "receive";

type TransferMode = StartTransferPayload["mode"];

const $ = <T extends HTMLElement = HTMLElement>(id: string): T => {
  const el = document.getElementById(id);
  if (!el) {
    throw new Error(`Missing required element: #${id}`);
  }
  return el as T;
};

const viewConfig = $("viewConfig");
const viewTransfer = $("viewTransfer");
const tabSend = $<HTMLButtonElement>("tabSend");
const tabReceive = $<HTMLButtonElement>("tabReceive");
const sendConfig = $("sendConfig");
const receiveConfig = $("receiveConfig");
const dropZone = $<HTMLButtonElement>("dropZone");
const fileNameEl = $("fileName");
const sendTicketField = $("sendTicketField");
const sendTicketInput = $<HTMLInputElement>("sendTicketInput");
const targetField = $("targetField");
const targetInput = $<HTMLInputElement>("targetInput");
const outputDir = $<HTMLInputElement>("outputDir");
const pickOutputBtn = $<HTMLButtonElement>("pickOutputBtn");
const startBtn = $<HTMLButtonElement>("startBtn");
const cancelBtn = $<HTMLButtonElement>("cancelBtn");
const newTransferBtn = $<HTMLButtonElement>("newTransferBtn");
const statusDot = $("statusDot");
const statusText = $("statusText");
const connectionText = $("connectionText");
const handshakeCode = $("handshakeCode");
const progressPercent = $("progressPercent");
const progressDetail = $("progressDetail");
const progressFill = $("progressFill");
const shareArea = $("shareArea");
const qrImage = $<HTMLImageElement>("qrImage");
const ticketValue = $("ticketValue");
const copyBtn = $<HTMLButtonElement>("copyBtn");
const qrTrigger = $<HTMLButtonElement>("qrTrigger");
const qrOverlay = $("qrOverlay");
const qrOverlayImage = $<HTMLImageElement>("qrOverlayImage");
const completeSummary = $("completeSummary");
const completeDetail = $("completeDetail");
const errorSummary = $("errorSummary");
const errorDetail = $("errorDetail");
const platformInfo = $("platformInfo");
const p2pViz = $("p2pViz");
const p2pLink = $("p2pLink");
const logToggle = $<HTMLButtonElement>("logToggle");
const logPanel = $("logPanel");
const logChevron = $("logChevron");
const clearLogBtn = $<HTMLButtonElement>("clearLogBtn");
const eventLog = $<HTMLPreElement>("eventLog");

let mode: UiMode = "send";
let subMode: TransferMode = "send_wait";
let filePath = "";
let isRunning = false;
let currentTicket = "";
let logOpen = false;
let transferSchemaWarningShown = false;
let platformLabel = "Unknown platform";
let buildInfoLabel = "";

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function describeCliCompatibility(info: BuildInfo): string {
  const version = info.cli.version ?? "unknown";
  const schema = info.cli.schemaVersion ?? "unknown";
  const source = info.cli.source;
  return `CLI ${version} (${source}), schema ${schema}`;
}

function refreshPlatformInfo(): void {
  platformInfo.textContent = buildInfoLabel ? `${platformLabel} · ${buildInfoLabel}` : platformLabel;
}

function applyBuildInfoStatus(info: BuildInfo): void {
  buildInfoLabel = describeCliCompatibility(info);
  refreshPlatformInfo();

  if (!info.cli.exists) {
    pushLog(`CLI not found: ${info.cli.command}`);
    return;
  }

  if (info.cli.compatibility === "mismatch") {
    pushLog(
      `Schema mismatch: expected ${info.expectedSchemaVersion}, CLI reports ${info.cli.schemaVersion ?? "unknown"}.`
    );
    return;
  }

  if (info.cli.compatibility === "unknown") {
    pushLog(
      `Schema compatibility is unknown. Expected ${info.expectedSchemaVersion}; CLI did not report a schema version.`
    );
  }
}

function isTransferMode(value: string): value is TransferMode {
  return (
    value === "send_wait" ||
    value === "send_to_ticket" ||
    value === "receive_target" ||
    value === "receive_listen"
  );
}

function getApi(): P2PShareApi | null {
  if (api) {
    return api;
  }
  pushLog("Desktop bridge unavailable. Restart app.");
  return null;
}

function formatBytes(bytes: number | null | undefined): string {
  const v = Number(bytes) || 0;
  if (v < 1024) return `${v} B`;
  if (v < 1024 * 1024) return `${(v / 1024).toFixed(1)} KB`;
  if (v < 1024 ** 3) return `${(v / 1024 / 1024).toFixed(1)} MB`;
  return `${(v / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

function nowTime(): string {
  return new Date().toLocaleTimeString();
}

function pushLog(text: string): void {
  eventLog.textContent += `[${nowTime()}] ${text}\n`;
  eventLog.scrollTop = eventLog.scrollHeight;
}

function flash(el: HTMLElement): void {
  el.classList.remove("flash-error");
  void el.offsetWidth;
  el.classList.add("flash-error");
  setTimeout(() => el.classList.remove("flash-error"), 700);
}

function setMode(newMode: UiMode): void {
  mode = newMode;

  tabSend.classList.toggle("mode-tab--active", mode === "send");
  tabReceive.classList.toggle("mode-tab--active", mode === "receive");

  sendConfig.classList.toggle("hidden", mode !== "send");
  receiveConfig.classList.toggle("hidden", mode !== "receive");

  if (mode === "send") {
    setSubMode("send_wait");
  } else {
    setSubMode("receive_target");
  }
}

function setSubMode(newSubMode: TransferMode): void {
  subMode = newSubMode;

  document.querySelectorAll<HTMLButtonElement>(".variant-btn").forEach((btn) => {
    btn.classList.toggle("variant-btn--active", btn.dataset.variant === subMode);
  });

  if (mode === "send") {
    sendTicketField.classList.toggle("hidden", subMode !== "send_to_ticket");
  } else {
    targetField.classList.toggle("hidden", subMode !== "receive_target");
  }
}

function updateDropZoneLabel(text: string): void {
  const label = dropZone.querySelector<HTMLElement>(".drop-zone-label");
  if (label) {
    label.textContent = text;
  }
}

function updateFileDisplay(name?: string | null, size?: number): void {
  if (name) {
    const sizeStr = size ? ` (${formatBytes(size)})` : "";
    fileNameEl.textContent = name + sizeStr;
    fileNameEl.classList.remove("hidden");
    dropZone.classList.add("drop-zone--selected");
    updateDropZoneLabel("Click to change file");
  } else {
    fileNameEl.textContent = "";
    fileNameEl.classList.add("hidden");
    dropZone.classList.remove("drop-zone--selected");
    updateDropZoneLabel("Drop a file here or click to browse");
  }
}

function switchToTransfer(): void {
  viewConfig.classList.add("hidden");
  viewTransfer.classList.remove("hidden");

  statusDot.className = "status-dot";
  statusText.textContent = "Connecting...";
  connectionText.textContent = "Waiting...";
  handshakeCode.textContent = "- - - -";
  progressPercent.textContent = "0%";
  progressDetail.textContent = "";
  progressFill.style.width = "0%";
  shareArea.classList.add("hidden");
  completeSummary.classList.add("hidden");
  errorSummary.classList.add("hidden");
  newTransferBtn.classList.add("hidden");
  cancelBtn.classList.remove("hidden");
  currentTicket = "";

  p2pViz.className = "p2p-viz";
  p2pLink.className = mode === "receive" ? "p2p-link p2p-link--receive" : "p2p-link";
}

function switchToConfig(): void {
  viewTransfer.classList.add("hidden");
  viewConfig.classList.remove("hidden");
  qrOverlay.classList.add("hidden");

  filePath = "";
  updateFileDisplay(null);
  sendTicketInput.value = "";
  targetInput.value = "";
}

function buildPayload(): StartTransferPayload {
  return {
    mode: subMode,
    filePath,
    ticket: sendTicketInput.value.trim(),
    target: targetInput.value.trim(),
    outputDir: outputDir.value.trim() || "."
  };
}

async function startTransfer(): Promise<void> {
  const desktopApi = getApi();
  if (!desktopApi) return;

  if ((subMode === "send_wait" || subMode === "send_to_ticket") && !filePath) {
    flash(dropZone);
    pushLog("Please select a file to send.");
    return;
  }
  if (subMode === "send_to_ticket" && !sendTicketInput.value.trim()) {
    flash(sendTicketInput);
    pushLog("Please enter a destination ticket.");
    return;
  }
  if (subMode === "receive_target" && !targetInput.value.trim()) {
    flash(targetInput);
    pushLog("Please enter a ticket or address.");
    return;
  }

  const payload = buildPayload();

  try {
    isRunning = true;
    switchToTransfer();
    await desktopApi.startTransfer(payload);
  } catch (err) {
    isRunning = false;
    statusDot.className = "status-dot status-dot--error";
    statusText.textContent = "Failed to start";
    p2pViz.classList.add("p2p-viz--error");
    errorSummary.classList.remove("hidden");
    errorDetail.textContent = errorMessage(err);
    cancelBtn.classList.add("hidden");
    newTransferBtn.classList.remove("hidden");
    pushLog(`Error: ${errorMessage(err)}`);
  }
}

async function setTicketVisual(ticket: string): Promise<void> {
  currentTicket = ticket || "";
  ticketValue.textContent = currentTicket || "Generating...";

  if (!currentTicket) {
    shareArea.classList.add("hidden");
    return;
  }

  shareArea.classList.remove("hidden");

  try {
    const desktopApi = getApi();
    if (!desktopApi) return;
    const qrDataUrl = await desktopApi.createQrDataUrl(currentTicket);
    if (qrDataUrl) {
      qrImage.src = qrDataUrl;
      qrOverlayImage.src = qrDataUrl;
    }
  } catch (err) {
    pushLog(`QR generation failed: ${errorMessage(err)}`);
  }
}

function summarizeEvent(evt: TransferEvent): string {
  switch (evt.kind) {
    case "status":
      return typeof evt.message === "string" ? evt.message : "status";
    case "ticket":
      return "Ticket generated";
    case "qr_payload":
      return "QR payload ready";
    case "handshake_code":
      return `Handshake: ${typeof evt.value === "string" ? evt.value : "n/a"}`;
    case "progress": {
      const done = typeof evt.done === "number" ? evt.done : 0;
      const total = typeof evt.total === "number" ? evt.total : 0;
      return `Progress: ${done}/${total}`;
    }
    case "connection_path":
      return `Path: ${typeof evt.value === "string" ? evt.value : "unknown"} ${typeof evt.message === "string" ? `(${evt.message})` : ""}`;
    case "completed": {
      const fileName = typeof evt.file_name === "string" ? evt.file_name : "file";
      const sizeBytes = typeof evt.size_bytes === "number" ? evt.size_bytes : 0;
      return `Done: ${fileName} (${formatBytes(sizeBytes)})`;
    }
    case "error":
      return `Error: ${typeof evt.message === "string" ? evt.message : "unknown"}`;
    case "process_end":
      return `Process: ${typeof evt.value === "string" ? evt.value : ""} ${typeof evt.message === "string" ? evt.message : ""}`.trim();
    default:
      return `${evt.kind || "event"}: ${typeof evt.message === "string" ? evt.message : typeof evt.value === "string" ? evt.value : ""}`;
  }
}

function handleTransferEvent(evt: TransferEvent): void {
  if (!evt || typeof evt !== "object") return;

  if (!transferSchemaWarningShown) {
    const compatibility = evaluateSchemaCompatibility(
      TRANSFER_EVENT_SCHEMA_VERSION,
      typeof evt.schema_version === "string" ? evt.schema_version : null
    );
    if (compatibility === "mismatch") {
      transferSchemaWarningShown = true;
      pushLog(
        `Transfer event schema mismatch: expected ${TRANSFER_EVENT_SCHEMA_VERSION}, got ${evt.schema_version}.`
      );
    }
  }

  pushLog(summarizeEvent(evt));

  switch (evt.kind) {
    case "status":
      statusText.textContent = typeof evt.message === "string" ? evt.message : "Working...";
      break;

    case "ticket":
    case "qr_payload":
      void setTicketVisual(typeof evt.value === "string" ? evt.value : "");
      break;

    case "handshake_code":
      handshakeCode.textContent = typeof evt.value === "string" ? evt.value : "- - - -";
      break;

    case "progress": {
      const done = typeof evt.done === "number" ? evt.done : 0;
      const total = typeof evt.total === "number" ? evt.total : 0;
      const pct = total > 0 ? Math.min(100, (done / total) * 100) : 0;
      progressFill.style.width = `${pct.toFixed(1)}%`;
      progressPercent.textContent = `${Math.round(pct)}%`;
      progressDetail.textContent = `${formatBytes(done)} / ${formatBytes(total)}`;
      break;
    }

    case "connection_path": {
      const base = typeof evt.value === "string" ? evt.value : "unknown";
      const details = typeof evt.message === "string" ? ` (${evt.message})` : "";
      const latency = typeof evt.latency_ms === "number" ? `, ${evt.latency_ms.toFixed(1)}ms` : "";
      connectionText.textContent = `${base}${details}${latency}`;
      break;
    }

    case "completed": {
      statusDot.className = "status-dot status-dot--success";
      statusText.textContent = "Transfer complete";
      progressFill.style.width = "100%";
      progressPercent.textContent = "100%";
      p2pViz.classList.add("p2p-viz--done");

      completeSummary.classList.remove("hidden");
      const fileName = typeof evt.file_name === "string" ? evt.file_name : "File";
      const sizeBytes = typeof evt.size_bytes === "number" ? evt.size_bytes : 0;
      let detail = `${fileName} (${formatBytes(sizeBytes)})`;

      const savedPath =
        typeof evt.saved_path === "string"
          ? evt.saved_path
          : typeof evt.saved_to === "string"
            ? evt.saved_to
            : "";
      if (savedPath) detail += ` -> ${savedPath}`;
      completeDetail.textContent = detail;
      break;
    }

    case "error":
      statusDot.className = "status-dot status-dot--error";
      statusText.textContent = "Transfer failed";
      p2pViz.classList.add("p2p-viz--error");
      errorSummary.classList.remove("hidden");
      errorDetail.textContent = typeof evt.message === "string" ? evt.message : "Unknown error";
      break;

    case "process_end":
      isRunning = false;
      if (evt.value === "canceled") {
        switchToConfig();
      } else {
        cancelBtn.classList.add("hidden");
        newTransferBtn.classList.remove("hidden");
      }
      break;

    default:
      break;
  }
}

tabSend.addEventListener("click", () => setMode("send"));
tabReceive.addEventListener("click", () => setMode("receive"));

document.querySelectorAll<HTMLButtonElement>(".variant-btn").forEach((btn) => {
  btn.addEventListener("click", () => {
    const variant = btn.dataset.variant || "";
    if (isTransferMode(variant)) {
      setSubMode(variant);
    }
  });
});

dropZone.addEventListener("click", async () => {
  const desktopApi = getApi();
  if (!desktopApi) return;
  const selected = await desktopApi.pickFile();
  if (selected) {
    filePath = selected;
    const name = selected.split(/[\\/]/).pop();
    updateFileDisplay(name);
  }
});

dropZone.addEventListener("dragover", (e: DragEvent) => {
  e.preventDefault();
  e.stopPropagation();
  dropZone.classList.add("drop-zone--hover");
});

dropZone.addEventListener("dragleave", (e: DragEvent) => {
  e.preventDefault();
  e.stopPropagation();
  dropZone.classList.remove("drop-zone--hover");
});

dropZone.addEventListener("drop", (e: DragEvent) => {
  e.preventDefault();
  e.stopPropagation();
  dropZone.classList.remove("drop-zone--hover");

  const file = e.dataTransfer?.files?.[0];
  if (!file) return;

  if (api && api.getPathForFile) {
    try {
      const droppedPath = api.getPathForFile(file);
      if (droppedPath) {
        filePath = droppedPath;
        updateFileDisplay(file.name, file.size);
        return;
      }
    } catch {
      // fall through
    }
  }

  pushLog("Could not resolve file path from drop. Please use the browse button.");
});

document.body.addEventListener("dragover", (e: DragEvent) => e.preventDefault());
document.body.addEventListener("drop", (e: DragEvent) => e.preventDefault());

pickOutputBtn.addEventListener("click", async () => {
  const desktopApi = getApi();
  if (!desktopApi) return;
  const selected = await desktopApi.pickDir(outputDir.value.trim());
  if (selected) outputDir.value = selected;
});

startBtn.addEventListener("click", () => {
  void startTransfer();
});

cancelBtn.addEventListener("click", async () => {
  const desktopApi = getApi();
  if (!desktopApi) return;
  await desktopApi.cancelTransfer();
});

newTransferBtn.addEventListener("click", switchToConfig);

copyBtn.addEventListener("click", async () => {
  const desktopApi = getApi();
  if (!currentTicket || !desktopApi) return;
  try {
    await desktopApi.copyText(currentTicket);
  } catch {
    pushLog("Failed to copy ticket to clipboard.");
    return;
  }

  const textEl = copyBtn.querySelector<HTMLSpanElement>("span");
  const origText = textEl?.textContent || "Copy";
  copyBtn.classList.add("btn-copy--copied");
  if (textEl) {
    textEl.textContent = "Copied!";
  }

  setTimeout(() => {
    copyBtn.classList.remove("btn-copy--copied");
    if (textEl) {
      textEl.textContent = origText;
    }
  }, 2000);

  pushLog("Ticket copied to clipboard.");
});

qrTrigger.addEventListener("click", () => {
  if (!qrImage.src) return;
  qrOverlay.classList.remove("hidden");
});

qrOverlay.addEventListener("click", () => {
  qrOverlay.classList.add("hidden");
});

sendTicketInput.addEventListener("keydown", (e: KeyboardEvent) => {
  if (e.key === "Enter" && !isRunning) {
    void startTransfer();
  }
});

targetInput.addEventListener("keydown", (e: KeyboardEvent) => {
  if (e.key === "Enter" && !isRunning) {
    void startTransfer();
  }
});

logToggle.addEventListener("click", () => {
  logOpen = !logOpen;
  logPanel.classList.toggle("hidden", !logOpen);
  logChevron.classList.toggle("log-chevron--open", logOpen);
});

clearLogBtn.addEventListener("click", () => {
  eventLog.textContent = "";
});

function init(): void {
  const desktopApi = getApi();
  if (!desktopApi) {
    platformInfo.textContent = "Bridge unavailable";
    return;
  }

  desktopApi.onTransferEvent(handleTransferEvent);
  desktopApi
    .getPlatformInfo()
    .then((info) => {
      platformLabel = `${info.platform} / ${info.arch}`;
      refreshPlatformInfo();
    })
    .catch(() => {
      platformLabel = "Unknown platform";
      refreshPlatformInfo();
    });

  desktopApi
    .getDefaultOutputDir()
    .then((defaultDir) => {
      outputDir.value = defaultDir || ".";
    })
    .catch(() => {
      outputDir.value = ".";
    });

  desktopApi
    .getBuildInfo()
    .then((info) => {
      applyBuildInfoStatus(info);
      if (info.cli.error) {
        pushLog(`CLI probe warning: ${info.cli.error}`);
      }
    })
    .catch((err) => {
      pushLog(`Build info unavailable: ${errorMessage(err)}`);
    });

  setMode("send");
}

init();
