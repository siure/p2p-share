"use strict";

const fs = require("fs");
const path = require("path");

function fail(message) {
  console.error(`FAIL: ${message}`);
  process.exit(1);
}

function info(message) {
  console.log(`INFO: ${message}`);
}

function inferOsKey(unpackedDirName) {
  if (unpackedDirName.startsWith("linux")) return "linux";
  if (unpackedDirName.startsWith("win")) return "win";
  if (unpackedDirName.startsWith("mac")) return "mac";
  return null;
}

function findMacResourcesDir(unpackedDir) {
  const entries = fs.readdirSync(unpackedDir, { withFileTypes: true });
  const appBundle = entries.find((entry) => entry.isDirectory() && entry.name.endsWith(".app"));
  if (!appBundle) return null;
  return path.join(unpackedDir, appBundle.name, "Contents", "Resources");
}

function fileIsExecutable(filePath, osKey) {
  if (osKey === "win") {
    return true;
  }
  const mode = fs.statSync(filePath).mode;
  return (mode & 0o111) !== 0;
}

function verifyUnpackedDir(unpackedDir) {
  const name = path.basename(unpackedDir);
  const osKey = inferOsKey(name);
  if (!osKey) {
    info(`Skipping ${name}: cannot infer target OS.`);
    return { checked: false, ok: true };
  }

  const resourcesDir =
    osKey === "mac" ? findMacResourcesDir(unpackedDir) : path.join(unpackedDir, "resources");
  if (!resourcesDir || !fs.existsSync(resourcesDir)) {
    return { checked: true, ok: false, error: `${name}: resources directory not found.` };
  }

  const binaryName = osKey === "win" ? "p2p-share.exe" : "p2p-share";
  const osBinRoot = path.join(resourcesDir, "bin", osKey);
  if (!fs.existsSync(osBinRoot)) {
    return {
      checked: true,
      ok: false,
      error: `${name}: missing bundled bin root ${path.relative(process.cwd(), osBinRoot)}.`
    };
  }

  const archDirs = fs
    .readdirSync(osBinRoot, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name);

  if (archDirs.length === 0) {
    return { checked: true, ok: false, error: `${name}: no architecture folders inside ${osBinRoot}.` };
  }

  for (const arch of archDirs) {
    const binaryPath = path.join(osBinRoot, arch, binaryName);
    if (!fs.existsSync(binaryPath)) {
      return {
        checked: true,
        ok: false,
        error: `${name}: missing ${binaryName} for arch ${arch} at ${path.relative(process.cwd(), binaryPath)}.`
      };
    }
    if (!fileIsExecutable(binaryPath, osKey)) {
      return {
        checked: true,
        ok: false,
        error: `${name}: binary is not executable at ${path.relative(process.cwd(), binaryPath)}.`
      };
    }
    info(`${name}: verified ${path.relative(process.cwd(), binaryPath)}`);
  }

  return { checked: true, ok: true };
}

function main() {
  const projectRoot = path.resolve(__dirname, "..");
  const distDir = path.join(projectRoot, "dist");
  if (!fs.existsSync(distDir)) {
    fail("dist directory not found. Build first with an electron-builder dist command.");
  }

  const unpackedDirs = fs
    .readdirSync(distDir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && entry.name.endsWith("-unpacked"))
    .map((entry) => path.join(distDir, entry.name));

  if (unpackedDirs.length === 0) {
    fail("no *-unpacked directories found under dist. Build first with `npm run dist`.");
  }

  let checkedCount = 0;
  for (const unpackedDir of unpackedDirs) {
    const result = verifyUnpackedDir(unpackedDir);
    if (!result.checked) {
      continue;
    }
    checkedCount += 1;
    if (!result.ok) {
      fail(result.error);
    }
  }

  if (checkedCount === 0) {
    fail("found unpacked artifacts, but none had a recognizable OS name (linux/win/mac).");
  }

  console.log(`PASS: verified packaged p2p-share CLI in ${checkedCount} unpacked artifact(s).`);
}

main();
