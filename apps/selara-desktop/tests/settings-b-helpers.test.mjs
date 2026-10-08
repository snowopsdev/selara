// Tests for the pure Providers/History/Usage/General/Limits helpers in
// ../index.html, sliced out of the `// @selara-b-helpers-start` /
// `// @selara-b-helpers-end` block (see helpers.test.mjs for why).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const START = "// @selara-b-helpers-start";
const END = "// @selara-b-helpers-end";
const NAMES = [
  "historyCommandIdentity", "diffTokens", "historyWordDiff", "diffOverlap", "historyDiffMode", "historyDayLabel",
  "historyOutcomeText", "friendlyModelName", "roundedCost", "usageTileSub", "usageShare", "fmtMs", "fmtTimeSaved",
  "receiptStats", "rulerPos", "rulerValue", "rulerNice", "rulerClamp", "rulerStep", "pagesLabel", "languageInfo",
  "languageMatches", "languageBlurValue", "hotkeyKeycapList", "excludedAppName", "appMonogram", "USAGE_KIND_FOR_CHOICE",
];

function loadHelpers() {
  const html = readFileSync(fileURLToPath(new URL("../index.html", import.meta.url)), "utf8");
  const start = html.indexOf(START);
  const end = html.indexOf(END);
  assert.ok(start !== -1 && end > start, "settings-b helper markers present and ordered");
  const body = html.slice(start + START.length, end);
  const code = body.slice(body.indexOf("\n") + 1);
  return vm.runInNewContext(`${code}\n({${NAMES.join(", ")}})`, {}, { filename: "index.html#settings-b-helpers" });
}

const H = loadHelpers();
const plain = (value) => JSON.parse(JSON.stringify(value));
const render = (ops) => ops.map((t) => (t.op === "eq" ? t.text : t.op === "del" ? `[-${t.text}-]` : `{+${t.text}+}`)).join("");

test("block exposes every helper", () => {
  for (const name of NAMES) assert.ok(H[name] !== undefined, name);
});

test("historyWordDiff marks the changed words of a proofread", () => {
  const ops = H.historyWordDiff("Thanks for you're help with the launch, its been great.", "Thanks for your help with the launch; it's been great.");
  assert.equal(render(ops), "Thanks for [-you're-]{+your+} help with the launch[-, its-]{+; it's+} been great.");
});

test("historyWordDiff keeps both texts recoverable", () => {
  const a = "I just wanted to quickly follow up and check in on whether you had a chance to take a look at the document I sent over last week.";
  const b = "Did you get a chance to review the document I sent last week?";
  const ops = H.historyWordDiff(a, b);
  assert.equal(ops.filter((t) => t.op !== "ins").map((t) => t.text).join(""), a);
  assert.equal(ops.filter((t) => t.op !== "del").map((t) => t.text).join(""), b);
  assert.match(render(ops), /\[-take a look at-\]\{\+review\+\}/);
  assert.match(render(ops), /^\[-I just wanted to quickly follow up and check in on whether you had-\]\{\+Did you get\+\} a chance/, "a lone shared word stays inside the big replacement");
});

test("historyWordDiff handles empty and identical input, and refuses huge input", () => {
  assert.deepEqual(plain(H.historyWordDiff("", "new")), [{ op: "ins", text: "new" }]);
  assert.deepEqual(plain(H.historyWordDiff("same text", "same text")), [{ op: "eq", text: "same text" }]);
  assert.equal(H.historyWordDiff("a ".repeat(600), "b ".repeat(600)), null);
});

test("historyDiffMode shows a diff for edits and the result alone for rewrites", () => {
  assert.equal(H.historyDiffMode({ original: "hey can u send the numbers asap", result: "Could you send the numbers when you have a moment?" }), "diff");
  assert.equal(H.historyDiffMode({ original: "Merci beaucoup pour votre patience.", result: "Thank you very much for your patience." }), "result");
  assert.equal(H.historyDiffMode({ command_id: "summary", original: "Long text here", result: "Long text" }), "result");
  assert.equal(H.historyDiffMode({ label: "Key Points", original: "a b c", result: "a b" }), "result");
  assert.equal(H.historyDiffMode({ original: "one, two, three", result: "- one\n- two\n- three" }), "result");
  assert.equal(H.historyDiffMode({ original: "", result: "Legacy" }), "result");
  assert.equal(H.historyDiffMode({ original: "x ".repeat(800), result: "x ".repeat(799) }), "result");
});

test("historyDayLabel groups by local day", () => {
  const now = new Date(2026, 9, 8, 15, 0).getTime() / 1000;
  const at = (d, h = 9) => new Date(2026, 9, d, h, 0).getTime() / 1000;
  assert.equal(H.historyDayLabel(at(8, 1), now), "Today");
  assert.equal(H.historyDayLabel(at(7, 23), now), "Yesterday");
  assert.equal(H.historyDayLabel(at(5), now), "Monday");
  assert.equal(H.historyDayLabel(at(1), now), "Oct 1");
  assert.equal(H.historyDayLabel(new Date(2025, 11, 30).getTime() / 1000, now), "Dec 30, 2025");
});

test("historyOutcomeText names the app for problem outcomes only", () => {
  assert.equal(H.historyOutcomeText({ outcome: "not_applied", app: "Slack" }), "Not applied: still in Slack as before");
  assert.equal(H.historyOutcomeText({ outcome: "paste_unverified", app: "Notes" }), "Check Notes: paste may have worked");
  assert.equal(H.historyOutcomeText({ outcome: "paste_unverified" }), "Check the app: paste may have worked");
  assert.equal(H.historyOutcomeText({ outcome: "applied", app: "Mail" }), "");
});

test("historyCommandIdentity honors glyph and color, else derives a stable monogram", () => {
  assert.deepEqual(plain(H.historyCommandIdentity({ id: "concise", label: "Concise", glyph: "✂︎", color: "#ff375f" })), { glyph: "✂︎", color: "#ff375f" });
  const derived = H.historyCommandIdentity({ id: "proofread", label: "proofread" });
  assert.equal(derived.glyph, "P");
  assert.match(derived.color, /^#[0-9a-f]{6}$/);
  assert.deepEqual(plain(H.historyCommandIdentity(null, "proofread", "Proofread")), plain(derived));
  assert.equal(H.historyCommandIdentity({ id: "x", label: "X", color: "red" }).color.startsWith("#"), true);
});

test("friendlyModelName explains CLI defaults and routed models", () => {
  assert.equal(H.friendlyModelName("claude_cli", ""), "Claude Code · default model");
  assert.equal(H.friendlyModelName("cursor_cli", "sonnet"), "sonnet · Cursor");
  assert.equal(H.friendlyModelName("openrouter", "anthropic/claude-opus-5"), "claude-opus-5 via OpenRouter");
  assert.equal(H.friendlyModelName("openai_compatible", "gpt-5.4-mini"), "gpt-5.4-mini");
  assert.equal(H.friendlyModelName("chatgpt_codex", "gpt-5.4"), "gpt-5.4 · ChatGPT");
});

test("costs are rounded so they do not imply false precision", () => {
  assert.equal(H.roundedCost(0.0061), "< $0.01");
  assert.equal(H.roundedCost(0.1482), "~$0.15");
  assert.equal(H.roundedCost(null), "");
  assert.equal(H.roundedCost(0), "$0");
  assert.equal(H.usageTileSub({ requests: 12, cost_usd: 0.0061 }), "requests · < $0.01");
  assert.equal(H.usageTileSub({ requests: 1031, cost_usd: 0.5427, unpriced: 170 }), "~$0.54 priced · 170 free/local");
  assert.equal(H.usageTileSub({ requests: 4, cost_usd: null, unpriced: 4 }), "requests · 4 free/local");
  assert.equal(H.usageTileSub({ requests: 0 }), "No requests yet");
});

test("usageShare orders providers by requests with percentages", () => {
  const share = H.usageShare([
    { kind: "claude_cli", model: "", requests: 128 },
    { kind: "openai_compatible", model: "gpt-5.4-mini", requests: 861 },
    { kind: "openrouter", model: "anthropic/claude-opus-5", requests: 42 },
  ]);
  assert.deepEqual(share.map((s) => [s.label, s.pct]), [["gpt-5.4-mini", 84], ["Claude Code · default model", 12], ["claude-opus-5 via OpenRouter", 4]]);
  assert.deepEqual(plain(H.usageShare([])), []);
});

test("receiptStats derives words and time from aggregates", () => {
  const r = H.receiptStats({
    all_time: { requests: 1031, output: 370880, cost_usd: 0.5427, unpriced: 170 },
    models: [{ kind: "openai_compatible", model: "gpt-5.4-mini", requests: 861 }, { kind: "claude_cli", model: "", requests: 128 }],
  });
  assert.deepEqual(plain(r), { rewrites: "1,031", words: "≈ 278,160", timeSaved: "≈ 17 h", favourite: "gpt-5.4-mini", cost: "$0.54", free: "170", perRewrite: "$0.0005", empty: false });
  const empty = H.receiptStats({ all_time: {}, models: [] });
  assert.equal(empty.empty, true);
  assert.equal(empty.rewrites, "0");
  assert.equal(empty.favourite, "—");
  assert.equal(H.fmtTimeSaved(45), "≈ 45 min");
  assert.equal(H.fmtTimeSaved(60 * 150), "≈ 6 days");
});

test("fmtMs reads like a stopwatch", () => {
  assert.equal(H.fmtMs(840), "840 ms");
  assert.equal(H.fmtMs(1100), "1.1 s");
  assert.equal(H.fmtMs(12400), "12 s");
  assert.equal(H.fmtMs(null), "");
});

test("the ruler maps characters to a log scale with an unlimited zone", () => {
  assert.equal(H.rulerPos(500), 0);
  assert.ok(Math.abs(H.rulerPos(200000) - 0.86) < 1e-9);
  assert.ok(H.rulerPos(0) > 0.9, "0 sits in the ∞ zone");
  for (const v of [1000, 4000, 8000, 100000]) assert.equal(H.rulerValue(H.rulerPos(v)), v);
  assert.equal(H.rulerValue(0.95), 0);
  assert.equal(H.rulerValue(0), 500);
  assert.equal(H.rulerNice(4321), 4300);
  assert.equal(H.rulerNice(123456), 120000);
});

test("ruler markers cannot cross, and unlimited sorts last", () => {
  const values = [4000, 8000, 100000];
  assert.equal(H.rulerClamp(values, 0, 9000), 8000);
  assert.equal(H.rulerClamp(values, 1, 2000), 4000);
  assert.equal(H.rulerClamp(values, 1, 0), 100000);
  assert.equal(H.rulerClamp(values, 2, 0), 0);
  assert.equal(H.rulerClamp([4000, 8000, 0], 1, 0), 0);
  assert.equal(H.rulerClamp([0, 8000, 100000], 0, 0), 0, "an unlimited marker stays unlimited");
});

test("an unlimited neighbour is skipped as a bound, never copied", () => {
  // [0, x, y]: replace is unlimited.
  assert.equal(H.rulerClamp([0, 8000, 100000], 1, 5000), 5000);
  assert.equal(H.rulerClamp([0, 8000, 100000], 1, 150000), 100000);
  assert.equal(H.rulerClamp([0, 8000, 100000], 2, 6000), 8000);
  assert.equal(H.rulerClamp([0, 8000, 100000], 0, 3000), 3000, "replace can leave unlimited");
  assert.equal(H.rulerClamp([0, 8000, 100000], 0, 9000), 8000);
  // [x, 0, y]: soft is unlimited, so replace and hard bound each other.
  assert.equal(H.rulerClamp([4000, 0, 100000], 2, 50000), 50000);
  assert.equal(H.rulerClamp([4000, 0, 100000], 2, 3000), 4000);
  assert.equal(H.rulerClamp([4000, 0, 100000], 0, 150000), 100000);
  assert.equal(H.rulerClamp([4000, 0, 100000], 0, 0), 100000, "replace can't become unlimited while hard is finite");
  assert.equal(H.rulerClamp([4000, 0, 100000], 1, 20000), 20000);
  assert.equal(H.rulerClamp([4000, 0, 100000], 1, 2000), 4000);
  // [x, y, 0]: hard is unlimited.
  assert.equal(H.rulerClamp([4000, 8000, 0], 1, 150000), 150000);
  assert.equal(H.rulerClamp([4000, 8000, 0], 2, 6000), 8000);
  assert.equal(H.rulerClamp([4000, 8000, 0], 2, 50000), 50000);
  assert.equal(H.rulerClamp([4000, 0, 0], 2, 3000), 4000, "hard leaves unlimited, bounded by the nearest finite marker");
  assert.equal(H.rulerClamp([4000, 0, 0], 0, 0), 0, "every later marker is unlimited");
});

test("keyboard steps never move a value against the key", () => {
  assert.ok(H.rulerStep(4000, 1, 0.02) > 4000);
  assert.ok(H.rulerStep(4000, -1, 0.02) < 4000);
  assert.ok(H.rulerStep(4000, -1, 0.1) < H.rulerStep(4000, -1, 0.02), "a page step is larger");
  assert.equal(H.rulerStep(200, -1, 0.02), 200, "below the scale, ← doesn't raise it to 500");
  assert.equal(H.rulerStep(200, -1, 0.1), 200);
  assert.ok(H.rulerStep(200, 1, 0.02) > 200);
  assert.equal(H.rulerStep(250000, 1, 0.02), 0, "above the scale, → goes on to no limit");
  assert.equal(H.rulerStep(250000, -1, 0.02) < 250000, true);
  assert.equal(H.rulerStep(0, 1, 0.02), 0);
  assert.equal(H.rulerStep(0, -1, 0.02), 200000);
});

test("pagesLabel uses 3,000 characters per page with correct plurals", () => {
  assert.equal(H.pagesLabel(3000), "≈ 1 page");
  assert.equal(H.pagesLabel(4000), "≈ 1 page");
  assert.equal(H.pagesLabel(8000), "≈ 3 pages");
  assert.equal(H.pagesLabel(100000), "≈ 35 pages");
  assert.equal(H.pagesLabel(600), "≈ 0.2 pages");
  assert.equal(H.pagesLabel(0), "no limit");
});

test("languages resolve by code or name and filter by any of them", () => {
  assert.deepEqual(plain(H.languageInfo("es")), { code: "es", name: "Español", english: "Spanish", known: true });
  assert.equal(H.languageInfo("Japanese").code, "ja");
  assert.deepEqual(plain(H.languageInfo("tlh")), { code: "tlh", name: "tlh", english: "tlh", known: false });
  assert.equal(H.languageMatches("fr")[0].code, "fr");
  assert.equal(H.languageMatches("germ")[0].code, "de");
  assert.equal(H.languageMatches("日本")[0].code, "ja");
  assert.equal(H.languageMatches("zzz").length, 0);
  assert.ok(H.languageMatches("").length >= 30);
});

test("leaving the language field keeps a listed or plausible custom language", () => {
  assert.equal(H.languageBlurValue("French"), "fr");
  assert.equal(H.languageBlurValue("deutsch"), "de");
  assert.equal(H.languageBlurValue("ja"), "ja");
  assert.equal(H.languageBlurValue("tlh"), "tlh");
  assert.equal(H.languageBlurValue("zh-Hant"), "zh-Hant");
  assert.equal(H.languageBlurValue("Klingon"), "Klingon");
  assert.equal(H.languageBlurValue("Fren"), null, "half of a listed name reverts");
  assert.equal(H.languageBlurValue("x"), null);
  assert.equal(H.languageBlurValue("  "), null);
  assert.equal(H.languageBlurValue("<script>"), null);
});

test("hotkeyKeycapList orders modifiers the macOS way", () => {
  assert.deepEqual(plain(H.hotkeyKeycapList("ctrl+shift+space")), ["⌃", "⇧", "Space"]);
  assert.deepEqual(plain(H.hotkeyKeycapList("shift+alt+p")), ["⌥", "⇧", "P"]);
  assert.deepEqual(plain(H.hotkeyKeycapList("cmd+ctrl+f5")), ["⌃", "⌘", "F5"]);
  assert.deepEqual(plain(H.hotkeyKeycapList("")), []);
});

test("excluded apps read as names, with monogram fallbacks", () => {
  assert.equal(H.excludedAppName("com.1password.1password"), "1Password");
  assert.equal(H.excludedAppName("com.example.app", { "com.example.app": "Example" }), "Example");
  assert.equal(H.excludedAppName("com.apple.*"), "com.apple.*");
  assert.equal(H.appMonogram("1Password"), "1P");
  assert.equal(H.appMonogram("Terminal"), "T");
  assert.equal(H.appMonogram("com.apple.*"), "✱");
  assert.equal(H.USAGE_KIND_FOR_CHOICE.openai, "openai_compatible");
  assert.equal(H.USAGE_KIND_FOR_CHOICE.codex, "chatgpt_codex");
});
