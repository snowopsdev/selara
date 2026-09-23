import { test } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { SECTIONS, openSettings, showSection, artifactPath } from "./helpers.mjs";

// Report-only: measures styling that reads as "web page" rather than macOS
// app, for the native-feel audit. Nothing here fails; compare the JSON across
// runs as the UI changes.
test("collect native-feel metrics", async ({ page }, testInfo) => {
  await openSettings(page);
  const perSection = {};
  for (const name of SECTIONS) {
    await showSection(page, name);
    perSection[name] = await page.evaluate(collectVisible);
  }
  await showSection(page, "commands");
  await page.locator("#cmd-new").click();
  perSection.commandSheet = await page.evaluate(collectVisible);
  const global = await page.evaluate(collectStylesheet);

  const report = { project: testInfo.project.name, generated: new Date().toISOString(), global, sections: perSection };
  const file = artifactPath(testInfo, "native-lint.json");
  mkdirSync(file.replace(/\/[^/]+$/, ""), { recursive: true });
  writeFileSync(file, JSON.stringify(report, null, 2));
  await testInfo.attach("native-lint", { body: JSON.stringify(report, null, 2), contentType: "application/json" });
});

// Runs in the page. Only visible elements count, so each section is measured
// as the user sees it.
function collectVisible() {
  const describe = (el) => {
    let s = el.tagName.toLowerCase();
    if (el.id) s += "#" + el.id;
    const cls = [...el.classList].slice(0, 3).join(".");
    if (cls) s += "." + cls;
    return s;
  };
  const visible = (el) => {
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0 && getComputedStyle(el).visibility !== "hidden";
  };
  // user-select computes to "auto" on descendants; resolve it the way the
  // engine does by walking up to the first explicit value.
  const selectableByUser = (el) => {
    for (let node = el; node && node.nodeType === 1; node = node.parentElement) {
      const cs = getComputedStyle(node);
      const value = cs.webkitUserSelect || cs.userSelect;
      if (value && value !== "auto") return value !== "none";
    }
    return true;
  };
  const tally = (list) => {
    const out = {};
    for (const item of list) out[item] = (out[item] || 0) + 1;
    return out;
  };
  const els = [...document.querySelectorAll("body *")].filter(visible);
  const pointer = [];
  const shadows = [];
  const blur = [];
  const pills = [];
  const selectable = [];
  const fonts = [];
  const fontSizes = [];
  for (const el of els) {
    const cs = getComputedStyle(el);
    if (cs.cursor === "pointer") pointer.push(describe(el));
    if (cs.boxShadow && cs.boxShadow !== "none") shadows.push(describe(el) + " → " + cs.boxShadow);
    const bf = cs.backdropFilter || cs.webkitBackdropFilter;
    if (bf && bf !== "none") blur.push(describe(el) + " → " + bf);
    const radius = parseFloat(cs.borderTopLeftRadius);
    if ((el.tagName === "BUTTON" || el.classList.contains("btn")) && radius >= el.getBoundingClientRect().height / 2 - 0.5) pills.push(describe(el));
    const ownText = [...el.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim());
    if (ownText && !el.closest("input, textarea, select") && selectableByUser(el)) selectable.push(describe(el));
    if (ownText) {
      fonts.push(cs.fontFamily);
      fontSizes.push(cs.fontSize + "/" + cs.fontWeight);
    }
  }
  const accentProbe = document.querySelector(".btn:not(.secondary):not(.danger)");
  const focusable = [...document.querySelectorAll("button, input, select, textarea, summary, [tabindex]")].filter(visible);
  return {
    visibleElements: els.length,
    cursorPointer: pointer,
    boxShadows: shadows,
    backdropFilters: blur,
    pillButtons: { count: pills.length, examples: pills.slice(0, 8) },
    selectableText: { count: selectable.length, examples: selectable.slice(0, 8) },
    fontFamilies: tally(fonts),
    fontSizesWeights: tally(fontSizes),
    headings: [...document.querySelectorAll("h1, h2")].filter(visible).map((h) => h.tagName + " " + getComputedStyle(h).fontSize + " " + h.textContent.trim().slice(0, 40)),
    primaryButtonColor: accentProbe ? getComputedStyle(accentProbe).backgroundColor : null,
    accentVar: getComputedStyle(document.documentElement).getPropertyValue("--accent").trim(),
    focusables: focusable.length,
    nativeControls: {
      selects: document.querySelectorAll("select").length,
      appearanceNone: [...document.querySelectorAll("select, input, button")].filter((el) => getComputedStyle(el).appearance === "none" || getComputedStyle(el).webkitAppearance === "none").length,
      checkboxes: document.querySelectorAll("input[type=checkbox]").length,
    },
    externalLinks: [...document.querySelectorAll("a[target=_blank]")].map((a) => a.href),
  };
}

function collectStylesheet() {
  const hover = [];
  const focus = [];
  const transitions = [];
  const animations = [];
  const media = [];
  const walk = (rules) => {
    for (const rule of rules) {
      if (rule.cssRules && rule.conditionText !== undefined) {
        media.push(rule.conditionText);
        walk(rule.cssRules);
        continue;
      }
      if (!rule.selectorText) continue;
      if (rule.selectorText.includes(":hover")) hover.push(rule.selectorText);
      if (/:focus(-visible|-within)?/.test(rule.selectorText)) focus.push(rule.selectorText + " { " + ["outline", "box-shadow"].map((p) => rule.style.getPropertyValue(p) && p + ": " + rule.style.getPropertyValue(p)).filter(Boolean).join("; ") + " }");
      if (rule.style.transition) transitions.push(rule.selectorText + " → " + rule.style.transition);
      if (rule.style.animation || rule.style.animationName) animations.push(rule.selectorText + " → " + (rule.style.animation || rule.style.animationName));
    }
  };
  for (const sheet of document.styleSheets) {
    try { walk(sheet.cssRules); } catch (_) { /* cross-origin */ }
  }
  return { hoverRules: hover, focusRules: focus, transitions, animations, mediaQueries: [...new Set(media)] };
}
