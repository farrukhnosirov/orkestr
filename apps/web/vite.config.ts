import { fileURLToPath } from "node:url";

import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const pkg = (name: string) => fileURLToPath(new URL(`../../packages/${name}/src/index.ts`, import.meta.url));

// Served by `orkestr-gateway --web-dist apps/web/dist` (same origin as the gateway API).
export default defineConfig({
  plugins: [react()],
  resolve: {
    // Consume workspace packages from source (no pre-build needed).
    alias: [
      { find: /^@orkestr\/protocol$/, replacement: pkg("protocol") },
      { find: /^@orkestr\/client$/, replacement: pkg("client") },
      { find: /^@orkestr\/ui$/, replacement: pkg("ui") },
    ],
  },
  server: { port: 5174, strictPort: true, host: "127.0.0.1" },
  build: { target: "es2022", outDir: "dist", emptyOutDir: true },
});
