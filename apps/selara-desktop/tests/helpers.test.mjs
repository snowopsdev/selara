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
  "commandIdentity", "commandTileHtml", "hotkeyKeycaps", "wordDiff", "wordDiffHtml",
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

test("kindLabel always describes the replacement command behavior", () => {
  assert.equal(H.kindLabel("popup"), "Replace");
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

const CMD = { label: "Make Formal", prompt: "Rewrite the Text formally", hotkey: "ctrl+shift+f", kind: "replace" };

test("commandMatches matches on label, prompt, hotkey, and kind label", () => {
  assert.equal(H.commandMatches(CMD, "formal"), true);
  assert.equal(H.commandMatches(CMD, "rewrite"), true);
  assert.equal(H.commandMatches(CMD, "shift+f"), true);
  assert.equal(H.commandMatches(CMD, "replace"), true);
  assert.equal(H.commandMatches({ ...CMD, kind: "popup" }, "popup"), false);
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
  assert.equal(H.statusDotClass({ logged_in: false, message: "credentials not found" }), "bad");
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

test("commandIdentity uses the saved glyph and color", () => {
  const id = H.commandIdentity({ id: "concise", label: "Concise", glyph: "\u2702\ufe0e", color: "#FF375F" });
  assert.equal(id.glyph, "\u2702\ufe0e");
  assert.equal(id.color, "#ff375f");
  assert.equal(id.monogram, false);
  assert.equal(id.derivedColor, false);
});

test("commandIdentity derives a monogram and a stable palette color", () => {
  const a = H.commandIdentity({ id: "key_points", label: "key points" });
  const again = H.commandIdentity({ id: "key_points", label: "Renamed" });
  assert.equal(a.glyph, "K");
  assert.equal(a.monogram, true);
  assert.equal(a.derivedColor, true);
  assert.match(a.color, /^#[0-9a-f]{6}$/);
  assert.equal(again.color, a.color, "the color follows the id, not the label");
  assert.equal(H.commandIdentity({ id: "x", label: "Fix", color: "red" }).derivedColor, true, "invalid colors fall back");
  assert.equal(H.commandIdentity({}).glyph, "?");
  const colors = new Set(["proofread", "rewrite", "friendly", "professional", "concise", "summary", "key_points", "table", "translate"].map((id) => H.commandIdentity({ id, label: id }).color));
  assert.ok(colors.size >= 5, "built-in commands spread across the palette");
});

test("commandTileHtml and hotkeyKeycaps escape and split their input", () => {
  assert.match(H.commandTileHtml({ id: "a", label: "<b>" }, "lg"), /class="glyph-tile lg" style="--tile:#[0-9a-f]{6}"[^>]*>&lt;<\/span>/);
  assert.equal(H.hotkeyKeycaps("ctrl+shift+space"), '<span class="kcs"><span class="kc">\u2303</span><span class="kc">\u21e7</span><span class="kc">Space</span></span>');
  assert.equal(H.hotkeyKeycaps(""), "");
});

// The helpers run in another realm; compare their arrays as plain data.
const plain = (value) => JSON.parse(JSON.stringify(value));

test("wordDiff groups deletions before insertions around shared words", () => {
  assert.deepEqual(plain(H.wordDiff("hey can u send the numbers asap", "Could you send the numbers when you have a moment?")), [
    { type: "del", text: "hey can u" },
    { type: "ins", text: "Could you" },
    { type: "same", text: "send the numbers" },
    { type: "del", text: "asap" },
    { type: "ins", text: "when you have a moment?" },
  ]);
  assert.deepEqual(plain(H.wordDiff("same words", "same words")), [{ type: "same", text: "same words" }]);
  assert.deepEqual(plain(H.wordDiff("", "new")), [{ type: "ins", text: "new" }]);
});

test("wordDiffHtml marks changes and falls back to the result for rewrites", () => {
  assert.equal(H.wordDiffHtml("a <b> c", "a <i> c"), "a <del>&lt;b&gt;</del> <ins>&lt;i&gt;</ins> c");
  assert.equal(H.wordDiffHtml("Merci beaucoup pour votre patience.", "Thank you very much for your patience."), "Thank you very much for your patience.");
  assert.equal(H.wordDiffHtml("", "x"), "x");
});
