export type TransferMode = "send_wait" | "send_to_ticket" | "receive_target" | "receive_listen";

export interface StartTransferPayload {
  mode: TransferMode;
  filePath?: string;
  ticket?: string;
  target?: string;
  outputDir?: string;
}

export interface TransferEventBase {
  kind: string;
  message?: string;
  value?: string;
}

export interface TransferEventStatus extends TransferEventBase {
  kind: "status";
}

export interface TransferEventTicket extends TransferEventBase {
  kind: "ticket";
}

export interface TransferEventQrPayload extends TransferEventBase {
  kind: "qr_payload";
}

export interface TransferEventHandshakeCode extends TransferEventBase {
  kind: "handshake_code";
}

export interface TransferEventProgress extends TransferEventBase {
  kind: "progress";
  done?: number;
  total?: number;
}

export interface TransferEventConnectionPath extends TransferEventBase {
  kind: "connection_path";
  latency_ms?: number;
}

export interface TransferEventCompleted extends TransferEventBase {
  kind: "completed";
  file_name?: string;
  size_bytes?: number;
  saved_path?: string;
  saved_to?: string;
}

export interface TransferEventError extends TransferEventBase {
  kind: "error";
}

export interface TransferEventProcessEnd extends TransferEventBase {
  kind: "process_end";
  value?: "canceled" | "completed" | string;
}

export interface TransferEventUnknown extends TransferEventBase {
  [key: string]: unknown;
}

export type TransferEvent =
  | TransferEventStatus
  | TransferEventTicket
  | TransferEventQrPayload
  | TransferEventHandshakeCode
  | TransferEventProgress
  | TransferEventConnectionPath
  | TransferEventCompleted
  | TransferEventError
  | TransferEventProcessEnd
  | TransferEventUnknown;

export interface PlatformInfo {
  platform: NodeJS.Platform;
  arch: string;
  cwd: string;
}

export interface P2PShareApi {
  getPlatformInfo: () => Promise<PlatformInfo>;
  getDefaultOutputDir: () => Promise<string>;
  pickFile: () => Promise<string | null>;
  pickDir: (defaultPath?: string) => Promise<string | null>;
  startTransfer: (payload: StartTransferPayload) => Promise<{ ok: boolean }>;
  cancelTransfer: () => Promise<{ ok: boolean }>;
  onTransferEvent: (handler: (event: TransferEvent) => void) => () => void;
  copyText: (text: string) => Promise<{ ok: boolean }>;
  createQrDataUrl: (text: string) => Promise<string | null>;
  getPathForFile: (file: File) => string;
}
