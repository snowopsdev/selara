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
  // WebKit, like WKWebView with macOS keyboard navigation off, leaves buttons
  // out of the native Tab order; the trap must still keep focus in the sheet.
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

test("sidebar behaves like a source list", async ({ page }) => {
  const problems = await openSettings(page);
  const current = page.locator(".nav-item[aria-current=page]");
  await expect(current).toHaveAttribute("data-section", "status");
  await expect(page.locator('.nav-item[tabindex="0"]')).toHaveCount(1);
  await current.focus();
  await page.keyboard.press("ArrowDown");
  await expect(page.locator("#section-general")).toBeVisible();
  await expect(page.locator('.nav-item[data-section="general"]')).toBeFocused();
  await expect(page.locator(".nav-item[aria-current=page]")).toHaveAttribute("data-section", "general");
  await page.keyboard.press("End");
  await expect(page.locator("#section-limits")).toBeVisible();
  await page.keyboard.press("Home");
  await expect(page.locator("#section-status")).toBeVisible();
  await page.locator("main.content").evaluate((el) => { el.scrollTop = 400; });
  await page.keyboard.press("ArrowDown");
  await expect(page.locator("#section-general h1")).toBeInViewport();
  expect(await page.locator("main.content").evaluate((el) => el.scrollTop)).toBe(0);
  const selection = await page.locator(".nav-item.active").evaluate((el) => getComputedStyle(el).boxShadow);
  expect(selection).toBe("none");
  expect(problems).toEqual([]);
});

test("controls use the system accent color", async ({ page }) => {
  const problems = await openSettings(page, "configured", "&accent=%23a550a7");
  await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--accent").trim())).toBe("#a550a7");
  await showSection(page, "commands");
  expect(await page.locator("#cmd-new").evaluate((el) => getComputedStyle(el).backgroundColor)).toBe("rgb(165, 80, 167)");
  await showSection(page, "limits");
  expect(await page.locator("#secret_guard").evaluate((el) => getComputedStyle(el).accentColor)).toBe("rgb(165, 80, 167)");
  expect(problems).toEqual([]);
});

for (const [accent, label] of [["#a550a7", "rgb(255, 255, 255)"], ["#007aff", "rgb(255, 255, 255)"], ["#ffc600", "rgb(29, 29, 31)"], ["#f7821b", "rgb(29, 29, 31)"]]) {
  test(`accent ${accent} gets a readable button label`, async ({ page }) => {
    const problems = await openSettings(page, "configured", `&accent=${encodeURIComponent(accent)}`);
    await expect.poll(() => page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--accent").trim())).toBe(accent);
    await showSection(page, "commands");
    expect(await page.locator("#cmd-new").evaluate((el) => getComputedStyle(el).color)).toBe(label);
    expect(problems).toEqual([]);
  });
}

test("groups and buttons follow macOS metrics", async ({ page }) => {
  const problems = await openSettings(page);
  await showSection(page, "general");
  const groups = await page.locator("#section-general .panel").evaluateAll((els) => els.map((el) => {
    const cs = getComputedStyle(el);
    return { shadow: cs.boxShadow, blur: cs.webkitBackdropFilter || cs.backdropFilter || "none" };
  }));
  expect(groups.length).toBe(4);
  for (const group of groups) expect(group).toEqual({ shadow: "none", blur: "none" });

  const button = page.locator("#save-general");
  const metrics = await button.evaluate((el) => {
    const cs = getComputedStyle(el);
    return { height: el.getBoundingClientRect().height, radius: parseFloat(cs.borderTopLeftRadius) };
  });
  expect(metrics.height).toBe(24);
  expect(metrics.radius).toBeLessThan(metrics.height / 2);
  await button.hover();
  await page.mouse.down();
  expect(await button.evaluate((el) => getComputedStyle(el).transform)).toBe("none");
  await page.mouse.up();
  expect(problems).toEqual([]);
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
