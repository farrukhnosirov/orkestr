import { fileURLToPath } from "node:url";

import { defineConfig } from "vitest/config";

const pkg = (name: string) => fileURLToPath(new URL(`../../packages/${name}/src/index.ts`, import.meta.url));

export default defineConfig({
  resolve: {
    alias: [
      { find: /^@orkestr\/protocol$/, replacement: pkg("protocol") },
      { find: /^@orkestr\/client$/, replacement: pkg("client") },
      { find: /^@orkestr\/ui$/, replacement: pkg("ui") },
    ],
  },
  test: { environment: "jsdom", include: ["src/**/*.test.{ts,tsx}"], css: false },
});
