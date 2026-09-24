import { defineConfig, devices } from "@playwright/test";

// Local-only smoke suite for the Settings UI against the mock Tauri bridge
// (`npm run dev:mock`). WebKit matches the WKWebView Tauri uses on macOS.
// Sizes mirror the settings window in src-tauri/tauri.conf.json. The suite
// serves the mock on its own port so it never reuses a `npm run dev` or
// `tauri dev` server on 1420, which has no mock bridge.
const PORT = 1430;
const sizes = { default: { width: 920, height: 640 }, min: { width: 760, height: 520 } };
const schemes = ["light", "dark"];

export default defineConfig({
  testDir: "e2e",
  testMatch: "**/*.spec.mjs",
  fullyParallel: true,
  reporter: [["list"], ["html", { open: "never" }]],
  use: {
    baseURL: `http://localhost:${PORT}`,
    trace: "retain-on-failure",
  },
  projects: Object.entries(sizes).flatMap(([size, viewport]) =>
    schemes.map((colorScheme) => ({
      name: `${size}-${colorScheme}`,
      use: { ...devices["Desktop Safari"], viewport, deviceScaleFactor: 2, colorScheme },
    })),
  ),
  webServer: {
    command: `npm run dev:mock -- --port ${PORT}`,
    url: `http://localhost:${PORT}`,
    reuseExistingServer: false,
    timeout: 60_000,
  },
});
