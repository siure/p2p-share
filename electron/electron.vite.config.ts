import { resolve } from "node:path";
import { defineConfig } from "electron-vite";

export default defineConfig({
  main: {
    build: {
      outDir: "out/main",
      sourcemap: true,
      rollupOptions: {
        input: {
          index: resolve(__dirname, "src/main/main.ts")
        },
        output: {
          entryFileNames: "[name].js"
        }
      }
    }
  },
  preload: {
    build: {
      outDir: "out/preload",
      sourcemap: true,
      rollupOptions: {
        input: {
          index: resolve(__dirname, "src/preload/preload.ts")
        },
        output: {
          entryFileNames: "[name].js"
        }
      }
    }
  },
  renderer: {
    root: "src/renderer",
    resolve: {
      alias: {
        "@renderer": resolve(__dirname, "src/renderer"),
        "@shared": resolve(__dirname, "src/shared")
      }
    },
    build: {
      outDir: "out/renderer"
    }
  }
});
