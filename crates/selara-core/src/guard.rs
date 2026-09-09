//! Secret-shaped text detection, so `serve` can ask before a selection that
//! looks like an API key, a private key, a JWT, or a card number leaves the
//! machine for a hosted provider. Std-only heuristics: cheap, no regex crate,
//! and tuned to warn rather than to be exhaustive.

use std::fmt;

use serde::Serialize;

use crate::providers::ProviderKind;

/// What a hit looked like. The user sees this name in the picker banner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretKind {
    ApiKey,
    PrivateKey,
    CardNumber,
    Jwt,
}

impl SecretKind {
    /// Name with its indefinite article, for prose ("an API key").
    pub fn with_article(self) -> &'static str {
        match self {
            SecretKind::ApiKey => "an API key",
            SecretKind::PrivateKey => "a private key",
            SecretKind::CardNumber => "a card number",
            SecretKind::Jwt => "a JWT",
        }
    }
}

impl fmt::Display for SecretKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SecretKind::ApiKey => "API key",
            SecretKind::PrivateKey => "private key",
            SecretKind::CardNumber => "card number",
            SecretKind::Jwt => "JWT",
        })
    }
}

/// One detection. `preview` is the first few characters plus an ellipsis and
/// never the whole value, so it is safe to show in the UI and in logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecretHit {
    pub kind: SecretKind,
    pub preview: String,
}

const PREVIEW_CHARS: usize = 6;

fn preview_of(value: &str) -> String {
    let mut p: String = value.chars().take(PREVIEW_CHARS).collect();
    p.push('…');
    p
}

/// Token prefixes that mark well-known API key formats. `AKIA` and Slack
/// tokens have extra shape checks in [`looks_like_api_key`].
const KEY_PREFIXES: [&str; 8] = [
    "sk-ant-",
    "sk-or-",
    "sk-",
    "gsk_",
    "ghp_",
    "github_pat_",
    "AIza",
    "AKIA",
];

const MIN_KEY_LEN: usize = 20;

fn is_token_separator(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\'' | '`' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | ';' | '='
        )
}

/// Split on whitespace, quotes, brackets, and `=`/`,`/`;`, then trim trailing
/// sentence punctuation so `key: sk-abc...` and `(sk-abc...)` both surface.
fn tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(is_token_separator)
        .map(|t| t.trim_end_matches(['.', ':', '!', '?']))
        .filter(|t| !t.is_empty())
}

fn looks_like_api_key(token: &str) -> bool {
    if token.chars().count() < MIN_KEY_LEN {
        return false;
    }
    if let Some(rest) = token.strip_prefix("AKIA") {
        return rest.len() == 16
            && rest
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
    }
    if let Some(rest) = token.strip_prefix("xox") {
        let mut chars = rest.chars();
        return matches!(chars.next(), Some('a' | 'b' | 'p' | 'r')) && chars.next() == Some('-');
    }
    KEY_PREFIXES
        .iter()
        .any(|p| *p != "AKIA" && token.starts_with(p))
}

fn is_base64url(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn looks_like_jwt(token: &str) -> bool {
    if !token.starts_with("eyJ") {
        return false;
    }
    let mut parts = token.split('.');
    let (Some(h), Some(p), Some(s), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    is_base64url(h) && is_base64url(p) && is_base64url(s)
}

fn luhn_ok(digits: &[u8]) -> bool {
    let mut sum = 0u32;
    for (i, d) in digits.iter().rev().enumerate() {
        let mut v = u32::from(*d);
        if i % 2 == 1 {
            v *= 2;
            if v > 9 {
                v -= 9;
            }
        }
        sum += v;
    }
    sum.is_multiple_of(10)
}

/// Runs of 13–19 digits, optionally grouped by single spaces or dashes, that
/// pass the Luhn check. Longer runs are not cards and are skipped whole, so a
/// hash or a long id does not get sliced into false positives.
fn find_card_numbers(text: &str, out: &mut Vec<SecretHit>) {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        let mut digits: Vec<u8> = Vec::new();
        while i < chars.len() {
            let c = chars[i];
            if c.is_ascii_digit() {
                digits.push(c as u8 - b'0');
                i += 1;
            } else if matches!(c, ' ' | '-') && i + 1 < chars.len() && chars[i + 1].is_ascii_digit()
            {
                i += 1;
            } else {
                break;
            }
        }
        if (13..=19).contains(&digits.len()) && luhn_ok(&digits) {
            let run: String = chars[start..i].iter().collect();
            out.push(SecretHit {
                kind: SecretKind::CardNumber,
                preview: preview_of(&run),
            });
        }
    }
}

fn find_private_keys(text: &str, out: &mut Vec<SecretHit>) {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("-----BEGIN ") {
            if rest.contains("PRIVATE KEY-----") {
                out.push(SecretHit {
                    kind: SecretKind::PrivateKey,
                    preview: preview_of(line),
                });
            }
        }
    }
}

/// Scan `text` for secret-shaped values. Returns one hit per distinct value,
/// in order of appearance by kind (keys and JWTs, then private keys, then
/// card numbers). An empty result means nothing looked like a secret.
pub fn scan_secrets(text: &str) -> Vec<SecretHit> {
    let mut hits: Vec<SecretHit> = Vec::new();
    for token in tokens(text) {
        if looks_like_api_key(token) {
            hits.push(SecretHit {
                kind: SecretKind::ApiKey,
                preview: preview_of(token),
            });
        } else if looks_like_jwt(token) {
            hits.push(SecretHit {
                kind: SecretKind::Jwt,
                preview: preview_of(token),
            });
        }
    }
    find_private_keys(text, &mut hits);
    find_card_numbers(text, &mut hits);
    let mut seen: Vec<SecretHit> = Vec::with_capacity(hits.len());
    for h in hits {
        if !seen.contains(&h) {
            seen.push(h);
        }
    }
    seen
}

/// Host part of a URL, lowercased, without scheme, userinfo, port, or path.
/// `http://user@[::1]:8080/v1` -> `::1`; `localhost:11434` -> `localhost`.
fn url_host(url: &str) -> String {
    let no_scheme = match url.find("://") {
        Some(idx) => &url[idx + 3..],
        None => url,
    };
    let authority = no_scheme.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else if authority.matches(':').count() > 1 {
        // Bare IPv6 without brackets: no port to strip.
        authority
    } else {
        authority.split(':').next().unwrap_or("")
    };
    host.trim().to_ascii_lowercase()
}

fn is_private_v4(host: &str) -> bool {
    let mut octets = host.split('.');
    let (Some(a), Some(b), Some(_), Some(_), None) = (
        octets.next().and_then(|o| o.parse::<u8>().ok()),
        octets.next().and_then(|o| o.parse::<u8>().ok()),
        octets.next().and_then(|o| o.parse::<u8>().ok()),
        octets.next().and_then(|o| o.parse::<u8>().ok()),
        octets.next(),
    ) else {
        return false;
    };
    a == 10 || (a == 192 && b == 168) || (a == 172 && (16..=31).contains(&b))
}

/// Whether requests for this provider leave the machine. Loopback, `.local`
/// names, and private IPv4 ranges count as local; everything else (including
/// the provider defaults used when `base_url` is blank) is hosted.
pub fn provider_is_hosted(kind: ProviderKind, base_url: &str) -> bool {
    let host = url_host(&kind.resolve_base_url(base_url));
    if host.is_empty() {
        return true;
    }
    // Only an address that actually parses counts as loopback/private: a name
    // like `127.0.0.1.evil.com` starts with "127." but resolves to a public
    // host, and treating it as local would skip the warning entirely.
    let local = host == "localhost"
        || host.ends_with(".local")
        || host.ends_with(".localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified())
        || is_private_v4(&host);
    !local
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Append the Luhn check digit so a fixture is valid without committing a
    /// number that looks like a real card.
    fn luhn_complete(prefix: &str) -> String {
        for d in b'0'..=b'9' {
            let candidate = format!("{prefix}{}", d as char);
            let digits: Vec<u8> = candidate.bytes().map(|b| b - b'0').collect();
            if luhn_ok(&digits) {
                return candidate;
            }
        }
        unreachable!("a Luhn check digit always exists");
    }

    /// Same number with the last digit changed, so Luhn fails.
    fn bump_last_digit(number: &str) -> String {
        let mut out: Vec<char> = number.chars().collect();
        let last = out.len() - 1;
        let next = (out[last].to_digit(10).unwrap() + 1) % 10;
        out[last] = char::from_digit(next, 10).unwrap();
        out.into_iter().collect()
    }

    fn kinds(text: &str) -> Vec<SecretKind> {
        scan_secrets(text).into_iter().map(|h| h.kind).collect()
    }

    #[test]
    fn hosts_that_only_look_local_are_treated_as_hosted() {
        // Names that merely start with a loopback or private prefix resolve
        // through DNS to someone else's server.
        for host in [
            "http://127.0.0.1.evil.com/v1",
            "http://10.0.0.1.evil.com/v1",
            "http://192.168.1.1.attacker.test/v1",
            "http://localhost.evil.com/v1",
            "http://127-0-0-1.evil.com/v1",
        ] {
            assert!(
                provider_is_hosted(ProviderKind::OpenAiCompatible, host),
                "{host} must count as hosted"
            );
        }
        for host in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:1234/v1",
            "http://127.1.2.3:8000/v1",
            "http://[::1]:8080/v1",
            "http://0.0.0.0:9000/v1",
            "http://10.1.2.3/v1",
            "http://192.168.0.5/v1",
            "http://172.20.0.1/v1",
            "http://studio.local:1234/v1",
        ] {
            assert!(
                !provider_is_hosted(ProviderKind::OpenAiCompatible, host),
                "{host} must count as local"
            );
        }
    }

    #[test]
    fn plain_prose_is_clean() {
        let text = "Please proofread this paragraph. It mentions sk- in passing, \
                    a 12-digit number 123456789012, and a phone number 555-123-4567. \
                    Nothing here is a secret, and the year 2024 is not either.";
        assert!(scan_secrets(text).is_empty(), "{:?}", scan_secrets(text));
    }

    #[test]
    fn short_or_bare_prefixes_do_not_match() {
        assert!(kinds("sk-").is_empty());
        assert!(kinds("sk-short").is_empty());
        assert!(kinds("ghp_abc").is_empty());
        assert!(kinds("AKIA").is_empty());
        // AKIA needs exactly 16 uppercase/digit chars after it.
        assert!(kinds("AKIAaaaaaaaaaaaaaaaa").is_empty());
        assert!(kinds("AKIAABCDEFGHIJKLMNO").is_empty());
        assert!(kinds("AKIAABCDEFGHIJKLMNOPQ").is_empty());
        // xox needs one of a/b/p/r then a dash.
        assert!(kinds("xoxz-0000000000-aaaaaaaaaa").is_empty());
    }

    #[test]
    fn detects_common_api_key_shapes() {
        // Bodies are placeholder runs, not realistic values, and are joined at
        // runtime: the detectors only look at the prefix, length, and charset,
        // so this keeps coverage without a scanner-tripping literal in source.
        let cases: Vec<String> = [
            ("sk-", "proj-aaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            ("sk-ant-", "api03-aaaaaaaaaaaaaaaaaaaaaaaaaa"),
            ("sk-or-", "v1-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            ("gsk_", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            ("AKIA", "AAAAAAAAAAAAAAAA"),
            ("ghp_", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            ("github_pat_", "aaaaaaaaaaaaaaaaaaaa"),
            ("xoxb-", "0000-0000-aaaaaaaaaaaa"),
            ("xoxp-", "0000-0000-aaaaaaaaaaaa"),
            ("AIza", "aaaaaaaaaaaaaaaaaaaa"),
        ]
        .iter()
        .map(|(prefix, body)| format!("{prefix}{body}"))
        .collect();
        for key in &cases {
            let hits = scan_secrets(&format!("token: {key}"));
            assert_eq!(hits.len(), 1, "{key}: {hits:?}");
            assert_eq!(hits[0].kind, SecretKind::ApiKey, "{key}");
            let expected: String = key.chars().take(6).chain(std::iter::once('…')).collect();
            assert_eq!(hits[0].preview, expected, "{key}");
            assert!(!hits[0].preview.contains(&key[8..]), "preview leaks value");
        }
    }

    #[test]
    fn keys_are_found_inside_quotes_brackets_and_assignments() {
        assert_eq!(
            kinds("OPENAI_API_KEY=\"sk-aaaaaaaaaaaaaaaaaaaaaaaaaa\""),
            vec![SecretKind::ApiKey]
        );
        assert_eq!(
            kinds("(see [sk-aaaaaaaaaaaaaaaaaaaaaaaaaa], ok?)"),
            vec![SecretKind::ApiKey]
        );
        assert_eq!(
            kinds("Authorization: Bearer sk-aaaaaaaaaaaaaaaaaaaaaaaaaa."),
            vec![SecretKind::ApiKey]
        );
    }

    #[test]
    fn detects_private_key_blocks() {
        // find_private_keys only looks for the BEGIN/END markers, so the body
        // is a placeholder rather than anything resembling DER base64.
        let pem = format!(
            "some intro\n-----BEGIN RSA PRIVATE KEY-----\n{}\n-----END RSA PRIVATE KEY-----\n",
            "placeholder-body-not-a-key"
        );
        let pem = pem.as_str();
        let hits = scan_secrets(pem);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].kind, SecretKind::PrivateKey);
        assert_eq!(hits[0].preview, "-----B…");
        assert_eq!(
            kinds("-----BEGIN OPENSSH PRIVATE KEY-----"),
            vec![SecretKind::PrivateKey]
        );
        assert_eq!(
            kinds("-----BEGIN EC PRIVATE KEY-----"),
            vec![SecretKind::PrivateKey]
        );
        // Certificates and public keys are fine to send.
        assert!(kinds("-----BEGIN CERTIFICATE-----").is_empty());
        assert!(kinds("-----BEGIN PUBLIC KEY-----").is_empty());
    }

    #[test]
    fn detects_jwts() {
        // base64url of {"a":1} and {"b":2} plus a filler signature: the JWT
        // shape (three base64url segments, "eyJ" header) without a real token.
        let jwt = format!("eyJhIjoxfQ.eyJiIjoyfQ.{}", "a".repeat(43));
        let jwt = jwt.as_str();
        let hits = scan_secrets(&format!("cookie={jwt}; path=/"));
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].kind, SecretKind::Jwt);
        assert_eq!(hits[0].preview, "eyJhIj…");
        // Two segments, or a non-base64url segment, is not a JWT.
        assert!(kinds("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0").is_empty());
        assert!(kinds("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.sig+nature/with=pad").is_empty());
        assert!(kinds("abc.def.ghi").is_empty());
    }

    #[test]
    fn detects_card_numbers_that_pass_luhn() {
        // Built from a non-branded prefix so no real-looking card literal is
        // committed; `card` is Luhn-valid by construction.
        let card = luhn_complete("123456789012345");
        let spaced = format!(
            "{} {} {} {}",
            &card[0..4],
            &card[4..8],
            &card[8..12],
            &card[12..16]
        );
        let hits = scan_secrets(&format!("Card: {spaced} exp 12/29"));
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].kind, SecretKind::CardNumber);
        assert_eq!(hits[0].preview, format!("{}…", &spaced[0..6]));
        assert_eq!(
            kinds(&spaced.replace(' ', "-")),
            vec![SecretKind::CardNumber]
        );
        assert_eq!(kinds(&card), vec![SecretKind::CardNumber]);
        // 15- and 13-digit runs are cards too (Amex and old Visa lengths).
        assert_eq!(
            kinds(&luhn_complete("12345678901234")),
            vec![SecretKind::CardNumber]
        );
        assert_eq!(
            kinds(&luhn_complete("123456789012")),
            vec![SecretKind::CardNumber]
        );
    }

    #[test]
    fn digit_runs_that_are_not_cards_are_ignored() {
        let card = luhn_complete("123456789012345");
        let spaced = format!(
            "{} {} {} {}",
            &card[0..4],
            &card[4..8],
            &card[8..12],
            &card[12..16]
        );
        // Fails Luhn.
        assert!(kinds(&bump_last_digit(&spaced)).is_empty());
        // Phone number: too short and fails Luhn.
        assert!(kinds("+1 555-123-4567").is_empty());
        // A 12-digit run is too short to be a card, Luhn-valid or not.
        assert!(kinds(&card[0..12]).is_empty());
        assert!(kinds("123456789012").is_empty());
        // 20+ digit run is not sliced into a card.
        assert!(kinds(&format!("{card}0000")).is_empty());
        // Double spaces break the run.
        assert!(kinds(&spaced.replace(' ', "  ")).is_empty());
    }

    #[test]
    fn duplicate_values_are_reported_once_and_kinds_mix() {
        let card = luhn_complete("123456789012345");
        let text = format!(
            "key sk-aaaaaaaaaaaaaaaaaaaaaaaaaa again sk-aaaaaaaaaaaaaaaaaaaaaaaaaa\ncard {card}"
        );
        let hits = scan_secrets(&text);
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!(hits[0].kind, SecretKind::ApiKey);
        assert_eq!(hits[1].kind, SecretKind::CardNumber);
    }

    #[test]
    fn secret_kind_serializes_and_displays() {
        assert_eq!(
            serde_json::to_string(&SecretKind::ApiKey).unwrap(),
            "\"api_key\""
        );
        assert_eq!(SecretKind::CardNumber.to_string(), "card number");
        assert_eq!(SecretKind::Jwt.with_article(), "a JWT");
    }

    #[test]
    fn url_host_strips_scheme_port_userinfo_and_path() {
        assert_eq!(url_host("http://localhost:11434/v1"), "localhost");
        assert_eq!(
            url_host("https://user:pw@API.openai.com/v1"),
            "api.openai.com"
        );
        assert_eq!(url_host("http://[::1]:8080/v1"), "::1");
        assert_eq!(url_host("http://::1:8080"), "::1:8080");
        assert_eq!(url_host("192.168.1.5:8000"), "192.168.1.5");
        assert_eq!(url_host("https://openrouter.ai/api/v1"), "openrouter.ai");
    }

    #[test]
    fn local_servers_are_not_hosted() {
        let k = ProviderKind::OpenAiCompatible;
        for url in [
            "http://localhost:11434/v1",
            "http://LOCALHOST/v1",
            "http://127.0.0.1:1234/v1",
            "http://127.0.0.2/v1",
            "http://[::1]:8080/v1",
            "http://0.0.0.0:8000",
            "http://mac-studio.local:1234/v1",
            "http://10.0.0.7:8080/v1",
            "http://192.168.1.20:11434/v1",
            "http://172.16.0.1/v1",
            "http://172.31.255.254/v1",
        ] {
            assert!(!provider_is_hosted(k, url), "{url} should be local");
        }
    }

    #[test]
    fn hosted_providers_and_defaults_are_hosted() {
        assert!(provider_is_hosted(ProviderKind::OpenAiCompatible, ""));
        assert!(provider_is_hosted(ProviderKind::OpenAiCompatible, "   "));
        assert!(provider_is_hosted(ProviderKind::Anthropic, ""));
        assert!(provider_is_hosted(ProviderKind::OpenRouter, ""));
        assert!(provider_is_hosted(
            ProviderKind::OpenAiCompatible,
            "https://api.openai.com/v1"
        ));
        assert!(provider_is_hosted(
            ProviderKind::OpenAiCompatible,
            "https://my-proxy.example.com/v1"
        ));
        // 172.32.x and 11.x are public.
        assert!(provider_is_hosted(
            ProviderKind::OpenAiCompatible,
            "http://172.32.0.1/v1"
        ));
        assert!(provider_is_hosted(
            ProviderKind::OpenAiCompatible,
            "http://11.0.0.1/v1"
        ));
        // A name that merely contains "local" is still hosted.
        assert!(provider_is_hosted(
            ProviderKind::OpenAiCompatible,
            "https://localai.example.com/v1"
        ));
    }
}
