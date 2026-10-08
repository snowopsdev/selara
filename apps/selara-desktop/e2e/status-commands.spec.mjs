import { test, expect } from "@playwright/test";
import { openSettings, showSection, artifactPath, settle } from "./helpers.mjs";

// Status readiness (3A), the try-it pad (3C), the inline command editor (4A),
// the card shelf (4B), and the motion system (9).
const mock = (page, fn, arg) => page.evaluate(fn, arg);
const commands = (page) => mock(page, () => window.__selaraMock.state.config.commands);
const savesOf = (page, section) => mock(page, (s) => window.__selaraMock.calls.filter((c) => c.cmd === "save_config_section" && c.args.section === s).length, section);

test.describe("Status readiness", () => {
  test("a working setup shows the readiness hero with the custom-instruction keys", async ({ page }, testInfo) => {
    const problems = await openSettings(page);
    const hero = page.locator("#status-hero");
    await expect(hero).toBeVisible();
    await expect(page.locator("#status-hero-title")).toHaveText("Ready");
    await expect(page.locator("#status-hero-mark")).not.toHaveClass(/untested/);
    await expect(hero.locator(".kc")).toHaveText(["⌃", "⇧", "Space"]);
    await expect(page.locator(".status-pip .pip-face .v")).toHaveText(["Running", "Granted", "gpt-5.4-mini", "4 active"]);
    await expect(page.locator(".setup-steps")).toHaveCount(0);
    // Each pip opens the detail the old card held; one at a time, Escape closes.
    await page.locator("#status-provider .pip-face").click();
    await expect(page.locator("#status-check-provider")).toBeVisible();
    await page.locator("#status-serve .pip-face").click();
    await expect(page.locator("#status-provider-pop")).toBeHidden();
    await expect(page.locator("#serve-restart")).toBeVisible();
    await settle(page);
    await page.screenshot({ path: artifactPath(testInfo, "flow-status-pip.png") });
    await page.keyboard.press("Escape");
    await expect(page.locator("#status-serve-pop")).toBeHidden();
    await expect(page.locator("#status-serve .pip-face")).toBeFocused();
    expect(problems).toEqual([]);
  });

  test("a serve started in Terminal reaches Ready with an Accessibility caveat", async ({ page }) => {
    const problems = await openSettings(page, "configured", "&serve=external");
    await expect(page.locator("#status-hero-title")).toHaveText("Ready");
    await expect(page.locator(".setup-steps")).toHaveCount(0);
    await expect(page.locator("#status-try-open")).toBeVisible();
    await expect(page.locator("#status-serve-value")).toHaveText("Running outside");
    await expect(page.locator("#status-accessibility-value")).toHaveText("Can’t verify");
    await expect(page.locator("#status-accessibility .pip-face .status-dot")).toHaveClass(/unknown/);
    await page.locator("#status-accessibility .pip-face").click();
    await expect(page.locator("#status-ax-caveat")).toContainText("can only be checked for a serve started from this Settings app");
    expect(problems).toEqual([]);
  });

  test("Ready stays honest until a request has gone through the provider", async ({ page }) => {
    const problems = await openSettings(page);
    await mock(page, () => { window.__selaraMock.state.usage.providers = []; });
    await page.locator("#status-refresh").click();
    await expect(page.locator("#status-hero-title")).toHaveText("Ready · provider not yet tested");
    await expect(page.locator("#status-hero-mark")).toHaveClass(/untested/);
    await expect(page.locator("#status-provider .pip-face .status-dot")).toHaveClass(/unknown/);
    await page.locator("#status-provider .pip-face").click();
    await page.locator("#status-check-provider").click();
    await expect(page.locator("#status-hero-title")).toHaveText("Ready");
    expect(problems).toEqual([]);
  });

  test("a fresh install shows three ordered setup steps that advance live", async ({ page }, testInfo) => {
    const problems = await openSettings(page, "fresh");
    await expect(page.locator(".setup-head h2")).toHaveText("Welcome to Selara");
    await expect(page.locator(".setup-head p")).toHaveText("Three steps, about a minute.");
    const steps = page.locator(".setup-step");
    await expect(steps).toHaveCount(3);
    await expect(steps.nth(0)).toHaveClass(/now/);
    await expect(steps.nth(1)).toHaveClass(/later/);
    await expect(steps.nth(2)).toHaveClass(/later/);
    await expect(page.locator(".setup-progress")).toHaveAttribute("aria-valuenow", "0");
    await expect(page.locator("#serve-start")).toBeVisible();
    await expect(page.locator("#status-open-ax")).toBeVisible();
    await expect(page.locator("#status-hero")).toHaveCount(0);
    await settle(page);
    await page.screenshot({ path: artifactPath(testInfo, "flow-status-setup.png") });

    await page.locator("#serve-start").click();
    await expect(steps.nth(0)).toHaveClass(/done/);
    await expect(steps.nth(1)).toHaveClass(/now/);
    await expect(page.locator(".setup-progress")).toHaveAttribute("aria-valuenow", "1");
    await expect(page.locator(".setup-head p")).toHaveText("Two steps left.");

    // The grant arrives while Settings watches after "Open Settings".
    await page.locator("#status-open-ax").click();
    await mock(page, () => { window.__selaraMock.state.ax = "granted"; });
    await expect(steps.nth(1)).toHaveClass(/done/, { timeout: 5000 });
    await expect(steps.nth(2)).toHaveClass(/now/);
    await page.locator("#status-choose-provider").click();
    await expect(page.locator("#section-models")).toBeVisible();
    expect(problems).toEqual([]);
  });

  test("steps advance on serve-changed events", async ({ page }) => {
    const problems = await openSettings(page, "errors");
    await expect(page.locator('.setup-step[data-step="serve"]')).toHaveClass(/now/);
    await mock(page, () => {
      window.__selaraMock.state.serveRunning = true;
      window.__selaraMock.emit("serve-changed", null);
    });
    // Accessibility and the provider were already in place, so the last
    // missing step turns the page into the readiness hero.
    await expect(page.locator("#status-hero")).toBeVisible();
    await expect(page.locator(".setup-steps")).toHaveCount(0);
    expect(problems).toEqual([]);
  });
});

test.describe("Try-it pad", () => {
  test("Status runs a command on sample text and shows the word diff", async ({ page }, testInfo) => {
    const problems = await openSettings(page);
    await page.locator("#status-try-open").click();
    const pad = page.locator("#status-try-pad");
    await expect(pad.locator(".try-text")).toHaveValue("Thanks for you're help with the launch, its been great.");
    await expect(pad.locator(".try-chips .btn")).toHaveCount(6);
    await pad.locator('.try-chips [data-id="professional"]').click();
    await expect(pad.locator(".try-result ins").first()).toBeVisible();
    await expect(pad.locator(".try-result ins").first()).toHaveText("Thank you");
    await expect(pad.locator(".try-meta")).toContainText(/^Professional · gpt-5\.4-mini · \d+(\.\d)? s$/);
    const call = await mock(page, () => window.__selaraMock.calls.filter((c) => c.cmd === "try_command").at(-1).args);
    expect(call.prompt).toContain("professional tone");
    expect(call.model).toBeNull();
    expect(await mock(page, () => window.__selaraMock.state.usage.today.requests)).toBe(13);
    await settle(page);
    await page.screenshot({ path: artifactPath(testInfo, "flow-status-try.png") });

    await pad.locator(".try-text").fill("hey can u send the numbers asap");
    await page.keyboard.press("Meta+r");
    await expect(pad.locator(".try-result del")).toHaveText(["hey can u", "asap"]);
    await expect(pad.locator(".try-result ins")).toHaveText(["Could you", "when you have a moment?"]);
    expect(problems).toEqual([]);
  });

  test("errors show inline in the result", async ({ page }) => {
    const problems = await openSettings(page);
    await page.locator("#status-try-open").click();
    const pad = page.locator("#status-try-pad");
    await pad.locator(".try-text").fill("   ");
    await pad.locator(".try-run").click();
    await expect(pad.locator(".try-error")).toHaveText("Enter some sample text to try the command on.");
    await expect(pad.locator(".try-meta")).toContainText("didn’t run");
    expect(problems).toEqual([]);
  });

  test("the command editor tries the unsaved prompt with ⌘R", async ({ page }) => {
    const problems = await openSettings(page, "fresh");
    await showSection(page, "commands");
    await expect(page.locator("#cmd-try .try-src")).toHaveText("Sample");
    await expect(page.locator("#cmd-try .try-text")).toHaveValue("hey can u send the numbers asap");
    await page.locator("#cmd-prompt").fill("Rewrite this in a professional tone.");
    await page.keyboard.press("Meta+r");
    // Fresh has no key yet, so the provider's own error shows in place.
    await expect(page.locator("#cmd-try .try-error")).toContainText("401 Unauthorized");
    const call = await mock(page, () => window.__selaraMock.calls.filter((c) => c.cmd === "try_command").at(-1).args);
    expect(call.prompt).toBe("Rewrite this in a professional tone.");
    expect(await savesOf(page, "commands")).toBe(0);
    expect(problems).toEqual([]);
  });
});

test.describe("Inline command editor", () => {
  test("fields save when committed and show an inline check", async ({ page }) => {
    const problems = await openSettings(page);
    await showSection(page, "commands");
    await page.locator('.cmd-item[data-id="friendly"]').click();
    await page.locator("#cmd-label").fill("Warm");
    await page.locator("#cmd-label").press("Enter");
    await expect.poll(async () => (await commands(page)).find((c) => c.id === "friendly").label).toBe("Warm");
    await expect(page.locator(".saved-tick.show")).toHaveText("✓ Saved");
    await expect(page.locator('.cmd-item[data-id="friendly"] .cmd-name')).toHaveText("Warm");
    await expect(page.locator("#save-status")).toHaveText("Saved");
    await expect(page.locator(".nav-feedback")).toHaveClass(/quiet/);
    await expect(page.locator(".saved-tick")).toHaveCount(0, { timeout: 4000 });

    await page.locator("#cmd-prompt").click();
    await page.keyboard.press("End");
    await page.locator('.tokbar [data-insert="{{language}}"]').click();
    await expect.poll(async () => (await commands(page)).find((c) => c.id === "friendly").prompt).toBe("Rewrite the text in a warm, friendly tone. {{language}}");
    await expect(page.locator("#cmd-editor .prompt-backdrop mark")).toHaveText("{{language}}");
    expect(problems).toEqual([]);
  });

  test("Review before replacing persists to the command's review field", async ({ page }) => {
    const problems = await openSettings(page);
    await showSection(page, "commands");
    await page.locator('.cmd-item[data-id="friendly"]').click();
    await page.locator("#cmd-advanced > summary").click();
    await expect(page.locator("#cmd-review")).not.toBeChecked();
    await page.locator("#cmd-review").check();
    await expect.poll(async () => (await commands(page)).find((c) => c.id === "friendly").review).toBe(true);
    await page.locator('.cmd-item[data-id="professional"]').click();
    await expect(page.locator("#cmd-review")).toBeChecked();
    await page.locator("#cmd-review").uncheck();
    await expect.poll(async () => "review" in (await commands(page)).find((c) => c.id === "professional")).toBe(false);
    expect(problems).toEqual([]);
  });

  test("icon and color are picked from a popover and saved", async ({ page }) => {
    const problems = await openSettings(page);
    await showSection(page, "commands");
    await page.locator('.cmd-item[data-id="friendly"]').click();
    await expect(page.locator("#cmd-glyph .glyph-tile")).toHaveText("F");
    await page.locator("#cmd-glyph").click();
    await expect(page.locator("#cmd-identity-pop")).toBeVisible();
    await page.locator('#cmd-identity-pop [data-glyph="★"]').click();
    await page.locator('#cmd-identity-pop [data-color="#bf5af2"]').click();
    await expect.poll(async () => {
      const c = (await commands(page)).find((x) => x.id === "friendly");
      return [c.glyph, c.color];
    }).toEqual(["★", "#bf5af2"]);
    await expect(page.locator('.cmd-item[data-id="friendly"] .glyph-tile')).toHaveText("★");
    await page.keyboard.press("Escape");
    await expect(page.locator("#cmd-identity-pop")).toBeHidden();
    expect(problems).toEqual([]);
  });

  test("a save that fails after switching commands is reported above the list and kept", async ({ page }) => {
    const problems = await openSettings(page);
    await showSection(page, "commands");
    await page.locator('.cmd-item[data-id="proofread"]').click();
    await mock(page, () => { window.__selaraMock.state.failSaves = true; });
    await page.locator("#cmd-prompt").fill("Fix only the typos.");
    await page.locator('.cmd-item[data-id="rewrite"]').click();
    await expect(page.locator("#commands-error")).toContainText("Could not save “Proofread”.");
    await expect(page.locator("#commands-error")).toContainText("Permission denied");
    await expect(page.locator("#cmd-label")).toHaveValue("Rewrite");
    await expect(page.locator("#cmd-save-error")).toBeHidden();
    // Reselecting shows the failed draft again.
    await page.locator('.cmd-item[data-id="proofread"]').click();
    await expect(page.locator("#cmd-prompt")).toHaveValue("Fix only the typos.");
    await mock(page, () => { window.__selaraMock.state.failSaves = false; });
    await page.locator("#cmd-retry-save").click();
    await expect.poll(async () => (await commands(page)).find((c) => c.id === "proofread").prompt).toBe("Fix only the typos.");
    await expect(page.locator("#commands-error")).toBeHidden();

    // A failed create after switching rows doesn't vanish either.
    await mock(page, () => { window.__selaraMock.state.failSaves = true; });
    await page.locator("#cmd-new").click();
    await page.locator("#cmd-label").fill("Shorter");
    await page.locator("#cmd-prompt").fill("Cut the text to half its length.");
    await page.locator('.cmd-item[data-id="rewrite"]').click();
    await expect(page.locator("#commands-error")).toContainText("Could not save “Shorter”.");
    await expect(page.locator("#cmd-label")).toHaveValue("Rewrite");
    expect((await commands(page)).some((c) => c.label === "Shorter")).toBe(false);
    await mock(page, () => { window.__selaraMock.state.failSaves = false; });
    await page.locator("#cmd-retry-save").click();
    await expect.poll(async () => (await commands(page)).find((c) => c.label === "Shorter")?.prompt).toBe("Cut the text to half its length.");
    await expect(page.locator("#commands-error")).toBeHidden();
    await expect(page.locator(".cmd-item .cmd-name").last()).toHaveText("Shorter");
    expect(problems).toEqual([]);
  });

  test("quick ⌥↓ presses in the list keep every move", async ({ page }) => {
    const problems = await openSettings(page);
    await showSection(page, "commands");
    const row = page.locator('.cmd-item[data-id="proofread"]');
    await row.click();
    await row.focus();
    await page.keyboard.press("Alt+ArrowDown");
    await page.keyboard.press("Alt+ArrowDown");
    await expect.poll(async () => (await commands(page)).map((c) => c.id)).toEqual(["rewrite", "friendly", "proofread", "professional", "concise", "translate"]);
    expect(await page.locator(".cmd-rows .cmd-item").evaluateAll((els) => els.map((el) => el.dataset.id))).toEqual(["rewrite", "friendly", "proofread", "professional", "concise", "translate"]);
    await expect(page.locator('.cmd-item[data-id="proofread"]')).toBeFocused();
    await expect(page.locator("#cmd-live")).toHaveText("Moved Proofread to position 3 of 6.");
    expect(problems).toEqual([]);
  });

  test("a blank label shakes instead of saving", async ({ page }) => {
    const problems = await openSettings(page);
    await showSection(page, "commands");
    const before = await savesOf(page, "commands");
    await page.locator("#cmd-label").fill("");
    await page.locator("#cmd-label").press("Enter");
    await expect(page.locator("#cmd-label")).toHaveClass(/field-invalid/);
    await expect(page.locator("#cmd-editor .field-error")).toHaveText("A command needs a label.");
    expect(await page.locator("#cmd-label").evaluate((el) => getComputedStyle(el).animationName)).toBe("field-shake");
    expect(await savesOf(page, "commands")).toBe(before);
    await page.locator("#cmd-label").fill("Proofread");
    await expect(page.locator("#cmd-label")).not.toHaveClass(/field-invalid/);
    expect(problems).toEqual([]);
  });

  test("a new command saves once it has a label and a prompt", async ({ page }) => {
    const problems = await openSettings(page);
    await showSection(page, "commands");
    await page.locator("#cmd-new").click();
    await expect(page.locator("#cmd-label")).toBeFocused();
    await expect(page.locator(".cmd-item.draft .cmd-name")).toHaveText("New Command");
    await page.locator("#cmd-label").fill("Shorter");
    await expect(page.locator(".cmd-item.draft .cmd-name")).toHaveText("Shorter");
    await page.locator("#cmd-label").press("Tab");
    expect(await commands(page)).toHaveLength(6);
    await page.locator("#cmd-prompt").fill("Cut the text to half its length.");
    await page.locator("#cmd-label").focus();
    await expect.poll(async () => (await commands(page)).length).toBe(7);
    const created = (await commands(page)).at(-1);
    expect(created).toMatchObject({ label: "Shorter", prompt: "Cut the text to half its length.", kind: "replace" });
    await expect(page.locator(`.cmd-item[data-id="${created.id}"]`)).toHaveAttribute("aria-current", "true");
    await expect(page.locator("#cmd-delete")).toBeVisible();
    expect(problems).toEqual([]);
  });

  test("Delete asks with a native confirmation", async ({ page }) => {
    const problems = await openSettings(page);
    await showSection(page, "commands");
    await page.locator('.cmd-item[data-id="rewrite"]').click();
    await page.locator("#cmd-delete").click();
    await expect.poll(async () => (await commands(page)).some((c) => c.id === "rewrite")).toBe(false);
    const ask = await mock(page, () => window.__selaraMock.calls.filter((c) => c.cmd === "confirm_action").at(-1).args);
    expect(ask).toMatchObject({ confirmLabel: "Delete Command" });
    await expect(page.locator(".cmd-item[aria-current=true]")).toHaveAttribute("data-id", "friendly");
    expect(problems).toEqual([]);
  });

  test("at the minimum size the Try row stacks below its label", async ({ page }, testInfo) => {
    test.skip(!testInfo.project.name.startsWith("min"), "minimum-size layout");
    const problems = await openSettings(page);
    await showSection(page, "commands");
    const label = await page.locator(".try-row > .ed-label").boundingBox();
    const pad = await page.locator("#cmd-try").boundingBox();
    expect(pad.y).toBeGreaterThan(label.y + label.height - 1);
    const boxes = await page.locator("#cmd-try .try-box").evaluateAll((els) => els.map((el) => el.getBoundingClientRect().y));
    expect(boxes[1]).toBeGreaterThan(boxes[0]);
    expect(problems).toEqual([]);
  });
});

test.describe("Card shelf", () => {
  test("a remembered Grid view opens with Grid selected", async ({ page }) => {
    await page.addInitScript(() => localStorage.setItem("selara.commandView", "grid"));
    const problems = await openSettings(page);
    const section = await showSection(page, "commands");
    await expect(section.locator("#cmd-grid")).toBeVisible();
    await expect(section.locator('.segmented[data-for="cmd-view"] .seg[data-value="grid"]')).toHaveAttribute("aria-checked", "true");
    await expect(section.locator("#cmd-view")).toHaveValue("grid");
    expect(problems).toEqual([]);
  });

  const grid = async (page) => {
    await showSection(page, "commands");
    await page.locator('.segmented[data-for="cmd-view"] .seg[data-value="grid"]').click();
    await expect(page.locator("#cmd-grid")).toBeVisible();
  };
  const menuOrder = (page) => page.locator("#cmd-menu-preview .mi[data-id]").evaluateAll((els) => els.map((el) => el.dataset.id));

  test("the menu-bar preview mirrors the order and ⌥↓ moves a card", async ({ page }, testInfo) => {
    const problems = await openSettings(page);
    await grid(page);
    await expect(page.locator("#cmd-menu-preview .mi").first()).toContainText("Custom instruction…");
    expect(await menuOrder(page)).toEqual(["proofread", "rewrite", "friendly", "professional", "concise", "translate"]);
    await page.locator('.cmd-card[data-id="proofread"]').focus();
    await page.keyboard.press("Alt+ArrowDown");
    await expect(page.locator('.cmd-card[data-id="proofread"]')).toBeFocused();
    expect(await menuOrder(page)).toEqual(["rewrite", "proofread", "friendly", "professional", "concise", "translate"]);
    await expect.poll(async () => (await commands(page)).map((c) => c.id)).toEqual(["rewrite", "proofread", "friendly", "professional", "concise", "translate"]);
    await expect(page.locator("#cmd-live")).toHaveText("Moved Proofread to position 2 of 6.");
    await settle(page);
    await page.screenshot({ path: artifactPath(testInfo, "flow-command-shelf.png") });
    expect(problems).toEqual([]);
  });

  test("dragging a card reorders it with a lift and saves the order", async ({ page }) => {
    const problems = await openSettings(page);
    await grid(page);
    const from = await page.locator('.cmd-card[data-id="translate"]').boundingBox();
    const to = await page.locator('.cmd-card[data-id="proofread"]').boundingBox();
    await page.mouse.move(from.x + 40, from.y + 40);
    await page.mouse.down();
    await page.mouse.move(from.x + 30, from.y + 20, { steps: 3 });
    await expect(page.locator('.cmd-card[data-id="translate"]')).toHaveClass(/lifted/);
    expect(await page.locator('.cmd-card[data-id="translate"]').evaluate((el) => el.style.transform)).toContain("rotate(-2deg) scale(1.04)");
    await expect(page.locator("#cmd-menu-preview .mi.hl")).toHaveText("Translate");
    await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 12 });
    await page.mouse.up();
    await expect(page.locator('.cmd-card[data-id="translate"]')).not.toHaveClass(/lifted/);
    await expect.poll(async () => (await commands(page))[0].id).toBe("translate");
    expect((await menuOrder(page))[0]).toBe("translate");
    expect(problems).toEqual([]);
  });

  test("clicking a card opens it in the list editor", async ({ page }) => {
    const problems = await openSettings(page);
    await grid(page);
    await page.locator('.cmd-card[data-id="concise"]').click();
    await expect(page.locator("#cmd-label")).toHaveValue("Concise");
    await expect(page.locator(".cmd-item[aria-current=true]")).toHaveAttribute("data-id", "concise");
    expect(problems).toEqual([]);
  });
});

test.describe("Motion", () => {
  test("tokens are defined and page switches crossfade", async ({ page }) => {
    const problems = await openSettings(page);
    const tokens = await page.evaluate(() => Object.fromEntries(["--motion-instant", "--motion-micro", "--motion-panel", "--motion-spring", "--motion-stagger", "--motion-hold"].map((n) => [n, getComputedStyle(document.documentElement).getPropertyValue(n).trim()])));
    expect(tokens).toEqual({ "--motion-instant": "0ms", "--motion-micro": "120ms", "--motion-panel": "240ms", "--motion-spring": "420ms", "--motion-stagger": "38ms", "--motion-hold": "1800ms" });
    await page.locator('.nav-item[data-section="general"]').click();
    const anim = await page.locator("#section-general").evaluate((el) => ({ name: getComputedStyle(el).animationName, duration: getComputedStyle(el).animationDuration }));
    expect(anim).toEqual({ name: "motion-fade", duration: "0.12s" });
    await expect(page.locator('.nav-item[data-section="general"]')).toHaveClass(/active/);
    expect(problems).toEqual([]);
  });

  test("Reduce Motion collapses tokens and turns the shake into an outline", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    const problems = await openSettings(page);
    expect(await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--motion-panel").trim())).toBe("120ms");
    await showSection(page, "limits");
    await page.locator("#soft_warn").fill("");
    await page.locator("#soft_warn").press("Enter");
    await expect(page.locator("#soft_warn")).toHaveClass(/field-invalid/);
    const field = await page.locator("#soft_warn").evaluate((el) => ({ animation: getComputedStyle(el).animationName, outline: getComputedStyle(el).outlineStyle }));
    expect(field).toEqual({ animation: "none", outline: "solid" });
    await page.locator('.nav-item[data-section="commands"]').click();
    expect(await page.locator("#section-commands").evaluate((el) => getComputedStyle(el).animationName)).toBe("motion-fade");
    await page.locator('.segmented[data-for="cmd-view"] .seg[data-value="grid"]').click();
    await page.locator('.cmd-card[data-id="proofread"]').focus();
    await page.keyboard.press("Alt+ArrowDown");
    expect(await page.locator(".cmd-card.flip").evaluateAll((els) => els.map((el) => getComputedStyle(el).transitionProperty))).not.toContain("transform");
    expect(problems).toEqual([]);
  });
});
