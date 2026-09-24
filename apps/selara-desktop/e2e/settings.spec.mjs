import { test, expect } from "@playwright/test";
import { SECTIONS, openSettings, showSection, artifactPath, settle } from "./helpers.mjs";

const SCENARIOS = ["configured", "fresh", "update", "errors"];

for (const scenario of SCENARIOS) {
  test(`${scenario}: every section renders without horizontal overflow`, async ({ page }, testInfo) => {
    const problems = await openSettings(page, scenario);
    for (const name of SECTIONS) {
      const section = await showSection(page, name);
      await expect(section.locator("h1").first()).toBeVisible();
      await expect(page.locator("#section-status h1")).not.toHaveText("Settings unavailable");

      const overflow = await page.evaluate((id) => {
        const offenders = [];
        const check = (el, label) => {
          if (el && el.scrollWidth > el.clientWidth + 1) offenders.push(`${label} ${el.scrollWidth}>${el.clientWidth}`);
        };
        check(document.querySelector("main.content"), "main.content");
        document.querySelectorAll(`#section-${id} .toolbar, #section-${id} .status-toolbar`).forEach((el, i) => check(el, `toolbar[${i}]`));
        return offenders;
      }, name);
      expect(overflow, `horizontal overflow in ${name}`).toEqual([]);

      // Viewport capture, then a tall capture so the audit sees content below the fold.
      await settle(page);
      await page.screenshot({ path: artifactPath(testInfo, `${scenario}-${name}.png`) });
      const viewport = page.viewportSize();
      const contentHeight = await page.evaluate(() => {
        const main = document.querySelector("main.content");
        return main.scrollHeight - main.clientHeight;
      });
      if (contentHeight > 0) {
        await page.setViewportSize({ width: viewport.width, height: viewport.height + contentHeight });
        await settle(page);
        await page.screenshot({ path: artifactPath(testInfo, `${scenario}-${name}-full.png`) });
        await page.setViewportSize(viewport);
      }
    }
    expect(problems).toEqual([]);
  });
}

test("command sheet closes on Escape and returns focus to the opener", async ({ page }, testInfo) => {
  const problems = await openSettings(page);
  await showSection(page, "commands");
  const opener = page.locator("#cmd-new");
  await opener.click();
  await expect(page.locator("#sheet-root [role=dialog]")).toBeVisible();
  await expect(page.locator("#cmd-label")).toBeFocused();
  await settle(page);
  await page.screenshot({ path: artifactPath(testInfo, "flow-command-sheet.png") });
  await page.keyboard.press("Escape");
  await expect(page.locator("#sheet-root")).toHaveCount(0);
  await expect(opener).toBeFocused();
  expect(problems).toEqual([]);
});

test("command sheet keeps Tab focus inside the dialog", async ({ page }) => {
  // Known issue (docs/audits/2026-09-23-native-feel): WebKit, like WKWebView
  // with macOS Full Keyboard Access off, skips buttons on Tab. The trap only
  // intercepts Tab on its first/last button, so focus falls to <body> after
  // the Advanced summary. Remove this line once the trap handles every Tab.
  test.fail(true, "focus escapes the sheet when buttons are not tabbable");
  await openSettings(page);
  await showSection(page, "commands");
  await page.locator("#cmd-new").click();
  await expect(page.locator("#cmd-label")).toBeFocused();
  const focusInsideSheet = () => page.evaluate(() => !!document.activeElement && !!document.activeElement.closest("#sheet-root"));
  for (const key of ["Tab", "Shift+Tab"]) {
    for (let i = 0; i < 12; i += 1) {
      await page.keyboard.press(key);
      expect(await focusInsideSheet(), `${key} #${i + 1} left the sheet`).toBe(true);
    }
  }
});

test("saving General round-trips through the bridge", async ({ page }) => {
  const problems = await openSettings(page);
  await showSection(page, "general");
  await page.locator("#language").fill("es");
  await page.locator("#save-general").click();
  await expect(page.locator("#save-status")).toHaveText("Saved");
  const saved = await page.evaluate(() =>
    window.__selaraMock.calls.filter((c) => c.cmd === "save_config_section" && c.args.section === "general").map((c) => c.args.value),
  );
  expect(saved.at(-1)).toMatchObject({ language: "es" });
  expect(await page.evaluate(() => window.__selaraMock.state.config.language)).toBe("es");
  expect(problems).toEqual([]);
});

test("update scenario offers install and shows release notes", async ({ page }, testInfo) => {
  const problems = await openSettings(page, "update");
  await expect(page.locator("#status-updates-state")).toHaveText("v0.7.0 available");
  await expect(page.locator("#status-install-update")).toBeVisible();
  await page.locator("#update-details-toggle").click();
  await expect(page.locator(".update-popover")).toBeVisible();
  await settle(page);
  await page.screenshot({ path: artifactPath(testInfo, "flow-update-popover.png") });
  expect(problems).toEqual([]);
});

test("errors scenario reports a failed save", async ({ page }, testInfo) => {
  const problems = await openSettings(page, "errors");
  await showSection(page, "general");
  await page.locator("#language").fill("fr");
  await page.locator("#save-general").click();
  await expect(page.locator("#save-status")).toContainText("Permission denied");
  await expect(page.locator("#save-dot")).toHaveClass(/bad/);
  await settle(page);
  await page.screenshot({ path: artifactPath(testInfo, "flow-save-error.png") });
  expect(problems).toEqual([]);
});

test("unreadable config disables editing", async ({ page }, testInfo) => {
  const problems = await openSettings(page, "errors", "&fail=config");
  await expect(page.locator("#section-status h1")).toHaveText("Settings unavailable");
  await expect(page.locator("#section-status .lead")).toContainText("invalid TOML");
  await settle(page);
  await page.screenshot({ path: artifactPath(testInfo, "flow-config-unavailable.png") });
  expect(problems).toEqual([]);
});

test("serve-changed events refresh the Status section", async ({ page }) => {
  const problems = await openSettings(page);
  await expect(page.locator("#status-serve")).toBeVisible();
  const before = await page.locator("#status-serve-state").textContent();
  await page.evaluate(() => {
    window.__selaraMock.state.serveRunning = false;
    window.__selaraMock.emit("serve-changed", null);
  });
  await expect(page.locator("#status-serve-state")).not.toHaveText(before);
  expect(problems).toEqual([]);
});

test("usage breakdown labels every provider", async ({ page }) => {
  const problems = await openSettings(page);
  const usage = await showSection(page, "usage");
  const providers = usage.locator(".usage-models .usage-provider");
  await expect(providers).toHaveText(["OpenAI-compatible", "Claude Code", "OpenRouter"]);
  expect(problems).toEqual([]);
});
