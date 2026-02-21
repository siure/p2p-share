import type { P2PShareApi } from "../shared/ipc";

declare global {
  interface Window {
    p2pShareApi?: P2PShareApi;
  }
}

export {};
