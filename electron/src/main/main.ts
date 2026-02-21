import { app, BrowserWindow, clipboard, dialog, ipcMain, Menu } from "electron";
import { spawn, type ChildProcessByStdio } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import readline from "node:readline";
import type { Readable } from "node:stream";
import QRCode from "qrcode";

import type { StartTransferPayload, TransferEvent, TransferMode } from "../shared/ipc";

let mainWindow: BrowserWindow | null = null;

interface ActiveTransfer {
  child: ChildProcessByStdio<null, Readable, Readable>;
  mode: TransferMode;
  canceled: boolean;
  forceKillTimer: NodeJS.Timeout | null;
}

let activeTransfer: ActiveTransfer | null = null;

function cliBinaryName(): string {
  return process.platform === "win32" ? "p2p-share.exe" : "p2p-share";
}

function repoRoot(): string {
  return path.resolve(__dirname, "..", "..");
}

function packagedOsName(): string {
  switch (process.platform) {
    case "darwin":
      return "mac";
    case "win32":
      return "win";
    case "linux":
      return "linux";
    default:
      return process.platform;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function trimOrEmpty(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function isTransferMode(value: string): value is TransferMode {
  return (
    value === "send_wait" ||
    value === "send_to_ticket" ||
    value === "receive_target" ||
    value === "receive_listen"
  );
}

function parseTransferPayload(payload: unknown): StartTransferPayload {
  if (!isRecord(payload)) {
    throw new Error("Transfer payload is invalid.");
  }

  const mode = trimOrEmpty(payload.mode);
  if (!isTransferMode(mode)) {
    throw new Error("Unsupported transfer mode.");
  }

  return {
    mode,
    filePath: trimOrEmpty(payload.filePath),
    ticket: trimOrEmpty(payload.ticket),
    target: trimOrEmpty(payload.target),
    outputDir: trimOrEmpty(payload.outputDir)
  };
}

function emitTransferEvent(event: TransferEvent): void {
  if (!mainWindow || mainWindow.isDestroyed()) {
    return;
  }
  mainWindow.webContents.send("transfer:event", event);
}

function existingFile(filePath: string): boolean {
  return Boolean(filePath && fs.existsSync(filePath));
}

function resolveCliCommand(): { command: string; argsPrefix: string[]; source: string } {
  const binary = cliBinaryName();
  const root = repoRoot();
  const envPath = trimOrEmpty(process.env.P2P_SHARE_CLI_PATH);
  const packagedPaths = [
    path.join(process.resourcesPath, "bin", packagedOsName(), process.arch, binary),
    path.join(process.resourcesPath, "bin", process.platform, process.arch, binary)
  ];
  const localRelease = path.join(root, "target", "release", binary);
  const localDebug = path.join(root, "target", "debug", binary);

  if (envPath) {
    return { command: envPath, argsPrefix: [], source: "env" };
  }
  for (const packagedPath of packagedPaths) {
    if (existingFile(packagedPath)) {
      return { command: packagedPath, argsPrefix: [], source: "packaged" };
    }
  }
  if (existingFile(localRelease)) {
    return { command: localRelease, argsPrefix: [], source: "local-release" };
  }
  if (existingFile(localDebug)) {
    return { command: localDebug, argsPrefix: [], source: "local-debug" };
  }

  return {
    command: process.platform === "win32" ? "cargo.exe" : "cargo",
    argsPrefix: ["run", "-q", "-p", "p2p-share", "--"],
    source: "cargo-run"
  };
}

function buildTransferArgs(payload: StartTransferPayload): string[] {
  const mode = payload.mode;
  const filePath = trimOrEmpty(payload.filePath);
  const ticket = trimOrEmpty(payload.ticket);
  const target = trimOrEmpty(payload.target);
  const outputDir = trimOrEmpty(payload.outputDir);

  switch (mode) {
    case "send_wait":
      if (!filePath) {
        throw new Error("File path is required.");
      }
      return ["send", filePath];
    case "send_to_ticket":
      if (!filePath || !ticket) {
        throw new Error("File path and ticket are required.");
      }
      return ["send", filePath, "--to", ticket];
    case "receive_target":
      if (!target || !outputDir) {
        throw new Error("Target and output directory are required.");
      }
      return ["receive", target, "--output", outputDir];
    case "receive_listen":
      if (!outputDir) {
        throw new Error("Output directory is required.");
      }
      return ["receive", "--qr", "--output", outputDir];
    default:
      throw new Error("Unsupported transfer mode.");
  }
}

function attachLineStream(stream: NodeJS.ReadableStream | null, onLine: (line: string) => void): void {
  if (!stream) {
    return;
  }
  const rl = readline.createInterface({ input: stream });
  rl.on("line", onLine);
}

function cleanupTransfer(): void {
  if (activeTransfer?.forceKillTimer) {
    clearTimeout(activeTransfer.forceKillTimer);
  }
  activeTransfer = null;
}

function emitStatus(message: string, value: string): void {
  emitTransferEvent({ kind: "status", message, value });
}

function startTransfer(payload: StartTransferPayload): void {
  if (activeTransfer?.child && activeTransfer.child.exitCode === null) {
    throw new Error("A transfer is already running.");
  }

  const transferArgs = buildTransferArgs(payload);
  const cli = resolveCliCommand();
  const args = [...cli.argsPrefix, "--json", ...transferArgs];
  const child = spawn(cli.command, args, {
    cwd: repoRoot(),
    stdio: ["ignore", "pipe", "pipe"]
  });

  let sawStructuredEvent = false;
  let sawStructuredError = false;

  activeTransfer = {
    child,
    mode: payload.mode,
    canceled: false,
    forceKillTimer: null
  };

  emitStatus(`Transfer started via ${cli.source}.`, cli.command);

  attachLineStream(child.stdout, (line) => {
    const text = line.trim();
    if (!text) {
      return;
    }

    try {
      const parsedUnknown: unknown = JSON.parse(text);
      if (!isRecord(parsedUnknown) || typeof parsedUnknown.kind !== "string") {
        throw new Error("Invalid JSON event shape.");
      }
      const parsed = parsedUnknown as TransferEvent;
      sawStructuredEvent = true;
      if (parsed.kind === "error") {
        sawStructuredError = true;
      }
      emitTransferEvent(parsed);
    } catch {
      if (!sawStructuredEvent) {
        emitStatus(text, "stdout");
      }
    }
  });

  attachLineStream(child.stderr, (line) => {
    const text = line.trim();
    if (!text) {
      return;
    }
    if (!sawStructuredEvent) {
      emitStatus(text, "stderr");
      return;
    }

    if (text.startsWith("Error:") && !sawStructuredError) {
      sawStructuredError = true;
      emitTransferEvent({
        kind: "error",
        message: text.replace(/^Error:\s*/, ""),
        value: "stderr_error"
      });
    }
  });

  child.on("error", (err: Error) => {
    emitTransferEvent({
      kind: "error",
      message: `Failed to start transfer process: ${err.message}`,
      value: "spawn_error"
    });
    cleanupTransfer();
  });

  child.on("close", (code: number | null, signal: NodeJS.Signals | null) => {
    const wasCanceled = Boolean(activeTransfer?.canceled);
    cleanupTransfer();

    if (wasCanceled) {
      emitStatus("Transfer canceled by user.", "canceled");
    } else if (code !== 0 && !sawStructuredError) {
      emitTransferEvent({
        kind: "error",
        message: `Transfer process exited with code ${code}${signal ? ` (signal ${signal})` : ""}.`,
        value: "process_exit"
      });
    } else {
      emitStatus("Transfer process finished.", "process_exit");
    }

    emitTransferEvent({
      kind: "process_end",
      message: signal ? `signal ${signal}` : `code ${code ?? 0}`,
      value: wasCanceled ? "canceled" : "completed"
    });
  });
}

function cancelTransfer(): boolean {
  if (!activeTransfer?.child || activeTransfer.child.exitCode !== null) {
    return false;
  }
  const child = activeTransfer.child;
  activeTransfer.canceled = true;
  child.kill("SIGTERM");

  if (!activeTransfer.forceKillTimer) {
    activeTransfer.forceKillTimer = setTimeout(() => {
      if (activeTransfer?.child !== child || child.exitCode !== null) {
        return;
      }
      child.kill("SIGKILL");
      emitStatus("Transfer did not stop gracefully; forcing shutdown.", "force_kill");
    }, 5000);
  }
  return true;
}

function createWindow(): void {
  mainWindow = new BrowserWindow({
    width: 1180,
    height: 840,
    minWidth: 980,
    minHeight: 700,
    backgroundColor: "#08080c",
    webPreferences: {
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
      webSecurity: true,
      preload: path.join(__dirname, "../preload/index.js")
    }
  });

  const devServerUrl = process.env.ELECTRON_RENDERER_URL || process.env.VITE_DEV_SERVER_URL;
  const allowedDevOrigin = devServerUrl ? new URL(devServerUrl).origin : "";

  mainWindow.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
  mainWindow.webContents.on("will-navigate", (event, url) => {
    const isFile = url.startsWith("file://");
    const isAllowedDevNavigation = allowedDevOrigin !== "" && url.startsWith(allowedDevOrigin);
    if (!isFile && !isAllowedDevNavigation) {
      event.preventDefault();
    }
  });

  mainWindow.webContents.session.setPermissionRequestHandler((_webContents, _permission, callback) => {
    callback(false);
  });

  if (devServerUrl) {
    void mainWindow.loadURL(devServerUrl);
  } else {
    void mainWindow.loadFile(path.join(__dirname, "../renderer/index.html"));
  }
}

ipcMain.handle("app:get-platform-info", () => ({
  platform: process.platform,
  arch: process.arch,
  cwd: repoRoot()
}));

ipcMain.handle("app:get-default-output-dir", () => {
  try {
    const downloads = trimOrEmpty(app.getPath("downloads"));
    if (downloads) {
      return downloads;
    }
    const home = trimOrEmpty(app.getPath("home"));
    if (home) {
      return home;
    }
  } catch {
    // Fall through to repo root.
  }
  return repoRoot();
});

ipcMain.handle("qr:create-data-url", async (_event, text: unknown) => {
  const payload = trimOrEmpty(text);
  if (!payload) {
    return null;
  }
  return QRCode.toDataURL(payload, {
    margin: 1,
    width: 320,
    errorCorrectionLevel: "M"
  });
});

ipcMain.handle("dialog:pick-file", async () => {
  if (!mainWindow) {
    return null;
  }
  const result = await dialog.showOpenDialog(mainWindow, {
    title: "Select File to Send",
    properties: ["openFile"]
  });
  if (result.canceled || result.filePaths.length === 0) {
    return null;
  }
  return result.filePaths[0] ?? null;
});

ipcMain.handle("dialog:pick-dir", async (_event, defaultPath: unknown) => {
  if (!mainWindow) {
    return null;
  }
  const result = await dialog.showOpenDialog(mainWindow, {
    title: "Select Output Directory",
    defaultPath: trimOrEmpty(defaultPath) || undefined,
    properties: ["openDirectory", "createDirectory"]
  });
  if (result.canceled || result.filePaths.length === 0) {
    return null;
  }
  return result.filePaths[0] ?? null;
});

ipcMain.handle("clipboard:write", async (_event, text: unknown) => {
  const value = typeof text === "string" ? text : "";
  clipboard.writeText(value);
  return { ok: true };
});

ipcMain.handle("transfer:start", async (_event, payload: unknown) => {
  startTransfer(parseTransferPayload(payload));
  return { ok: true };
});

ipcMain.handle("transfer:cancel", async () => ({
  ok: cancelTransfer()
}));

app.whenReady().then(() => {
  Menu.setApplicationMenu(null);
  createWindow();
  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) {
      createWindow();
    }
  });
});

app.on("window-all-closed", () => {
  cancelTransfer();
  if (process.platform !== "darwin") {
    app.quit();
  }
});
