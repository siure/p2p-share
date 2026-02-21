import { contextBridge, ipcRenderer, webUtils } from "electron";

import type { P2PShareApi, StartTransferPayload, TransferEvent } from "../shared/ipc";

const api: P2PShareApi = {
  getPlatformInfo: () => ipcRenderer.invoke("app:get-platform-info"),
  getDefaultOutputDir: () => ipcRenderer.invoke("app:get-default-output-dir"),
  pickFile: () => ipcRenderer.invoke("dialog:pick-file"),
  pickDir: (defaultPath?: string) => ipcRenderer.invoke("dialog:pick-dir", defaultPath),
  startTransfer: (payload: StartTransferPayload) => ipcRenderer.invoke("transfer:start", payload),
  cancelTransfer: () => ipcRenderer.invoke("transfer:cancel"),
  onTransferEvent: (handler: (event: TransferEvent) => void) => {
    const wrapped = (_event: Electron.IpcRendererEvent, transferEvent: TransferEvent) => {
      handler(transferEvent);
    };
    ipcRenderer.on("transfer:event", wrapped);
    return () => ipcRenderer.removeListener("transfer:event", wrapped);
  },
  copyText: (text: string) => ipcRenderer.invoke("clipboard:write", text),
  createQrDataUrl: (text: string) => ipcRenderer.invoke("qr:create-data-url", text),
  getPathForFile: (file: File) => webUtils.getPathForFile(file)
};

contextBridge.exposeInMainWorld("p2pShareApi", api);
