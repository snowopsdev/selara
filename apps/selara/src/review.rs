//! Review-before-replace (Ghost Diff) logic that does not touch AppKit: the
//! word diff shown in the card, its JSON payload for Swift, and the decision
//! table for the card's keys. Pure so it is tested on every platform.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

/// Larger inputs skip the O(n·m) table and show one deletion plus one insertion.
const MAX_DIFF_CELLS: usize = 4_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffOp {
    Equal,
    Delete,
    Insert,
}

impl DiffOp {
    fn name(self) -> &'static str {
        match self {
            Self::Equal => "equal",
            Self::Delete => "delete",
            Self::Insert => "insert",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffSegment {
    pub op: DiffOp,
    pub text: String,
}

fn seg(op: DiffOp, text: &str) -> DiffSegment {
    DiffSegment {
        op,
        text: text.to_string(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TokenClass {
    Word,
    Space,
    Punct,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn class_of(c: char) -> TokenClass {
    if is_word_char(c) {
        TokenClass::Word
    } else if c.is_whitespace() {
        TokenClass::Space
    } else {
        TokenClass::Punct
    }
}

/// Split into words, whitespace runs, and single punctuation characters.
/// An apostrophe between letters stays inside its word (`don't`, `it’s`).
pub fn tokenize(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (start, c) = chars[i];
        let class = class_of(c);
        let mut j = i + 1;
        match class {
            TokenClass::Word => {
                while j < chars.len() {
                    let next = chars[j].1;
                    let joins_word = matches!(next, '\'' | '’')
                        && chars
                            .get(j + 1)
                            .is_some_and(|(_, after)| is_word_char(*after));
                    if is_word_char(next) || joins_word {
                        j += 1;
                    } else {
                        break;
                    }
                }
            }
            TokenClass::Space => {
                while j < chars.len() && chars[j].1.is_whitespace() {
                    j += 1;
                }
            }
            TokenClass::Punct => {}
        }
        let end = chars.get(j).map_or(text.len(), |(index, _)| *index);
        tokens.push(&text[start..end]);
        i = j;
    }
    tokens
}

/// Longest-common-subsequence diff over tokens. Deletions are emitted before
/// insertions so a replaced phrase reads “old, then new”.
fn token_diff<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<(DiffOp, &'a str)> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let a = &old[prefix..old.len() - suffix];
    let b = &new[prefix..new.len() - suffix];
    let mut ops: Vec<(DiffOp, &str)> = old[..prefix].iter().map(|t| (DiffOp::Equal, *t)).collect();
    let (n, m) = (a.len(), b.len());
    if (n + 1).saturating_mul(m + 1) > MAX_DIFF_CELLS {
        ops.extend(a.iter().map(|t| (DiffOp::Delete, *t)));
        ops.extend(b.iter().map(|t| (DiffOp::Insert, *t)));
    } else {
        // table[i][j] = LCS length of a[i..] and b[j..].
        let width = m + 1;
        let mut table = vec![0u32; (n + 1) * width];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                table[i * width + j] = if a[i] == b[j] {
                    table[(i + 1) * width + j + 1] + 1
                } else {
                    table[(i + 1) * width + j].max(table[i * width + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && a[i] == b[j] {
                ops.push((DiffOp::Equal, a[i]));
                i += 1;
                j += 1;
            } else if j == m || (i < n && table[(i + 1) * width + j] >= table[i * width + j + 1]) {
                ops.push((DiffOp::Delete, a[i]));
                i += 1;
            } else {
                ops.push((DiffOp::Insert, b[j]));
                j += 1;
            }
        }
    }
    ops.extend(
        old[old.len() - suffix..]
            .iter()
            .map(|t| (DiffOp::Equal, *t)),
    );
    ops
}

/// Merge each run of changes into one deletion followed by one insertion,
/// and adjacent equal text into one segment.
fn normalize(segments: Vec<DiffSegment>) -> Vec<DiffSegment> {
    let mut out: Vec<DiffSegment> = Vec::new();
    let mut deleted = String::new();
    let mut inserted = String::new();
    let flush = |out: &mut Vec<DiffSegment>, deleted: &mut String, inserted: &mut String| {
        if !deleted.is_empty() {
            out.push(seg(DiffOp::Delete, &std::mem::take(deleted)));
        }
        if !inserted.is_empty() {
            out.push(seg(DiffOp::Insert, &std::mem::take(inserted)));
        }
    };
    for segment in segments {
        match segment.op {
            DiffOp::Delete => deleted.push_str(&segment.text),
            DiffOp::Insert => inserted.push_str(&segment.text),
            DiffOp::Equal => {
                if segment.text.is_empty() {
                    continue;
                }
                flush(&mut out, &mut deleted, &mut inserted);
                match out.last_mut() {
                    Some(last) if last.op == DiffOp::Equal => last.text.push_str(&segment.text),
                    _ => out.push(segment),
                }
            }
        }
    }
    flush(&mut out, &mut deleted, &mut inserted);
    out
}

/// Size of the change run around an equality: the longer of its deleted and
/// inserted text, in characters.
fn change_weight(segments: &[DiffSegment]) -> usize {
    let (mut deleted, mut inserted) = (0, 0);
    for segment in segments {
        match segment.op {
            DiffOp::Delete => deleted += segment.text.chars().count(),
            DiffOp::Insert => inserted += segment.text.chars().count(),
            DiffOp::Equal => {}
        }
    }
    deleted.max(inserted)
}

/// A token-level LCS happily keeps a lone “you” or a space between two
/// rewritten phrases, which reads as confetti. Fold such small equalities
/// into the surrounding change: whitespace or punctuation always, a single
/// word when it is at most half as long as the larger neighbouring change.
fn fold_small_equalities(mut segments: Vec<DiffSegment>) -> Vec<DiffSegment> {
    loop {
        segments = normalize(segments);
        let mut folded = false;
        for index in 1..segments.len().saturating_sub(1) {
            if segments[index].op != DiffOp::Equal {
                continue;
            }
            let before_start = segments[..index]
                .iter()
                .rposition(|s| s.op == DiffOp::Equal)
                .map_or(0, |p| p + 1);
            let after_end = segments[index + 1..]
                .iter()
                .position(|s| s.op == DiffOp::Equal)
                .map_or(segments.len(), |p| index + 1 + p);
            let before = change_weight(&segments[before_start..index]);
            let after = change_weight(&segments[index + 1..after_end]);
            if before == 0 || after == 0 {
                continue;
            }
            let text = &segments[index].text;
            let words: Vec<&str> = tokenize(text)
                .into_iter()
                .filter(|t| t.chars().next().is_some_and(is_word_char))
                .collect();
            let lone_word = words.len() == 1 && words[0].chars().count() * 2 <= before.max(after);
            // Line breaks stay put so paragraphs keep their own changes.
            if !text.contains('\n') && (words.is_empty() || lone_word) {
                let text = std::mem::take(&mut segments[index].text);
                segments.splice(
                    index..=index,
                    [seg(DiffOp::Delete, &text), seg(DiffOp::Insert, &text)],
                );
                folded = true;
                break;
            }
        }
        if !folded {
            return segments;
        }
    }
}

/// Word-level diff of `old` → `new`. Concatenating the equal and deleted
/// segments yields `old`; the equal and inserted segments yield `new`.
pub fn word_diff(old: &str, new: &str) -> Vec<DiffSegment> {
    let old_tokens = tokenize(old);
    let new_tokens = tokenize(new);
    let segments = token_diff(&old_tokens, &new_tokens)
        .into_iter()
        .map(|(op, text)| seg(op, text))
        .collect();
    fold_small_equalities(segments)
}

/// JSON handed to the native card: the segments, the net character change,
/// and the full result (used for ⌘C so Swift never reconstructs it).
pub fn diff_payload(original: &str, result: &str) -> String {
    let segments = word_diff(original, result)
        .into_iter()
        .map(|s| serde_json::json!({ "op": s.op.name(), "text": s.text }))
        .collect::<Vec<_>>();
    let delta = result.chars().count() as i64 - original.chars().count() as i64;
    serde_json::json!({ "segments": segments, "delta": delta, "result": result }).to_string()
}

/// Keys and gestures the review card reports. The codes are the C ABI
/// values returned by `selara_progress_take_review_action`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewAction {
    /// ↩ or the Replace button.
    Accept,
    /// ⇥: run the same command again.
    AnotherTake,
    /// ⌘C: Swift has already put the result on the pasteboard.
    Copy,
    /// Esc or the Discard button.
    Discard,
    /// The card lost key focus because the user clicked or switched away.
    Dismissed,
}

impl ReviewAction {
    pub fn from_code(code: i32) -> Option<Self> {
        Some(match code {
            1 => Self::Accept,
            2 => Self::AnotherTake,
            3 => Self::Copy,
            4 => Self::Discard,
            5 => Self::Dismissed,
            _ => return None,
        })
    }
}

/// What serve does with a review action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewStep {
    /// Return focus to the source and run the verified replacement.
    Replace(String),
    /// Start a new generation of the same command; the card shows a skeleton.
    Rerun,
    /// Close the card without replacing. Any ready result is kept in History
    /// as not applied; `copied` keeps the “Copied” receipt on screen.
    Close { copied: bool },
    /// Not meaningful in this state (e.g. ↩ before the result arrives).
    Ignore,
}

/// Decision table for the card. `result` is `None` while the model is still
/// working (skeleton), `Some` once the diff is on screen.
pub fn review_step(result: Option<&str>, action: ReviewAction) -> ReviewStep {
    match (result, action) {
        (_, ReviewAction::Discard | ReviewAction::Dismissed) => ReviewStep::Close { copied: false },
        (None, _) => ReviewStep::Ignore,
        (Some(text), ReviewAction::Accept) => ReviewStep::Replace(text.to_string()),
        (Some(_), ReviewAction::AnotherTake) => ReviewStep::Rerun,
        (Some(_), ReviewAction::Copy) => ReviewStep::Close { copied: true },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rebuild(segments: &[DiffSegment], keep: DiffOp) -> String {
        segments
            .iter()
            .filter(|s| s.op == DiffOp::Equal || s.op == keep)
            .map(|s| s.text.as_str())
            .collect()
    }

    fn render(segments: &[DiffSegment]) -> String {
        segments
            .iter()
            .map(|s| match s.op {
                DiffOp::Equal => s.text.clone(),
                DiffOp::Delete => format!("[-{}-]", s.text),
                DiffOp::Insert => format!("{{+{}+}}", s.text),
            })
            .collect()
    }

    #[test]
    fn tokenize_splits_words_spaces_and_punctuation() {
        assert_eq!(
            tokenize("Hi, Dana!  It's  ok…"),
            vec!["Hi", ",", " ", "Dana", "!", "  ", "It's", "  ", "ok", "…"]
        );
        assert_eq!(tokenize("don’t stop"), vec!["don’t", " ", "stop"]);
        assert_eq!(tokenize("'quoted'"), vec!["'", "quoted", "'"]);
        assert_eq!(tokenize("a\n\nb"), vec!["a", "\n\n", "b"]);
        assert!(tokenize("").is_empty());
        assert_eq!(tokenize("日本語 テキスト"), vec!["日本語", " ", "テキスト"]);
    }

    #[test]
    fn identical_text_is_one_equal_segment() {
        assert_eq!(
            word_diff("Same text.", "Same text."),
            vec![seg(DiffOp::Equal, "Same text.")]
        );
        assert!(word_diff("", "").is_empty());
    }

    #[test]
    fn pure_insertions_and_deletions() {
        assert_eq!(render(&word_diff("", "New")), "{+New+}");
        assert_eq!(render(&word_diff("Old", "")), "[-Old-]");
        assert_eq!(
            render(&word_diff(
                "Send the report today.",
                "Send the final report today."
            )),
            "Send the {+final +}report today."
        );
        assert_eq!(
            render(&word_diff(
                "Send the final report today.",
                "Send the report today."
            )),
            "Send the [-final -]report today."
        );
    }

    #[test]
    fn replaced_phrase_reads_old_then_new() {
        assert_eq!(
            render(&word_diff("We will begin soon.", "We start soon.")),
            "We [-will begin-]{+start+} soon."
        );
        assert_eq!(
            render(&word_diff("Is it ok.", "Is it ok?")),
            "Is it ok[-.-]{+?+}"
        );
    }

    #[test]
    fn reference_concise_example_matches_the_design() {
        let old = "I just wanted to quickly follow up and check in on whether you had a chance to take a look at the document I sent over last week.";
        let new = "Did you get a chance to review the document I sent last week?";
        let diff = word_diff(old, new);
        assert_eq!(rebuild(&diff, DiffOp::Delete), old);
        assert_eq!(rebuild(&diff, DiffOp::Insert), new);
        assert_eq!(
            render(&diff),
            "[-I just wanted to quickly follow up and check in on whether you had-]{+Did you get+} a chance to [-take a look at-]{+review+} the document I sent [-over -]last week[-.-]{+?+}"
        );
    }

    #[test]
    fn diff_always_reconstructs_both_sides() {
        let cases = [
            ("The quick brown fox.", "A quick red fox jumps."),
            ("one two three", "three two one"),
            ("Line one.\nLine two.", "Line one!\n\nLine 2."),
            ("Ünïcödé — test", "Unicode - test"),
            ("a b c d e f", "a x c y e z"),
        ];
        for (old, new) in cases {
            let diff = word_diff(old, new);
            assert_eq!(rebuild(&diff, DiffOp::Delete), old, "{old:?} → {new:?}");
            assert_eq!(rebuild(&diff, DiffOp::Insert), new, "{old:?} → {new:?}");
            for pair in diff.windows(2) {
                assert!(
                    !(pair[0].op == pair[1].op),
                    "adjacent segments must be merged: {diff:?}"
                );
                assert!(
                    !(pair[0].op == DiffOp::Insert && pair[1].op == DiffOp::Delete),
                    "deletions come before insertions: {diff:?}"
                );
            }
        }
    }

    #[test]
    fn paragraph_breaks_keep_changes_apart() {
        assert_eq!(
            render(&word_diff(
                "Old start.\n\nOld end.",
                "New start.\n\nNew end."
            )),
            "[-Old-]{+New+} start.\n\n[-Old-]{+New+} end."
        );
        assert_eq!(
            render(&word_diff("alpha beta.\ngamma", "one two.\nthree")),
            "[-alpha beta-]{+one two+}.\n[-gamma-]{+three+}"
        );
    }

    #[test]
    fn huge_inputs_fall_back_to_one_replacement() {
        let old = "word ".repeat(2100);
        let new = "term ".repeat(2100);
        let diff = word_diff(&old, &new);
        assert_eq!(rebuild(&diff, DiffOp::Delete), old);
        assert_eq!(rebuild(&diff, DiffOp::Insert), new);
    }

    #[test]
    fn payload_carries_segments_delta_and_result() {
        let payload: serde_json::Value =
            serde_json::from_str(&diff_payload("Hello world.", "Hi world!")).unwrap();
        assert_eq!(payload["delta"], -3);
        assert_eq!(payload["result"], "Hi world!");
        let ops: Vec<_> = payload["segments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["op"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ops, ["delete", "insert", "equal", "delete", "insert"]);
        let longer: serde_json::Value =
            serde_json::from_str(&diff_payload("Hi", "Hi there")).unwrap();
        assert_eq!(longer["delta"], 6);
    }

    #[test]
    fn review_action_codes_round_trip() {
        for (code, action) in [
            (1, ReviewAction::Accept),
            (2, ReviewAction::AnotherTake),
            (3, ReviewAction::Copy),
            (4, ReviewAction::Discard),
            (5, ReviewAction::Dismissed),
        ] {
            assert_eq!(ReviewAction::from_code(code), Some(action));
        }
        assert_eq!(ReviewAction::from_code(0), None);
        assert_eq!(ReviewAction::from_code(9), None);
    }

    #[test]
    fn review_waits_for_the_result_before_accepting() {
        for action in [
            ReviewAction::Accept,
            ReviewAction::AnotherTake,
            ReviewAction::Copy,
        ] {
            assert_eq!(review_step(None, action), ReviewStep::Ignore);
        }
        assert_eq!(
            review_step(None, ReviewAction::Discard),
            ReviewStep::Close { copied: false }
        );
        assert_eq!(
            review_step(None, ReviewAction::Dismissed),
            ReviewStep::Close { copied: false }
        );
    }

    #[test]
    fn review_with_a_result_maps_each_key() {
        let result = Some("Did you get a chance?");
        assert_eq!(
            review_step(result, ReviewAction::Accept),
            ReviewStep::Replace("Did you get a chance?".into())
        );
        assert_eq!(
            review_step(result, ReviewAction::AnotherTake),
            ReviewStep::Rerun
        );
        assert_eq!(
            review_step(result, ReviewAction::Copy),
            ReviewStep::Close { copied: true }
        );
        assert_eq!(
            review_step(result, ReviewAction::Discard),
            ReviewStep::Close { copied: false }
        );
        assert_eq!(
            review_step(result, ReviewAction::Dismissed),
            ReviewStep::Close { copied: false }
        );
    }
}
