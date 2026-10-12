import { contextBridge, ipcRenderer, webUtils } from "electron";

import type { NearbyEvent, P2PShareApi, StartTransferPayload, TransferEvent, UpdateState } from "../shared/ipc";

const api: P2PShareApi = {
  getUpdateState: () => ipcRenderer.invoke("updates:state"),
  updateAction: () => ipcRenderer.invoke("updates:action"),
  onUpdateState: (handler) => {
    const wrapped = (_event: Electron.IpcRendererEvent, state: UpdateState) => handler(state);
    ipcRenderer.on("updates:state", wrapped);
    return () => ipcRenderer.removeListener("updates:state", wrapped);
  },
  getPlatformInfo: () => ipcRenderer.invoke("app:get-platform-info"),
  getBuildInfo: () => ipcRenderer.invoke("app:get-build-info"),
  getDefaultOutputDir: () => ipcRenderer.invoke("app:get-default-output-dir"),
  pickFiles: () => ipcRenderer.invoke("dialog:pick-files"),
  pickDir: (defaultPath?: string) => ipcRenderer.invoke("dialog:pick-dir", defaultPath),
  startTransfer: (payload: StartTransferPayload) => ipcRenderer.invoke("transfer:start", payload),
  cancelTransfer: () => ipcRenderer.invoke("transfer:cancel"),
  answerOffer: (accept: boolean) => ipcRenderer.invoke("transfer:answer", accept),
  setBrowsing: (on: boolean) => ipcRenderer.invoke("nearby:browse", on),
  onNearbyEvent: (handler: (event: NearbyEvent) => void) => {
    const wrapped = (_event: Electron.IpcRendererEvent, nearbyEvent: NearbyEvent) => handler(nearbyEvent);
    ipcRenderer.on("nearby:event", wrapped);
    return () => ipcRenderer.removeListener("nearby:event", wrapped);
  },
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
