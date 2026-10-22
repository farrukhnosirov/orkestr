import { fileURLToPath } from "node:url";

import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

import { shouldEmitSourcemap } from "./src/build/sourcemap";

const pkg = (name: string) => fileURLToPath(new URL(`../../packages/${name}/src/index.ts`, import.meta.url));

// Tauri expects a fixed dev port and must not clear its own console output.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  resolve: {
    // Consume workspace packages from source (no pre-build needed in dev).
    alias: [
      { find: /^@orkestr\/protocol$/, replacement: pkg("protocol") },
      { find: /^@orkestr\/client$/, replacement: pkg("client") },
      { find: /^@orkestr\/ui$/, replacement: pkg("ui") },
    ],
  },
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1",
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    target: "es2022",
    outDir: "dist",
    emptyOutDir: true,
    // Tauri embeds all of dist/ into the binary: no .map files in release builds.
    sourcemap: shouldEmitSourcemap(process.env),
    chunkSizeWarningLimit: 2048,
    rollupOptions: {
      // One entry for every window (main, HUD, orb, detached panels).
      input: {
        main: fileURLToPath(new URL("./index.html", import.meta.url)),
      },
    },
  },
});
