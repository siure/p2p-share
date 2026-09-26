import type { AppUpdater } from "electron-updater";
import type { UpdateState } from "../shared/ipc";
import path from "node:path";

export function isAppImageLaunch(executable: string, appDir: string | undefined, appImage: string | undefined): boolean {
  // APPIMAGE can be inherited from a different app that launched this process.
  return Boolean(appDir && appImage && executable.startsWith(path.resolve(appDir) + path.sep));
}

type Updater = Pick<AppUpdater,
  "autoDownload" | "autoInstallOnAppQuit" | "allowPrerelease" | "allowDowngrade" |
  "on" | "checkForUpdates" | "downloadUpdate" | "quitAndInstall">;

/** Keeps installation explicit, including when a downloaded update is cached. */
export class Updates {
  state: UpdateState = { status: "idle" };

  constructor(
    private readonly updater: Updater,
    private readonly changed: (state: UpdateState) => void,
    private readonly busy: () => boolean,
    private readonly beforeInstall: () => Promise<void>,
    private readonly installFailed: () => void,
  ) {
    updater.autoDownload = false;
    updater.autoInstallOnAppQuit = false;
    updater.allowPrerelease = false;
    updater.allowDowngrade = false;
    updater.on("update-available", (info) => this.set({ status: "available", version: info.version }));
    updater.on("update-not-available", () => this.set({ status: "idle" }));
    updater.on("download-progress", (progress) => {
      this.set({ ...this.state, status: "downloading", percent: Math.floor(progress.percent) });
    });
    updater.on("update-downloaded", (info) => this.set({ status: "ready", version: info.version }));
    updater.on("error", () => this.fail());
  }

  private set(state: UpdateState): void {
    this.state = state;
    this.changed(state);
  }

  private fail(): void {
    if (this.state.status === "installing") this.installFailed();
    this.set({ status: "error", message: "Update failed. Try again." });
  }

  async check(): Promise<void> {
    if (!["idle", "error"].includes(this.state.status)) return;
    this.set({ status: "checking" });
    try {
      await this.updater.checkForUpdates();
    } catch {
      this.fail();
    }
  }

  async download(): Promise<void> {
    if (this.state.status !== "available") return;
    this.set({ ...this.state, status: "downloading", percent: 0 });
    try {
      await this.updater.downloadUpdate();
    } catch {
      this.fail();
    }
  }

  async install(): Promise<void> {
    if (this.state.status !== "ready" || this.busy()) return;
    this.set({ ...this.state, status: "installing" });
    try {
      await this.beforeInstall();
      this.updater.quitAndInstall(false, true);
    } catch {
      this.fail();
    }
  }
}
