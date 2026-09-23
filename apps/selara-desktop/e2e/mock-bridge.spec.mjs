import { test, expect } from "@playwright/test";
import { openSettings, showSection } from "./helpers.mjs";

// The mock stands in for the native backend, so these check it behaves like
// the Rust handlers it imitates rather than like a stub.
const invoke = (page, cmd, args) => page.evaluate(([c, a]) => window.__TAURI__.core.invoke(c, a), [cmd, args]);

test.describe("mock bridge", () => {
  // A backend contract, not a layout check: one project is enough.
  test.beforeEach(({}, testInfo) => {
    test.skip(testInfo.project.name !== "default-light", "backend contract runs once");
  });

  test("uses the documented 150 ms latency unless delay is given", async ({ page }) => {
    await page.goto("/?scenario=configured");
    await expect(page.locator("#save-status")).toHaveText("Loaded", { timeout: 15_000 });
    const elapsed = await page.evaluate(async () => {
      const start = performance.now();
      await window.__TAURI__.core.invoke("app_version");
      return performance.now() - start;
    });
    expect(elapsed).toBeGreaterThanOrEqual(140);
  });

  test("fresh starts from the backend defaults", async ({ page }) => {
    await openSettings(page, "fresh");
    const config = await invoke(page, "get_config");
    expect(config.provider).toMatchObject({ kind: "open_ai_compatible", base_url: "https://api.openai.com/v1", model: "gpt-4o-mini" });
    expect(config.commands.map((c) => c.id)).toEqual(["proofread", "rewrite", "friendly", "professional", "concise", "summary", "key_points", "table", "translate"]);
    expect(await invoke(page, "api_key_source")).toBe("none");
  });

  test("tracks stored keys per provider kind", async ({ page }) => {
    await openSettings(page);
    expect(await invoke(page, "api_key_source")).toBe("keychain");
    const provider = { kind: "anthropic", enabled: true, base_url: "https://api.anthropic.com", model: "claude-sonnet-5", api_key: null, auth: "api_key", codex_home: null, cli_binary: null };
    await invoke(page, "save_config_section", { section: "provider", value: provider });
    expect(await invoke(page, "api_key_source")).toBe("none");
    expect(await invoke(page, "store_api_key", { kind: "anthropic", apiKey: "sk-ant-test" })).toBe("keychain");
    expect(await invoke(page, "clear_api_key", { kind: "anthropic" })).toBe("none");
  });

  test("imports commands the way merge_commands does", async ({ page }) => {
    await openSettings(page);
    const keepBoth = await invoke(page, "import_commands", { mode: "keep_both" });
    expect(keepBoth).toEqual({ added: 2, replaced: 0, skipped: 0, renamed: [["rewrite", "rewrite-2"]], hotkeys_dropped: 1 });
    const config = await invoke(page, "get_config");
    expect(config.commands.find((c) => c.id === "rewrite-2").label).toBe("Rewrite (imported)");
    expect(config.commands.find((c) => c.id === "summary").hotkey).toBeNull();
    const skip = await invoke(page, "import_commands", { mode: "skip" });
    expect(skip).toMatchObject({ added: 0, skipped: 2 });
  });

  test("clearing usage leaves costs unavailable, not zero", async ({ page }) => {
    await openSettings(page);
    await showSection(page, "usage");
    await page.evaluate(() => { window.confirm = () => true; });
    await page.locator("#usage-clear").click();
    await expect(page.locator("#section-usage .usage-table tbody tr").first()).toContainText("n/a");
  });

  test("installing an update walks through the native lifecycle", async ({ page }) => {
    await openSettings(page, "update");
    await page.locator("#status-install-update").click();
    await expect(page.locator("#status-updates-state")).toHaveText("Installing…", { timeout: 5_000 });
  });
});
