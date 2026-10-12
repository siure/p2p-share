import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { randomUUID } from "node:crypto";
import readline from "node:readline";

import type { NearbyEvent, TransferEvent } from "../shared/ipc";
import { TRANSFER_EVENT_SCHEMA_VERSION } from "../shared/schema";

export type WorkerTransfer =
  | { command: "send"; files: string[]; to?: string; nearby?: string }
  | { command: "receive"; target?: string; output: string; nearby?: boolean };

interface WorkerOptions {
  command: string;
  argsPrefix: string[];
  cwd: string;
  /** Name shown to nearby devices. */
  deviceName?: string;
  onEvent: (event: TransferEvent) => void;
  onNearby?: (event: NearbyEvent) => void;
  startupTimeoutMs?: number;
  stopTimeoutMs?: number;
}

/** Owns one reusable engine process; transfer IDs isolate late events after cancellation. */
export class WorkerClient {
  private child: ChildProcessWithoutNullStreams | null = null;
  private active: { id: string; canceled: boolean } | null = null;
  private startupTimer: NodeJS.Timeout | null = null;
  private cancelTimer: NodeJS.Timeout | null = null;
  private closing = false;
  private browsing = false;

  constructor(private readonly options: WorkerOptions) {}

  get busy(): boolean {
    return this.active !== null;
  }

  warmup(): void {
    if (this.child || this.closing) return;
    const name = this.options.deviceName ? ["--name", this.options.deviceName] : [];
    const child = spawn(this.options.command, [...this.options.argsPrefix, "--json", ...name, "worker"], {
      cwd: this.options.cwd,
      stdio: ["pipe", "pipe", "pipe"]
    });
    this.child = child;
    let failure = "";
    let stderr = "";

    this.startupTimer = setTimeout(() => {
      failure = "Transfer engine did not become ready in time.";
      child.kill("SIGKILL");
    }, this.options.startupTimeoutMs ?? 30_000);

    const lines = readline.createInterface({ input: child.stdout });
    lines.on("line", (line) => {
      if (this.child !== child) return;
      let event: TransferEvent;
      try {
        event = JSON.parse(line);
      } catch {
        return;
      }
      if (!event || typeof event.kind !== "string") return;
      if (event.kind === "worker_ready") {
        this.clearStartupTimer();
        return;
      }
      if (event.kind === "nearby_found" || event.kind === "nearby_lost") {
        this.options.onNearby?.(event as unknown as NearbyEvent);
        return;
      }
      if (!this.active || event.transfer_id !== this.active.id) return;
      if (event.kind === "process_end") {
        this.clearCancelTimer();
        this.active = null;
      }
      this.options.onEvent(event);
    });
    child.stderr.on("data", (chunk: Buffer) => {
      stderr = (stderr + chunk.toString()).slice(-4096);
    });
    child.on("error", (error) => {
      failure = `Could not start transfer engine: ${error.message}`;
    });
    child.stdin.on("error", (error) => {
      failure = `Could not communicate with transfer engine: ${error.message}`;
      child.kill("SIGKILL");
    });
    child.on("close", (code, signal) => {
      lines.close();
      if (this.child !== child) return;
      this.child = null;
      // The next process starts without a browse; ask for it again.
      if (this.browsing && !this.closing) queueMicrotask(() => this.setBrowsing(true));
      this.clearStartupTimer();
      this.clearCancelTimer();
      const active = this.active;
      this.active = null;
      if (!active || this.closing) return;
      if (!active.canceled) {
        this.options.onEvent({
          kind: "error",
          transfer_id: active.id,
          message: failure || `Transfer engine stopped unexpectedly (${signal || code}). ${stderr.trim()}`,
          schema_version: TRANSFER_EVENT_SCHEMA_VERSION
        });
      }
      this.options.onEvent({
        kind: "process_end",
        transfer_id: active.id,
        value: active.canceled ? "canceled" : "failed",
        schema_version: TRANSFER_EVENT_SCHEMA_VERSION
      });
    });
  }

  start(transfer: WorkerTransfer): void {
    if (this.closing) throw new Error("Transfer engine is shutting down.");
    if (this.busy) throw new Error("A transfer is already running.");
    this.warmup();
    const id = randomUUID();
    this.active = { id, canceled: false };
    this.child!.stdin.write(`${JSON.stringify({ id, ...transfer })}\n`);
  }

  /** Discovery runs alongside transfers until turned off. */
  setBrowsing(on: boolean): void {
    if (this.closing) return;
    this.browsing = on;
    if (on) this.warmup();
    this.child?.stdin.write(`${JSON.stringify({ command: on ? "browse" : "stop_browse" })}\n`);
  }

  /** Accept or decline the active transfer's `offer`. */
  answer(accept: boolean): boolean {
    if (!this.active || !this.child) return false;
    this.child.stdin.write(`${JSON.stringify({ id: this.active.id, command: "answer", accept })}\n`);
    return true;
  }

  cancel(): boolean {
    if (!this.active || !this.child) return false;
    if (this.active.canceled) return true;
    this.active.canceled = true;
    const child = this.child;
    child.stdin.write(`${JSON.stringify({ id: this.active.id, command: "cancel" })}\n`);
    this.cancelTimer = setTimeout(() => {
      if (this.child === child && this.active?.canceled) child.kill("SIGKILL");
    }, this.options.stopTimeoutMs ?? 5000);
    return true;
  }

  async shutdown(): Promise<void> {
    this.closing = true;
    const child = this.child;
    if (!child) return;
    await new Promise<void>((resolve) => {
      const timer = setTimeout(() => child.kill("SIGKILL"), this.options.stopTimeoutMs ?? 5000);
      child.once("close", () => {
        clearTimeout(timer);
        resolve();
      });
      child.stdin.end(`${JSON.stringify({ command: "shutdown" })}\n`);
    });
  }

  private clearStartupTimer(): void {
    if (this.startupTimer) clearTimeout(this.startupTimer);
    this.startupTimer = null;
  }

  private clearCancelTimer(): void {
    if (this.cancelTimer) clearTimeout(this.cancelTimer);
    this.cancelTimer = null;
  }
}
