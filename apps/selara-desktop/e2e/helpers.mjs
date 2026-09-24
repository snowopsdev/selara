import { expect } from "@playwright/test";

export const SECTIONS = ["status", "general", "models", "commands", "history", "usage", "limits"];

// Open the Settings UI in a mock scenario and wait until the initial load
// finishes. Page errors, and commands the mock does not implement, fail the
// test through the returned `problems` list.
export async function openSettings(page, scenario = "configured", extra = "") {
  const problems = [];
  page.on("pageerror", (err) => problems.push("pageerror: " + err.message));
  page.on("console", (msg) => {
    const text = msg.text();
    if (msg.type() === "error") problems.push("console.error: " + text);
    if (text.includes("[mock-tauri] unhandled command")) problems.push(text);
  });
  await page.goto(`/?scenario=${scenario}&delay=20${extra}`);
  await expect(page.locator("html")).toHaveAttribute("data-mock-bridge", scenario);
  if (!extra.includes("fail=config")) await expect(page.locator("#save-status")).toHaveText("Loaded");
  return problems;
}

export async function showSection(page, name) {
  await page.locator(`.nav-item[data-section="${name}"]`).click();
  const section = page.locator(`#section-${name}`);
  await expect(section).toBeVisible();
  return section;
}

export function artifactPath(testInfo, file) {
  return `e2e/artifacts/${testInfo.project.name}/${file}`;
}

// Wait for CSS transitions/animations to finish so screenshots show the
// resting state, not a mid-fade frame.
export async function settle(page) {
  await page.waitForFunction(() => document.getAnimations().every((a) => a.playState !== "running" || a.effect?.getComputedTiming().iterations === Infinity));
}
