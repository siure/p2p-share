const { test } = require("node:test");
const assert = require("node:assert/strict");
const { EventEmitter } = require("node:events");
const { Updates, isAppImageLaunch } = require("../out/tests/main/updates.js");
const path = require("node:path");

test("does not update another AppImage inherited from the parent process", () => {
  const mount = path.resolve("mount");
  assert.equal(isAppImageLaunch(path.join(mount, "app"), mount, "/App.AppImage"), true);
  assert.equal(isAppImageLaunch(path.resolve("unpacked/app"), mount, "/Other.AppImage"), false);
  assert.equal(isAppImageLaunch(path.join(mount + "-other", "app"), mount, "/Other.AppImage"), false);
  assert.equal(isAppImageLaunch(path.join(mount, "app"), undefined, "/Other.AppImage"), false);
});

function setup() {
  const updater = new EventEmitter();
  const calls = [];
  updater.checkForUpdates = async () => { calls.push("check"); };
  updater.downloadUpdate = async () => { calls.push("download"); };
  updater.quitAndInstall = () => { calls.push("install"); };
  let busy = false;
  const updates = new Updates(updater, () => {}, () => busy,
    async () => { calls.push("shutdown"); }, () => { calls.push("recover"); });
  return { updater, updates, calls, setBusy: (value) => { busy = value; } };
}

test("checks once, ignores repeated actions, and only downloads on request", async () => {
  const { updater, updates, calls } = setup();
  assert.equal(updater.autoDownload, false);
  assert.equal(updater.autoInstallOnAppQuit, false);
  assert.equal(updater.allowDowngrade, false);
  assert.equal(updater.allowPrerelease, false);
  await updates.check();
  await updates.check();
  updater.emit("update-available", { version: "0.5.0" });
  assert.deepEqual(calls, ["check"]);
  await updates.download();
  await updates.download();
  assert.deepEqual(calls, ["check", "download"]);
  updater.emit("download-progress", { percent: 38.9 });
  assert.equal(updates.state.percent, 38);
  updater.emit("update-downloaded", { version: "0.5.0" });
  assert.equal(updates.state.status, "ready");
  assert.deepEqual(calls, ["check", "download"]);
});

test("does not install during a transfer; stops the engine before installing once", async () => {
  const { updater, updates, calls, setBusy } = setup();
  await updates.install();
  assert.deepEqual(calls, []);
  updater.emit("update-downloaded", { version: "0.5.0" });
  setBusy(true);
  await updates.install();
  assert.equal(updates.state.status, "ready");
  assert.deepEqual(calls, []);
  setBusy(false);
  await Promise.all([updates.install(), updates.install()]);
  assert.deepEqual(calls, ["shutdown", "install"]);
});

test("network failures can be retried without enabling automatic installation", async () => {
  const { updater, updates } = setup();
  updater.checkForUpdates = async () => { throw new Error("offline"); };
  await updates.check();
  assert.equal(updates.state.status, "error");
  updater.checkForUpdates = async () => updater.emit("update-not-available");
  await updates.check();
  assert.equal(updates.state.status, "idle");
  assert.equal(updater.autoInstallOnAppQuit, false);
});

test("installer failure releases the shutdown guard and allows retry", async () => {
  const { updater, updates, calls } = setup();
  updater.emit("update-downloaded", { version: "0.5.0" });
  updater.quitAndInstall = () => updater.emit("error", new Error("installer failed"));
  await updates.install();
  assert.equal(updates.state.status, "error");
  assert.deepEqual(calls, ["shutdown", "recover"]);
  await updates.check();
  assert.equal(updates.state.status, "checking");
});
