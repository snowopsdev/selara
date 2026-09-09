// Tests for the pure helper functions in ../index.html.
//
// The Settings UI is a single inline <script>; only dist/index.html ships, so
// the helpers cannot live in a separate module. Instead they sit in one block
// delimited by `// @selara-helpers-start` / `// @selara-helpers-end`, which
// this file slices out of index.html and evaluates in a bare `node:vm` context.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const START = "// @selara-helpers-start";
const END = "// @selara-helpers-end";
const NAMES = [
  "escapeHtml", "escapeAttr", "num", "slug", "kindLabel",
  "maskEmail", "prettyHotkey", "commandMatches", "statusDotClass",
  "relativeTime", "previewLine",
];

function loadHelpers() {
  const htmlPath = fileURLToPath(new URL("../index.html", import.meta.url));
  const html = readFileSync(htmlPath, "utf8");
  const start = html.indexOf(START);
  const end = html.indexOf(END);
  if (start === -1) throw new Error(`${START} marker missing from index.html`);
  if (end === -1) throw new Error(`${END} marker missing from index.html`);
  if (end < start) throw new Error("helper markers are out of order in index.html");
  const body = html.slice(start + START.length, end);
  // Skip the rest of the marker comment line (the "(pure functions; ...)" note).
  const code = body.slice(body.indexOf("\n") + 1);
  return vm.runInNewContext(`${code}\n({${NAMES.join(", ")}})`, {}, { filename: "index.html#helpers" });
}

const H = loadHelpers();

test("marker block exposes the expected helpers", () => {
  for (const name of NAMES) assert.equal(typeof H[name], "function", `${name} should be a function`);
});

test("escapeHtml escapes & < > and leaves quotes alone", () => {
  assert.equal(H.escapeHtml("<a href=\"x\">&'</a>"), "&lt;a href=\"x\"&gt;&amp;'&lt;/a&gt;");
  assert.equal(H.escapeHtml("plain"), "plain");
  assert.equal(H.escapeHtml("&&"), "&amp;&amp;");
});

test("escapeHtml treats null/undefined as empty string", () => {
  assert.equal(H.escapeHtml(null), "");
  assert.equal(H.escapeHtml(undefined), "");
  assert.equal(H.escapeHtml(0), "0");
});

test("escapeAttr also escapes double quotes", () => {
  assert.equal(H.escapeAttr("say \"hi\" & <bye>"), "say &quot;hi&quot; &amp; &lt;bye&gt;");
  assert.equal(H.escapeAttr("it's"), "it's");
  assert.equal(H.escapeAttr(null), "");
  assert.equal(H.escapeAttr(undefined), "");
});

test("num floors non-negative numbers and clamps everything else to 0", () => {
  assert.equal(H.num("12.9"), 12);
  assert.equal(H.num(7), 7);
  assert.equal(H.num("0"), 0);
  assert.equal(H.num("-3"), 0);
  assert.equal(H.num("abc"), 0);
  assert.equal(H.num(""), 0);
  assert.equal(H.num(Infinity), 0);
  assert.equal(H.num(null), 0);
});

test("slug lowercases, dashes non-alphanumerics, trims dashes, and falls back to 'command'", () => {
  assert.equal(H.slug("Make it Formal!"), "make-it-formal");
  assert.equal(H.slug("  Fix   Grammar  "), "fix-grammar");
  assert.equal(H.slug("Summarize_2x"), "summarize-2x");
  assert.equal(H.slug("!!!"), "command");
  assert.equal(H.slug(""), "command");
});

test("kindLabel maps popup to Popup and everything else to Replace", () => {
  assert.equal(H.kindLabel("popup"), "Popup");
  assert.equal(H.kindLabel("replace"), "Replace");
  assert.equal(H.kindLabel(undefined), "Replace");
  assert.equal(H.kindLabel("Popup"), "Replace");
});

test("maskEmail masks the local part and most of the domain", () => {
  // domain "example.com": last dot at index 7 -> max(4, min(7, 6)) = 6 bullets.
  assert.equal(H.maskEmail("user@example.com"), "••••••••@••••••.com");
  // domain "io.dev": last dot at index 2 -> max(4, min(2, 6)) = 4 bullets.
  assert.equal(H.maskEmail("a@io.dev"), "••••••••@••••.dev");
  // Uses the LAST dot: "mail.example.co.uk" -> dot index 15 -> 6 bullets + ".uk".
  assert.equal(H.maskEmail("x@mail.example.co.uk"), "••••••••@••••••.uk");
});

test("maskEmail handles edge cases", () => {
  assert.equal(H.maskEmail(""), "");
  assert.equal(H.maskEmail(null), "");
  assert.equal(H.maskEmail(undefined), "");
  assert.equal(H.maskEmail("nodomain"), "••••••••");
  assert.equal(H.maskEmail("@leading-at.com"), "••••••••");
  assert.ok(H.maskEmail("user@localhost").endsWith("@••••"));
  assert.equal(H.maskEmail("user@localhost"), "••••••••@••••");
});

test("prettyHotkey renders modifier symbols and uppercases single keys", () => {
  assert.equal(H.prettyHotkey("ctrl+shift+p"), "⌃⇧P");
  assert.equal(H.prettyHotkey("control+alt+x"), "⌃⌥X");
  assert.equal(H.prettyHotkey("command+super+meta"), "⌘⌘⌘");
  assert.equal(H.prettyHotkey("CMD+K"), "⌘K");
});

test("prettyHotkey maps space and passes multi-char keys through trimmed", () => {
  assert.equal(H.prettyHotkey("option+space"), "⌥Space");
  assert.equal(H.prettyHotkey("cmd+enter"), "⌘enter");
  assert.equal(H.prettyHotkey(" cmd + Enter "), "⌘Enter");
});

test("prettyHotkey handles empty and nullish chords", () => {
  assert.equal(H.prettyHotkey(""), "");
  assert.equal(H.prettyHotkey(null), "");
  assert.equal(H.prettyHotkey(undefined), "");
});

const CMD = { label: "Make Formal", prompt: "Rewrite the Text formally", hotkey: "ctrl+shift+f", kind: "popup" };

test("commandMatches matches on label, prompt, hotkey, and kind label", () => {
  assert.equal(H.commandMatches(CMD, "formal"), true);
  assert.equal(H.commandMatches(CMD, "rewrite"), true);
  assert.equal(H.commandMatches(CMD, "shift+f"), true);
  assert.equal(H.commandMatches(CMD, "popup"), true);
  assert.equal(H.commandMatches({ ...CMD, kind: "replace" }, "replace"), true);
  assert.equal(H.commandMatches(CMD, "nomatch"), false);
});

test("commandMatches lowercases the haystack (callers pass a lowercased query)", () => {
  assert.equal(H.commandMatches(CMD, "make formal"), true);
  assert.equal(H.commandMatches(CMD, "text"), true);
  assert.equal(H.commandMatches(CMD, "MAKE"), false, "query is expected to be pre-lowercased by the caller");
});

test("commandMatches: empty query matches everything and missing hotkey is tolerated", () => {
  assert.equal(H.commandMatches(CMD, ""), true);
  assert.equal(H.commandMatches({ label: "", prompt: "", kind: "replace" }, ""), true);
  assert.equal(H.commandMatches({ label: "X", prompt: "Y", kind: "replace" }, "x"), true);
  assert.equal(H.commandMatches({ label: "X", prompt: "Y", kind: "replace" }, "ctrl"), false);
});

test("statusDotClass distinguishes unknown, ok, and bad", () => {
  assert.equal(H.statusDotClass(null), "unknown");
  assert.equal(H.statusDotClass(undefined), "unknown");
  assert.equal(H.statusDotClass({ logged_in: true }), "ok");
  assert.equal(H.statusDotClass({ logged_in: true, message: "error" }), "ok");
  assert.equal(H.statusDotClass({ logged_in: false, message: "ENOENT" }), "bad");
  assert.equal(H.statusDotClass({ logged_in: false, message: "Login failed" }), "bad");
  assert.equal(H.statusDotClass({ logged_in: false, message: "auth.json not found" }), "bad");
  assert.equal(H.statusDotClass({ logged_in: false, message: "Some Error" }), "bad");
  assert.equal(H.statusDotClass({ logged_in: false, message: "Not logged in" }), "unknown");
  assert.equal(H.statusDotClass({ logged_in: false }), "unknown");
  assert.equal(H.statusDotClass({ logged_in: false, message: "" }), "unknown");
});

test("relativeTime buckets seconds, minutes, hours, and days", () => {
  const now = 1_700_000_000;
  assert.equal(H.relativeTime(now - 10, now), "just now");
  assert.equal(H.relativeTime(now - 180, now), "3 min ago");
  assert.equal(H.relativeTime(now - 2 * 3600, now), "2 h ago");
  assert.equal(H.relativeTime(now - 5 * 86400, now), "5 d ago");
  assert.equal(H.relativeTime(now + 60, now), "just now", "future timestamps clamp to now");
  assert.equal(H.relativeTime(0, now), "unknown time");
  assert.equal(H.relativeTime("nope", now), "unknown time");
});

test("previewLine collapses whitespace and truncates with an ellipsis", () => {
  assert.equal(H.previewLine("  a\n\n  b\tc  ", 100), "a b c");
  assert.equal(H.previewLine("", 10), "(empty)");
  assert.equal(H.previewLine(null, 10), "(empty)");
  assert.equal(H.previewLine("abcdefghij", 10), "abcdefghij");
  assert.equal(H.previewLine("abcdefghijk", 10), "abcdefghi\u2026");
  assert.equal(H.previewLine("abcd efghijk", 10), "abcd efgh\u2026");
});
