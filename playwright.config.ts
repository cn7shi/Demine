import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/browser",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 30_000,
  use: {
    baseURL: "http://127.0.0.1:4318",
    channel: process.platform === "win32" ? "msedge" : undefined,
    viewport: { width: 1440, height: 1080 },
    trace: "retain-on-failure",
  },
  webServer: {
    command: "node scripts/browser-server.mjs",
    url: "http://127.0.0.1:4318/api/status",
    reuseExistingServer: false,
    timeout: 30_000,
  },
});
