import { test, expect } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { openSettings, showSection, artifactPath, settle } from "./helpers.mjs";

// Providers (5A), History (6A), Usage and the Writing Receipt (7A, 7B),
// General and Limits (8A).
const mockState = (page, fn) => page.evaluate(fn);
const savedConfig = (page) => page.evaluate(() => window.__selaraMock.state.config);

test.describe("Providers", () => {
  test("each connection has one status line and only the active one is In use", async ({ page }) => {
    const problems = await openSettings(page);
    const models = await showSection(page, "models");
    await expect(models.locator(".provider-list-label")).toHaveCount(0);
    await expect(models.locator(".provider-group-label")).toHaveText(["Accounts", "Installed CLIs", "API keys"]);
    const rows = models.locator(".provider-list .provider-choice");
    await expect(rows).toHaveCount(7);
    for (let i = 0; i < 7; i += 1) {
      const row = rows.nth(i);
      await expect(row.locator(".provider-line")).toHaveCount(1);
      const fits = await row.evaluate((el) => {
        const name = el.querySelector(".provider-name");
        return { singleLine: name.getBoundingClientRect().height < 22, untruncated: name.scrollWidth <= name.clientWidth + 1 };
      });
      expect(fits).toEqual({ singleLine: true, untruncated: true });
    }
    await expect(rows.nth(0).locator(".provider-line")).toHaveText("Ready · pro");
    await expect(rows.nth(1).locator(".provider-line")).toHaveText("Ready · 2.1.4");
    await expect(rows.nth(2).locator(".provider-line")).toHaveText("Not installed");
    await expect(rows.nth(5).locator(".provider-line")).toHaveText("Set up…");
    await expect(models.locator(".provider-list .provider-in-use:visible")).toHaveCount(1);
    await expect(models.locator('[data-provider-choice="openai"] .provider-in-use')).toBeVisible();
    await expect(page.locator('[role="switch"][data-provider-enabled]')).toHaveCount(1);
    await expect(page.locator("#save-models")).toHaveText("In Use ✓");

    await models.locator('[data-provider-choice="claude"]').click();
    await expect(page.locator("#provider-enabled-toggle")).toHaveAttribute("data-provider-enabled", "claude");
    await expect(page.locator("#save-models")).toHaveText("Use for Commands");
    await page.locator("#save-models").click();
    await expect.poll(async () => (await savedConfig(page)).provider.kind).toBe("claude_cli");
    await expect(models.locator('[data-provider-choice="claude"] .provider-in-use')).toBeVisible();
    await expect(models.locator(".provider-list .provider-in-use:visible")).toHaveCount(1);
    await expect(page.locator("#save-models")).toHaveText("In Use ✓");
    expect(problems).toEqual([]);
  });

  test("the health strip shows timing when recorded and a designed state when not", async ({ page }) => {
    const problems = await openSettings(page);
    const models = await showSection(page, "models");
    const strip = page.locator("#provider-health");
    await expect(strip).toHaveAttribute("data-state", "timed");
    await expect(page.locator("#provider-health-last .v")).toHaveText("2 min ago");
    await expect(page.locator("#provider-health-median .v")).toHaveText("1.1 s");
    await expect(page.locator("#provider-health-median .health-spark i")).toHaveCount(20);
    await expect(page.locator("#provider-health-month .v")).toHaveText("236 req");
    await expect(page.locator("#provider-health-month .s")).toHaveText("~$0.15");
    await expect(page.locator("#provider-runtime-version")).toHaveText("key in Keychain");
    await expect(page.locator("#key-stored")).toBeVisible();

    await models.locator('[data-provider-choice="claude"]').click();
    await expect(page.locator("#provider-health-median .v")).toHaveText("4.2 s");

    await models.locator('[data-provider-choice="openrouter"]').click();
    await expect(strip).toHaveAttribute("data-state", "untimed");
    await expect(page.locator("#provider-health-median .v")).toHaveText("No timing yet");
    await expect(page.locator("#provider-health-median .health-spark.empty")).toHaveCount(1);
    await expect(page.locator("#provider-health-month .v")).toHaveText("10 req");

    await models.locator('[data-provider-choice="anthropic"]').click();
    await expect(strip).toHaveAttribute("data-state", "idle");
    await expect(page.locator("#provider-health-last .v")).toHaveText("Never");
    await settle(page);
    await page.screenshot({ path: artifactPath(test.info(), "flow-provider-untimed.png") });
    expect(problems).toEqual([]);
  });

  test("Test runs the reachability check for the selected connection", async ({ page }) => {
    const problems = await openSettings(page);
    await showSection(page, "models");
    await page.locator("#provider-test").click();
    await expect(page.locator("#provider-test-result")).toHaveText("✓ Reachable · 3 models available.");
    await page.locator('[data-provider-choice="cursor"]').click();
    await page.locator("#provider-test").click();
    await expect(page.locator("#provider-test-result")).toContainText("not found");
    expect(problems).toEqual([]);
  });

  test("fresh connections show No timing yet", async ({ page }) => {
    const problems = await openSettings(page, "fresh");
    await showSection(page, "models");
    await expect(page.locator("#provider-health-median .v")).toHaveText("No timing yet");
    await expect(page.locator("#provider-health-last .v")).toHaveText("Never");
    expect(problems).toEqual([]);
  });
});

test.describe("History", () => {
  test("entries show an inline word diff grouped by day", async ({ page }) => {
    const problems = await openSettings(page);
    const history = await showSection(page, "history");
    await expect(history.locator(".hx-day")).toHaveText(["Today", "Yesterday", /day$/]);
    const first = history.locator(".hist-item").first();
    await expect(first.locator(".hx-text del")).toHaveText(["you're", ", its"]);
    await expect(first.locator(".hx-text ins")).toHaveText(["your", "; it's"]);
    await expect(first.locator(".badge")).toHaveCount(0);
    // The tile is the command's configured glyph (Proofread has "✓" in the mock).
    await expect(first.locator(".hx-ic")).toHaveText("✓");

    const concise = history.locator(".hist-item").nth(1);
    await expect(concise.locator(".badge.warn")).toHaveText("Not applied: still in Slack as before");
    await expect(concise.locator('[data-act="copy-result"]')).toHaveText("Copy result");
    await expect(concise.locator('[data-act="copy-menu"]')).toHaveCount(1);
    await expect(history.locator(".hist-item").nth(2).locator(".badge.warn")).toHaveText("Check Notes: paste may have worked");

    const translate = history.locator(".hist-item").nth(3);
    await expect(translate.locator(".hx-text.result-only")).toHaveText("Thank you very much for your patience.");
    await expect(translate.locator("del, ins")).toHaveCount(0);
    await translate.locator(".hx-orig summary").click();
    await expect(translate.locator(".hx-orig p")).toHaveText("Merci beaucoup pour votre patience.");
    expect(problems).toEqual([]);
  });

  test("problem entries reach Copy Original from the keyboard", async ({ page }) => {
    const problems = await openSettings(page);
    await page.evaluate(() => {
      Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: async (text) => { window.__copied = text; } } });
    });
    const history = await showSection(page, "history");
    const concise = history.locator(".hist-item").nth(1);
    await expect(concise).toHaveClass(/problem/);
    const original = await page.evaluate(() => window.__selaraMock.state.history[1].original);
    const arrow = concise.locator('[data-act="copy-menu"]');
    await expect(arrow).toHaveAttribute("aria-haspopup", "menu");
    await expect(arrow).toHaveAttribute("aria-label", "More copy options");
    await arrow.focus();
    await page.keyboard.press("Enter");
    await expect.poll(() => page.evaluate(() => window.__selaraMock.menu)).toEqual([
      { text: "Copy Result", enabled: true },
      { text: "Copy Original", enabled: true },
    ]);
    await page.evaluate(() => window.__selaraMock.chooseMenuItem("Copy Original"));
    await expect.poll(() => page.evaluate(() => window.__copied)).toBe(original);
    expect(problems).toEqual([]);
  });

  test("filter chips and search narrow the list", async ({ page }) => {
    const problems = await openSettings(page);
    const history = await showSection(page, "history");
    await expect(history.locator(".hx-chip")).toHaveText(["All4", "Needs attention2", "Mail", "Slack", "Notes", "Safari"]);
    await history.locator('.hx-chip[data-filter="attention"]').click();
    await expect(history.locator('.hx-chip[data-filter="attention"]')).toHaveAttribute("aria-pressed", "true");
    await expect(history.locator(".hist-item")).toHaveCount(2);
    await history.locator('.hx-chip[data-filter="app:Slack"]').click();
    await expect(history.locator(".hist-item .hist-name")).toHaveText(["Concise"]);
    await history.locator('.hx-chip[data-filter="all"]').click();
    await page.locator("#history-search").fill("numbers");
    await expect(history.locator(".hist-item .hist-name")).toHaveText(["Professional"]);
    await page.locator("#history-search").fill("nothing like this");
    await expect(history.locator("#history-list")).toContainText("No matching rewrites");
    expect(problems).toEqual([]);
  });

  test("an empty history looks designed", async ({ page }) => {
    const problems = await openSettings(page, "fresh");
    const history = await showSection(page, "history");
    await expect(history.locator(".hx-empty strong")).toHaveText("No rewrites yet");
    await expect(history.locator(".hx-chip")).toHaveCount(0);
    await expect(page.locator("#history-clear")).toBeDisabled();
    expect(problems).toEqual([]);
  });
});

test.describe("Usage", () => {
  test("tiles, 30 daily bars, and the provider share", async ({ page }) => {
    const problems = await openSettings(page);
    const usage = await showSection(page, "usage");
    await expect(usage.locator(".ux-tile .v")).toHaveText(["12", "284", "1,031"]);
    await expect(usage.locator(".ux-tile .s")).toHaveText(["requests · < $0.01", "requests · ~$0.15", "~$0.54 priced · 170 free/local"]);
    const bars = usage.locator("#usage-bars > i");
    await expect(bars).toHaveCount(30);
    await expect(bars.last()).toHaveClass(/today/);
    await expect(bars.last()).toHaveAttribute("title", "Today · 12 requests");
    const sum = await bars.evaluateAll((els) => els.reduce((n, el) => n + Number(el.getAttribute("title").match(/· (\d+)/)[1]), 0));
    expect(sum).toBe(284);
    await expect(usage.locator("#usage-legend li")).toHaveText(["gpt-5.4-mini · 84%", "Claude Code · default model · 12%", "claude-opus-5 via OpenRouter · 4%"]);
    await expect(page.locator("#usage-details")).not.toHaveAttribute("open", "");
    await page.locator("#usage-details > summary").click();
    await expect(usage.locator(".usage-models")).toBeVisible();
    expect(problems).toEqual([]);
  });

  test("bars grow in on the first view only", async ({ page }) => {
    await openSettings(page);
    await showSection(page, "usage");
    await expect(page.locator("#usage-bars")).toHaveClass(/grow/);
    await page.locator("#usage-refresh").click();
    await expect(page.locator("#usage-refresh")).toHaveText("Refresh");
    await expect(page.locator("#usage-bars")).not.toHaveClass(/grow/);
  });

  test("the receipt shares a 1080×1350 PNG through Copy and Save", async ({ page }, testInfo) => {
    const problems = await openSettings(page);
    const usage = await showSection(page, "usage");
    const receipt = usage.locator("#usage-receipt");
    await expect(receipt).toContainText("Rewrites1,031");
    await expect(receipt).toContainText("Words polished*≈ 278,160");
    await expect(receipt).toContainText("Time saved*≈ 17 h");
    await expect(receipt).toContainText("Favourite modelgpt-5.4-mini");
    await expect(receipt).toContainText("COST / REWRITE$0.0005");
    const share = page.locator("#usage-share");
    await share.click();
    const sheet = page.locator("#receipt-sheet-root [role=dialog]");
    await expect(sheet).toBeVisible();
    await expect(page.locator("#receipt-save")).toBeEnabled();
    const size = await page.locator("#receipt-png").evaluate(async (img) => { await img.decode(); return [img.naturalWidth, img.naturalHeight]; });
    expect(size).toEqual([1080, 1350]);
    // Aggregates only: the image is drawn from totals, never history text.
    const src = await page.locator("#receipt-png").getAttribute("src");
    expect(src.startsWith("data:image/png;base64,")).toBe(true);
    const file = artifactPath(testInfo, "receipt.png");
    mkdirSync(file.replace(/\/[^/]+$/, ""), { recursive: true });
    writeFileSync(file, Buffer.from(src.split(",")[1], "base64"));
    await page.locator("#receipt-copy").click();
    await expect(page.locator("#receipt-sheet-status")).toHaveText("Copied. Paste it anywhere.");
    await page.locator("#receipt-save").click();
    await expect(page.locator("#receipt-sheet-status")).toContainText("Saved to ~/Downloads/selara-receipt-");
    const mock = await mockState(page, () => ({ copies: window.__selaraMock.state.pngCopies, saves: window.__selaraMock.state.pngSaves }));
    expect(mock.copies.length).toBe(1);
    expect(mock.copies[0]).toBeGreaterThan(1000);
    expect(mock.saves[0].suggested_name).toMatch(/^selara-receipt-\d{4}-\d{2}-\d{2}\.png$/);
    await settle(page);
    await page.screenshot({ path: artifactPath(testInfo, "flow-receipt-share.png") });
    await page.keyboard.press("Escape");
    await expect(page.locator("#receipt-sheet-root")).toHaveCount(0);
    await expect(share).toBeFocused();
    expect(problems).toEqual([]);
  });

  test("the receipt share sheet is a modal overlay inside the window", async ({ page }) => {
    const problems = await openSettings(page);
    await showSection(page, "usage");
    await page.locator("#usage-share").click();
    const backdrop = page.locator("#receipt-sheet-root .sheet-backdrop");
    await expect(backdrop).toBeVisible();
    expect(await backdrop.evaluate((el) => getComputedStyle(el).position)).toBe("fixed");
    const box = await page.locator("#receipt-sheet-root .sheet").boundingBox();
    const view = page.viewportSize();
    expect(box.y).toBeGreaterThanOrEqual(0);
    expect(box.y + box.height).toBeLessThanOrEqual(view.height);
    await expect(page.locator("#receipt-save")).toBeInViewport();
    await page.locator("#receipt-done").click();
    await expect(page.locator("#receipt-sheet-root")).toHaveCount(0);
    expect(problems).toEqual([]);
  });

  test("a fresh receipt renders zeros and cannot be shared yet", async ({ page }) => {
    const problems = await openSettings(page, "fresh");
    const usage = await showSection(page, "usage");
    await expect(usage.locator("#usage-receipt")).toContainText("Your first rewrite will show up here.");
    await expect(usage.locator("#usage-receipt")).toContainText("Rewrites0");
    await expect(page.locator("#usage-share")).toBeDisabled();
    await expect(usage.locator("#usage-bars > i")).toHaveCount(30);
    await expect(usage.locator(".ux-empty-note")).toBeVisible();
    expect(problems).toEqual([]);
  });
});

test.describe("General", () => {
  test("the hotkey recorder records, rejects conflicts by name, and saves", async ({ page }) => {
    const problems = await openSettings(page);
    await showSection(page, "general");
    const recorder = page.locator("#hotkey");
    await expect(recorder.locator(".kc")).toHaveText(["⌃", "⇧", "Space"]);
    await recorder.click();
    await expect(recorder).toHaveAttribute("data-state", "recording");
    await expect(page.locator("#hotkey-hint")).toContainText("Recording");
    await page.keyboard.press("Control+Alt+P");
    await expect(recorder).toHaveAttribute("data-state", "conflict");
    await expect(page.locator("#hotkey-hint")).toHaveText("⌃⌥P is already used by Proofread. Try another.");
    await page.keyboard.press("Control+Alt+Space");
    await expect(recorder).toHaveAttribute("data-state", "saved");
    await expect(page.locator("#hotkey-hint")).toHaveText("Saved. Works within about a second.");
    await expect(recorder.locator(".kc")).toHaveText(["⌃", "⌥", "Space"]);
    expect((await savedConfig(page)).hotkey).toBe("ctrl+alt+space");
    const leases = await page.evaluate(() => window.__selaraMock.calls.filter((c) => c.cmd === "set_shortcut_recording").map((c) => c.args.active));
    expect(leases[0]).toBe(true);
    expect(leases.at(-1)).toBe(false);

    await recorder.click();
    await expect(recorder).toHaveAttribute("data-state", "recording");
    await page.keyboard.press("Escape");
    await expect(recorder).toHaveAttribute("data-state", "idle");
    expect((await savedConfig(page)).hotkey).toBe("ctrl+alt+space");
    expect(problems).toEqual([]);
  });

  test("the hotkey recorder stops when focus leaves it and never records another field's keys", async ({ page }) => {
    const problems = await openSettings(page);
    await showSection(page, "general");
    const recorder = page.locator("#hotkey");
    const leases = () => page.evaluate(() => window.__selaraMock.calls.filter((c) => c.cmd === "set_shortcut_recording").map((c) => c.args.active));
    const before = (await savedConfig(page)).hotkey;
    await recorder.click();
    await expect(page.locator("#hotkey-hint")).toContainText("Recording");
    const picker = page.locator("#language-picker");
    await picker.click();
    await expect(recorder).toHaveAttribute("data-state", "idle");
    await page.keyboard.press("Meta+A");
    await expect(picker).toBeFocused();
    await expect.poll(async () => (await leases()).at(-1)).toBe(false);
    const settled = (await leases()).length;
    await page.waitForTimeout(1000);
    expect((await leases()).length, "no lease renewals after recording stops").toBe(settled);
    expect((await savedConfig(page)).hotkey).toBe(before);
    await expect(recorder.locator(".kc")).toHaveText(["⌃", "⇧", "Space"]);
    await picker.press("Escape");

    // Shift-Tab moves focus on instead of being swallowed.
    await recorder.click();
    await expect(page.locator("#hotkey-hint")).toContainText("Recording");
    await page.keyboard.press("Shift+Tab");
    await expect(recorder).toHaveAttribute("data-state", "idle");
    await expect(recorder).not.toBeFocused();

    // Leaving the section releases the lease too.
    await recorder.click();
    await expect(page.locator("#hotkey-hint")).toContainText("Recording");
    await showSection(page, "limits");
    await expect.poll(async () => (await leases()).at(-1)).toBe(false);
    const left = (await leases()).length;
    await page.waitForTimeout(1000);
    expect((await leases()).length).toBe(left);
    expect((await savedConfig(page)).hotkey).toBe(before);
    expect(problems).toEqual([]);
  });

  test("leaving the language field keeps a typed custom language", async ({ page }) => {
    const problems = await openSettings(page);
    await showSection(page, "general");
    const picker = page.locator("#language-picker");
    await picker.click();
    await picker.fill("Klingon");
    await picker.press("Tab");
    await expect.poll(async () => (await savedConfig(page)).language).toBe("Klingon");
    await expect(picker).toHaveValue("Klingon");

    await picker.click();
    await picker.fill("Fren");
    await picker.press("Tab");
    await expect(picker).toHaveValue("Klingon");
    expect((await savedConfig(page)).language, "half of a listed name reverts").toBe("Klingon");

    await picker.click();
    await picker.fill("French");
    await picker.press("Tab");
    await expect.poll(async () => (await savedConfig(page)).language).toBe("fr");
    await expect(picker).toHaveValue("Français");
    expect(problems).toEqual([]);
  });

  test("the language combo box filters native names and saves the code", async ({ page }) => {
    const problems = await openSettings(page);
    await showSection(page, "general");
    const picker = page.locator("#language-picker");
    await expect(picker).toHaveValue("English");
    await picker.click();
    await expect(page.locator("#language-list")).toBeVisible();
    await picker.fill("日本");
    await expect(page.locator("#language-list [role=option]").first()).toHaveText("日本語ja");
    await picker.press("Enter");
    await expect.poll(async () => (await savedConfig(page)).language).toBe("ja");
    await expect(picker).toHaveValue("日本語");
    await expect(page.locator("#language-code")).toHaveText("ja");
    await picker.fill("Klingon");
    await expect(page.locator("#language-list [role=option]").last()).toContainText("Use “Klingon”");
    await picker.press("ArrowUp");
    await picker.press("Enter");
    await expect.poll(async () => (await savedConfig(page)).language).toBe("Klingon");
    expect(problems).toEqual([]);
  });

  test("excluded apps are chips with icons, added from a native menu and removed with ✕", async ({ page }) => {
    const problems = await openSettings(page);
    const general = await showSection(page, "general");
    const chips = general.locator("#excluded-apps .app-chip");
    // The configured fixture excludes one app (the 1Password bundle id).
    const initial = (await savedConfig(page)).excluded_apps;
    expect(initial).toHaveLength(1);
    await expect(chips).toHaveCount(1);
    await expect(chips.first().locator(".app-name")).toHaveText("1Password");
    await expect(chips.first().locator(".app-icon img")).toHaveCount(1);

    await page.locator("#excluded-add").click();
    await expect.poll(() => page.evaluate(() => (window.__selaraMock.menu || []).map((i) => (i === "-" ? "-" : i.text)))).toEqual(
      ["Finder", "Mail", "Messages", "Notes", "Safari", "Slack", "Terminal", "Visual Studio Code", "-", "Choose from Applications…", "Enter Bundle ID or Pattern…"]);
    await page.evaluate(() => window.__selaraMock.chooseMenuItem("Slack"));
    await expect(chips).toHaveCount(2);
    await expect.poll(async () => (await savedConfig(page)).excluded_apps).toEqual([...initial, "com.tinyspeck.slackmacgap"]);

    await page.locator("#excluded-add").click();
    await expect.poll(() => page.evaluate(() => (window.__selaraMock.menu || []).length)).toBeGreaterThan(2);
    await page.evaluate(() => window.__selaraMock.chooseMenuItem("Choose from Applications…"));
    await expect(chips.nth(2).locator(".app-name")).toHaveText("Keychain Access");

    await page.locator("#excluded-add").click();
    await expect.poll(() => page.evaluate(() => (window.__selaraMock.menu || []).length)).toBeGreaterThan(2);
    await page.evaluate(() => window.__selaraMock.chooseMenuItem("Enter Bundle ID or Pattern…"));
    await page.locator("#excluded-entry").fill("com.apple.*");
    await page.locator("#excluded-entry").press("Enter");
    await expect(chips.nth(3).locator(".app-icon")).toHaveText("✱");
    await expect.poll(async () => (await savedConfig(page)).excluded_apps.length).toBe(4);

    await chips.first().locator("button[data-remove]").click();
    await expect.poll(async () => (await savedConfig(page)).excluded_apps).toEqual(["com.tinyspeck.slackmacgap", "com.apple.keychainaccess", "com.apple.*"]);
    await settle(page);
    await page.screenshot({ path: artifactPath(test.info(), "flow-excluded-apps.png") });
    expect(problems).toEqual([]);
  });
});

test.describe("Limits", () => {
  test("ruler markers drag without crossing and save on release", async ({ page }) => {
    const problems = await openSettings(page);
    await showSection(page, "limits");
    await expect(page.locator(".ruler-marker")).toHaveCount(3);
    await expect(page.locator("#ruler-marker-2")).toHaveAttribute("aria-valuetext", "8,000 characters, about 3 pages");
    const ruler = await page.locator("#limits-ruler").boundingBox();
    const soft = await page.locator("#ruler-marker-2").boundingBox();
    await page.mouse.move(soft.x + soft.width / 2, soft.y + soft.height / 2);
    await page.mouse.down();
    await page.mouse.move(ruler.x + ruler.width * 0.8, soft.y + soft.height / 2, { steps: 8 });
    await page.mouse.move(ruler.x + ruler.width - 2, soft.y + soft.height / 2, { steps: 4 });
    await page.mouse.up();
    await expect.poll(async () => (await savedConfig(page)).limits.soft_warn_chars).toBe(100000);
    expect((await savedConfig(page)).limits.hard_max_chars).toBe(100000);
    await expect(page.locator("#soft_warn")).toHaveValue("100000");

    const replace = await page.locator("#ruler-marker-1").boundingBox();
    await page.mouse.move(replace.x + replace.width / 2, replace.y + replace.height / 2);
    await page.mouse.down();
    await page.mouse.move(ruler.x - 40, replace.y + replace.height / 2, { steps: 6 });
    await page.mouse.up();
    await expect.poll(async () => (await savedConfig(page)).limits.replace_warn_chars).toBe(500);
    expect(problems).toEqual([]);
  });

  test("ruler edges: off-scale values survive clicks and wrong-way keys, and Page keys step", async ({ page }) => {
    const problems = await openSettings(page);
    await showSection(page, "limits");
    const limits = async () => (await savedConfig(page)).limits;
    await page.locator("#replace_warn").fill("200");
    await page.locator("#replace_warn").press("Tab");
    await expect.poll(async () => (await limits()).replace_warn_chars).toBe(200);
    const replace = page.locator("#ruler-marker-1");
    await replace.focus();
    for (const key of ["ArrowLeft", "Home", "PageDown", "Shift+ArrowLeft"]) await page.keyboard.press(key);
    await page.waitForTimeout(600);
    expect((await limits()).replace_warn_chars, "below 500, ← and Home never raise it").toBe(200);
    await expect(page.locator("#replace_warn")).toHaveValue("200");

    await page.locator("#hard_max").fill("250000");
    await page.locator("#hard_max").press("Tab");
    await expect.poll(async () => (await limits()).hard_max_chars).toBe(250000);
    const box = await page.locator("#ruler-marker-3").boundingBox();
    const x = box.x + box.width / 2;
    const y = box.y + box.height / 2;
    await page.mouse.move(x, y);
    await page.mouse.down();
    await page.mouse.move(x - 1, y);
    await page.mouse.move(x - 2, y + 1);
    await page.mouse.up();
    await page.waitForTimeout(300);
    expect((await limits()).hard_max_chars, "a click with jitter is not a drag").toBe(250000);
    await expect(page.locator("#hard_max")).toHaveValue("250000");

    const soft = page.locator("#ruler-marker-2");
    await soft.focus();
    await page.keyboard.press("PageUp");
    await expect.poll(async () => (await limits()).soft_warn_chars).toBeGreaterThan(8000);
    const raised = (await limits()).soft_warn_chars;
    await page.keyboard.press("ArrowRight");
    await expect.poll(async () => (await limits()).soft_warn_chars).toBeGreaterThan(raised);
    const small = (await limits()).soft_warn_chars - raised;
    expect(raised - 8000, "a page step is bigger than an arrow step").toBeGreaterThan(small);
    await page.keyboard.press("PageDown");
    await expect.poll(async () => (await limits()).soft_warn_chars).toBeLessThan(raised + small);
    expect(problems).toEqual([]);
  });

  test("ruler markers work from the keyboard, including no limit", async ({ page }) => {
    const problems = await openSettings(page);
    await showSection(page, "limits");
    const hard = page.locator("#ruler-marker-3");
    await hard.focus();
    await page.keyboard.press("End");
    await expect.poll(async () => (await savedConfig(page)).limits.hard_max_chars).toBe(0);
    await expect(hard).toHaveAttribute("aria-valuetext", "No limit");
    await page.keyboard.press("ArrowLeft");
    await expect.poll(async () => (await savedConfig(page)).limits.hard_max_chars).toBe(200000);

    const replace = page.locator("#ruler-marker-1");
    await replace.focus();
    await page.keyboard.press("ArrowRight");
    await expect.poll(async () => (await savedConfig(page)).limits.replace_warn_chars).toBeGreaterThan(4000);
    await page.keyboard.press("End");
    await expect.poll(async () => (await savedConfig(page)).limits.replace_warn_chars).toBe(8000);
    await page.keyboard.press("Shift+ArrowLeft");
    await expect.poll(async () => (await savedConfig(page)).limits.replace_warn_chars).toBeLessThan(8000);
    await replace.dblclick();
    await expect(page.locator("#replace_warn")).toBeFocused();
    expect(problems).toEqual([]);
  });
});
