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

test("groups and buttons follow macOS metrics", async ({ page }) => {
  const problems = await openSettings(page);
  await showSection(page, "general");
  const groups = await page.locator("#section-general .panel").evaluateAll((els) => els.map((el) => {
    const cs = getComputedStyle(el);
    return { shadow: cs.boxShadow, blur: cs.webkitBackdropFilter || cs.backdropFilter || "none" };
  }));
  expect(groups.length).toBe(4);
  for (const group of groups) expect(group).toEqual({ shadow: "none", blur: "none" });

  await showSection(page, "commands");
  const button = page.locator("#cmd-new");
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

test("General saves each field when it is committed", async ({ page }) => {
  const problems = await openSettings(page);
  await showSection(page, "general");
  await expect(page.locator("#save-general")).toHaveCount(0);
  await page.locator("#language").fill("es");
  await page.locator("#language").press("Enter");
  await expect(page.locator("#save-status")).toHaveText("Saved");
  await expect(page.locator(".nav-feedback")).toHaveClass(/quiet/);
  const saved = await page.evaluate(() =>
    window.__selaraMock.calls.filter((c) => c.cmd === "save_config_section" && c.args.section === "general").map((c) => c.args.value),
  );
  expect(saved.at(-1)).toMatchObject({ language: "es" });
  expect(await page.evaluate(() => window.__selaraMock.state.config.language)).toBe("es");

  await showSection(page, "limits");
  await expect(page.locator("#save-limits")).toHaveCount(0);
  await page.locator("#secret_guard").click();
  await expect.poll(() => page.evaluate(() => window.__selaraMock.state.config.limits.secret_guard)).toBe(false);
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
  await page.locator("#language").press("Enter");
  await expect(page.locator("#section-general .field-error")).toContainText("Permission denied");
  await expect(page.locator("#language")).toHaveValue("fr");
  await expect(page.locator(".nav-feedback")).not.toHaveClass(/quiet/);
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

test("the footer keeps routine status quiet", async ({ page }) => {
  const problems = await openSettings(page);
  await expect(page.locator(".nav-feedback")).toHaveClass(/quiet/);
  await expect(page.locator("#status-updates")).toBeVisible();
  expect(problems).toEqual([]);
});

test("nothing changes on hover", async ({ page }) => {
  await openSettings(page);
  const hoverRules = await page.evaluate(() => {
    const found = [];
    const walk = (rules) => {
      for (const rule of rules) {
        if (rule.cssRules && !rule.selectorText) walk(rule.cssRules);
        else if (rule.selectorText && rule.selectorText.includes(":hover")) found.push(rule.selectorText);
      }
    };
    for (const sheet of document.styleSheets) walk(sheet.cssRules);
    return found;
  });
  expect(hoverRules).toEqual([]);
});

test("History copies through a button and a native menu", async ({ page }) => {
  const problems = await openSettings(page);
  await page.evaluate(() => {
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: async (text) => { window.__copied = text; } } });
  });
  const history = await showSection(page, "history");
  const first = history.locator(".hist-item").first();
  await expect(first.locator('[data-act="copy-original"]')).toHaveCount(0);
  await first.locator('[data-act="copy-result"]').click();
  await expect.poll(() => page.evaluate(() => window.__copied)).toBe("Thanks for your help with the launch; it's been great.");

  await first.locator('[data-act="copy-menu"]').click();
  await expect.poll(() => page.evaluate(() => window.__selaraMock.menu)).toEqual([
    { text: "Copy Result", enabled: true },
    { text: "Copy Original", enabled: true },
  ]);
  await page.evaluate(() => window.__selaraMock.chooseMenuItem("Copy Original"));
  await expect.poll(() => page.evaluate(() => window.__copied)).toBe("Thanks for you're help with the launch, its been great.");

  await history.locator(".hist-item").nth(1).click({ button: "right" });
  await expect.poll(() => page.evaluate(() => window.__selaraMock.menu && window.__selaraMock.menu.length)).toBe(2);
  await expect(history.locator(".hist-item").nth(1).locator(".badge")).toHaveClass(/warn/);
  expect(problems).toEqual([]);
});

test("command rows offer a native context menu and native delete confirmation", async ({ page }) => {
  const problems = await openSettings(page);
  const commands = await showSection(page, "commands");
  const row = commands.locator('.cmd-item[data-id="friendly"]');
  await expect(row.locator('[data-act="del"]')).toBeHidden();
  await row.click({ button: "right" });
  await expect.poll(() => page.evaluate(() => window.__selaraMock.menu)).toEqual([
    { text: "Edit…", enabled: true },
    { text: "Duplicate", enabled: true },
    "-",
    { text: "Delete…", enabled: true },
  ]);
  await page.evaluate(() => { window.__selaraMock.state.confirm = false; return window.__selaraMock.chooseMenuItem("Delete…"); });
  await expect.poll(() => page.evaluate(() => window.__selaraMock.calls.filter((c) => c.cmd === "confirm_action").length)).toBe(1);
  expect(await page.evaluate(() => window.__selaraMock.state.config.commands.length)).toBe(6);

  await row.click({ button: "right" });
  await page.evaluate(() => { window.__selaraMock.state.confirm = true; return window.__selaraMock.chooseMenuItem("Delete…"); });
  await expect.poll(() => page.evaluate(() => window.__selaraMock.state.config.commands.length)).toBe(5);
  const ask = await page.evaluate(() => window.__selaraMock.calls.filter((c) => c.cmd === "confirm_action").at(-1).args);
  expect(ask).toMatchObject({ confirmLabel: "Delete Command" });
  await expect(commands.locator('.cmd-item[data-id="friendly"]')).toHaveCount(0);
  expect(problems).toEqual([]);
});

test("Providers keeps its list beside the connection at every size", async ({ page }) => {
  const problems = await openSettings(page);
  await showSection(page, "models");
  const list = await page.locator(".provider-list").boundingBox();
  const detail = await page.locator(".provider-detail").boundingBox();
  expect(detail.x).toBeGreaterThanOrEqual(list.x + list.width - 1);
  expect(Math.abs(detail.y - list.y)).toBeLessThan(2);
  const appearance = await page.locator("#provider-preset").evaluate((el) => getComputedStyle(el).appearance || getComputedStyle(el).webkitAppearance);
  expect(appearance).not.toBe("none");
  expect(await page.locator(".provider-toggle").first().evaluate((el) => getComputedStyle(el).cursor)).toBe("default");
  expect(problems).toEqual([]);
});

test("the page title stays in the title bar while content scrolls", async ({ page }) => {
  const problems = await openSettings(page);
  const title = page.locator("#section-status > h1");
  await expect(title).toHaveAttribute("data-tauri-drag-region", "");
  await page.locator("main.content").evaluate((el) => { el.scrollTop = 300; });
  await expect(page.locator("main.content")).toHaveClass(/scrolled/);
  const box = await title.boundingBox();
  expect(Math.round(box.y)).toBe(0);
  expect(await title.evaluate((el) => getComputedStyle(el).boxShadow)).not.toBe("none");
  expect(problems).toEqual([]);
});

test("Status explains serve without developer commands up front", async ({ page }) => {
  const problems = await openSettings(page, "fresh");
  const serve = page.locator("#status-serve");
  await expect(serve.locator("> .field-hint").first()).toHaveText("Hotkeys only work while it runs.");
  await expect(serve.locator("details summary", { hasText: "Troubleshooting" })).toBeVisible();
  await expect(page.locator("#status-shortcuts .status-dot")).toHaveCount(0);
  await showSection(page, "usage");
  await expect(page.locator("#status-usage .status-dot")).toHaveCount(0);
  expect(problems).toEqual([]);
});

test("a destructive action asks once however often it is clicked", async ({ page }) => {
  const problems = await openSettings(page);
  await page.evaluate(() => { window.__selaraMock.state.confirm = false; });
  const history = await showSection(page, "history");
  await history.locator("#history-clear").evaluate((el) => { el.click(); el.click(); el.click(); });
  await expect.poll(() => page.evaluate(() => window.__selaraMock.calls.filter((c) => c.cmd === "confirm_action").length)).toBe(1);
  expect(await page.evaluate(() => window.__selaraMock.state.history.length)).toBe(4);
  expect(problems).toEqual([]);
});

test("Limits rejects a blank number instead of saving unlimited", async ({ page }) => {
  const problems = await openSettings(page);
  await showSection(page, "limits");
  const before = await page.evaluate(() => window.__selaraMock.calls.filter((c) => c.cmd === "save_config_section").length);
  await page.locator("#soft_warn").fill("");
  await page.locator("#soft_warn").press("Enter");
  await expect(page.locator("#section-limits .field-error")).toContainText("0 means no limit");
  expect(await page.evaluate(() => window.__selaraMock.calls.filter((c) => c.cmd === "save_config_section").length)).toBe(before);
  expect(await page.evaluate(() => window.__selaraMock.state.config.limits.soft_warn_chars)).toBe(8000);
  await page.locator("#soft_warn").fill("6000");
  await page.locator("#soft_warn").press("Enter");
  await expect.poll(() => page.evaluate(() => window.__selaraMock.state.config.limits.soft_warn_chars)).toBe(6000);
  await expect(page.locator("#section-limits .field-error")).toHaveCount(0);
  expect(problems).toEqual([]);
});
