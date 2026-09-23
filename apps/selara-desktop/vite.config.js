import { defineConfig } from "vite";

const host = process.env.TAURI_DEV_HOST;

// `npm run dev:mock` serves the Settings UI with a fake Tauri bridge so it runs
// in a plain browser. Dev server only: `vite build` never applies this plugin.
const mockTauriBridge = {
  name: "selara-mock-tauri",
  apply: "serve",
  transformIndexHtml() {
    if (process.env.SELARA_MOCK !== "1") return;
    return [{ tag: "script", attrs: { src: "/dev/mock-tauri.js" }, injectTo: "head-prepend" }];
  },
};

export default defineConfig({
  clearScreen: false,
  plugins: [mockTauriBridge],
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_"],
  build: {
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    minify: !process.env.TAURI_ENV_DEBUG ? "esbuild" : false,
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },
});
