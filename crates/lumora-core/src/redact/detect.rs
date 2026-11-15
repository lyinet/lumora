//! Text detectors: find personal or secret data in one line of OCR text.
//!
//! Hand-written scanners (no regex dependency) over `char`s. Every hit is a
//! half-open range of **char indices** into the line, so it can be mapped
//! back onto the OCR boxes (see [`super::layout`]).

use std::ops::Range;

use super::Category;

/// One detector match in a line of text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub category: Category,
    /// Char indices (not bytes) into the line.
    pub range: Range<usize>,
}

impl Hit {
    fn new(category: Category, range: Range<usize>) -> Self {
        Hit { category, range }
    }
}

/// Every hit in `text` whose category `enabled` accepts, sorted by start,
/// with overlaps resolved in favour of the longer match.
pub fn detect(text: &str, enabled: impl Fn(Category) -> bool) -> Vec<Hit> {
    let chars: Vec<char> = text.chars().collect();
    let mut hits = Vec::new();
    hits.extend(emails(&chars));
    hits.extend(numbers(&chars));
    hits.extend(ipv4(&chars));
    hits.extend(url_tokens(&chars));
    hits.extend(secrets(&chars));
    hits.retain(|h| enabled(h.category) && !h.range.is_empty());
    resolve_overlaps(hits)
}

fn resolve_overlaps(mut hits: Vec<Hit>) -> Vec<Hit> {
    // Longest first, then earliest; keep a hit only if it overlaps nothing kept.
    hits.sort_by(|a, b| b.range.len().cmp(&a.range.len()).then(a.range.start.cmp(&b.range.start)));
    let mut kept: Vec<Hit> = Vec::new();
    for h in hits {
        if !kept.iter().any(|k| k.range.start < h.range.end && h.range.start < k.range.end) {
            kept.push(h);
        }
    }
    kept.sort_by_key(|h| h.range.start);
    kept
}

// -- email -----------------------------------------------------------------------

fn is_local_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-')
}

fn is_domain_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-')
}

fn emails(s: &[char]) -> Vec<Hit> {
    let mut out = Vec::new();
    for (at, _) in s.iter().enumerate().filter(|(_, c)| **c == '@') {
        let mut start = at;
        while start > 0 && is_local_char(s[start - 1]) {
            start -= 1;
        }
        while start < at && matches!(s[start], '.' | '-' | '+') {
            start += 1;
        }
        let mut end = at + 1;
        while end < s.len() && is_domain_char(s[end]) {
            end += 1;
        }
        while end > at + 1 && matches!(s[end - 1], '.' | '-') {
            end -= 1;
        }
        if start == at || end <= at + 1 {
            continue;
        }
        let domain: String = s[at + 1..end].iter().collect();
        let Some((_, tld)) = domain.rsplit_once('.') else { continue };
        if tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()) && !domain.contains("..") {
            out.push(Hit::new(Category::Email, start..end));
        }
    }
    out
}

// -- phone numbers, ID cards, bank cards -------------------------------------------

/// A run of digits with single space / hyphen separators.
struct NumberRun {
    range: Range<usize>,
    digits: String,
    /// Digits per group, in order.
    groups: Vec<usize>,
    plus: bool,
    /// Ends in an `X` check digit (CN ID cards).
    x_check: bool,
}

fn number_runs(s: &[char]) -> Vec<NumberRun> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let plus = s[i] == '+' && s.get(i + 1).is_some_and(|c| c.is_ascii_digit());
        let starts = plus || s[i].is_ascii_digit();
        let prev_ok = i == 0 || !(s[i - 1].is_ascii_alphanumeric() || matches!(s[i - 1], '.' | '_' | '/'));
        if !starts || !prev_ok {
            i += 1;
            continue;
        }
        let start = i;
        if plus {
            i += 1;
        }
        let mut digits = String::new();
        let mut groups = vec![0usize];
        let mut end = i;
        while i < s.len() {
            let c = s[i];
            if c.is_ascii_digit() {
                digits.push(c);
                *groups.last_mut().unwrap_or(&mut 0) += 1;
                i += 1;
                end = i;
            } else if matches!(c, ' ' | '-')
                && s.get(i + 1).is_some_and(|n| n.is_ascii_digit())
                && groups.last().is_some_and(|&g| g > 0)
            {
                // Look ahead: groups after the first need 3+ digits (so
                // dates like 2024-01-15 and lists of small numbers stop).
                let next_len = s[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
                if next_len < 3 && !(plus && groups.len() == 1) {
                    break;
                }
                groups.push(0);
                i += 1;
            } else {
                break;
            }
        }
        let mut x_check = false;
        if digits.len() == 17 && s.get(end).is_some_and(|c| matches!(c, 'X' | 'x')) {
            x_check = true;
            end += 1;
        }
        let next_ok = !matches!(s.get(end), Some(c) if c.is_ascii_alphanumeric() || *c == '_')
            && !(s.get(end) == Some(&'.') && s.get(end + 1).is_some_and(|c| c.is_ascii_digit()));
        if next_ok && !digits.is_empty() {
            out.push(NumberRun { range: start..end, digits, groups, plus, x_check });
        }
        i = end.max(start + 1);
    }
    out
}

/// Luhn checksum over a digit string.
pub fn luhn_valid(digits: &str) -> bool {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let sum: u32 = digits
        .bytes()
        .rev()
        .enumerate()
        .map(|(i, b)| {
            let d = u32::from(b - b'0');
            if i % 2 == 1 {
                let x = d * 2;
                if x > 9 {
                    x - 9
                } else {
                    x
                }
            } else {
                d
            }
        })
        .sum();
    sum % 10 == 0
}

/// Mainland China resident ID (18 chars, ISO 7064 MOD 11-2 check digit).
pub fn cn_id_valid(id: &str) -> bool {
    let c: Vec<char> = id.chars().collect();
    if c.len() != 18 || !c[..17].iter().all(char::is_ascii_digit) {
        return false;
    }
    const W: [u32; 17] = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];
    const CHECK: [char; 11] = ['1', '0', 'X', '9', '8', '7', '6', '5', '4', '3', '2'];
    let sum: u32 = c[..17].iter().zip(W).map(|(d, w)| d.to_digit(10).unwrap_or(0) * w).sum();
    // Plausible birth date: 19xx/20xx, month 01–12, day 01–31.
    let year: u32 = c[6..10].iter().collect::<String>().parse().unwrap_or(0);
    let month: u32 = c[10..12].iter().collect::<String>().parse().unwrap_or(0);
    let day: u32 = c[12..14].iter().collect::<String>().parse().unwrap_or(0);
    let date_ok = (1900..=2099).contains(&year) && (1..=12).contains(&month) && (1..=31).contains(&day);
    date_ok && CHECK[(sum % 11) as usize] == c[17].to_ascii_uppercase()
}

fn is_cn_mobile(d: &str) -> bool {
    let b = d.as_bytes();
    b.len() == 11 && b[0] == b'1' && (b'3'..=b'9').contains(&b[1])
}

/// Issuer prefixes of real card networks (Amex/Diners 3, Visa 4,
/// Mastercard 5 and 2221–2720, Discover/UnionPay 6). Rules out most
/// timestamps and order numbers that happen to pass Luhn.
fn card_prefix_ok(d: &str) -> bool {
    match d.as_bytes()[0] {
        b'3'..=b'6' => true,
        b'2' => d.get(..4).and_then(|p| p.parse::<u32>().ok()).is_some_and(|p| (2221..=2720).contains(&p)),
        _ => false,
    }
}

fn card_groups_ok(groups: &[usize]) -> bool {
    match groups.len() {
        1 => true,
        // 4-4-4-4(-x) or Amex 4-6-5.
        _ => groups[..groups.len() - 1].iter().all(|&g| g == 4) || groups == [4, 6, 5] || groups == [4, 6, 4],
    }
}

fn numbers(s: &[char]) -> Vec<Hit> {
    let mut out = Vec::new();
    for run in number_runs(s) {
        let d = run.digits.as_str();
        let n = d.len();
        let cat = if run.x_check {
            cn_id_valid(&format!("{d}X")).then_some(Category::IdCard)
        } else if run.plus {
            (8..=15).contains(&n).then_some(Category::Phone)
        } else if n == 18 && cn_id_valid(d) {
            Some(Category::IdCard)
        } else if (is_cn_mobile(d) && run.groups.len() <= 3)
            || (n == 13 && d.starts_with("86") && is_cn_mobile(&d[2..]))
        {
            Some(Category::Phone)
        } else if (13..=19).contains(&n) && luhn_valid(d) && card_prefix_ok(d) && card_groups_ok(&run.groups) {
            Some(Category::BankCard)
        } else {
            None
        };
        if let Some(c) = cat {
            out.push(Hit::new(c, run.range));
        }
    }
    out
}

// -- IPv4 ------------------------------------------------------------------------------

fn ipv4(s: &[char]) -> Vec<Hit> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let prev_ok = i == 0 || !(s[i - 1].is_ascii_alphanumeric() || s[i - 1] == '.');
        if !s[i].is_ascii_digit() || !prev_ok {
            i += 1;
            continue;
        }
        let start = i;
        let mut parts = 0;
        let mut ok = true;
        let mut j = i;
        loop {
            let digits = s[j..].iter().take_while(|c| c.is_ascii_digit()).count();
            if digits == 0 || digits > 3 {
                ok = false;
                break;
            }
            let v: u32 = s[j..j + digits].iter().collect::<String>().parse().unwrap_or(999);
            if v > 255 || (digits > 1 && s[j] == '0') {
                ok = false;
                break;
            }
            parts += 1;
            j += digits;
            if parts == 4 {
                break;
            }
            if s.get(j) != Some(&'.') {
                ok = false;
                break;
            }
            j += 1;
        }
        let next_ok = !matches!(s.get(j), Some(c)
            if c.is_ascii_alphanumeric() || (*c == '.' && s.get(j + 1).is_some_and(char::is_ascii_digit)));
        if ok && parts == 4 && next_ok {
            out.push(Hit::new(Category::IpAddress, start..j));
            i = j;
        } else {
            i += 1;
            while i < s.len() && (s[i].is_ascii_digit() || s[i] == '.') {
                i += 1;
            }
        }
    }
    out
}

// -- URLs with secrets in the query ------------------------------------------------------

const SENSITIVE_KEYS: [&str; 14] = [
    "token",
    "auth",
    "key",
    "secret",
    "sig",
    "signature",
    "password",
    "passwd",
    "pwd",
    "session",
    "sid",
    "jwt",
    "code",
    "credential",
];

fn sensitive_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    SENSITIVE_KEYS
        .iter()
        .any(|s| k == *s || k.contains("token") || k.contains("secret") || k.ends_with(&format!("_{s}")))
        || k.contains("apikey")
        || k.contains("api_key")
        || k.contains("signature")
        || k.contains("password")
        || k.contains("credential")
}

fn url_tokens(s: &[char]) -> Vec<Hit> {
    let mut out = Vec::new();
    let text: String = s.iter().collect();
    let lower = text.to_ascii_lowercase();
    let mut search = 0;
    while let Some(off) = lower[search..].find("http") {
        let byte_start = search + off;
        search = byte_start + 4;
        let rest = &lower[byte_start..];
        if !(rest.starts_with("http://") || rest.starts_with("https://")) {
            continue;
        }
        let start = text[..byte_start].chars().count();
        let mut end = start;
        while end < s.len()
            && !s[end].is_whitespace()
            && !crate::ocr::is_cjk(s[end])
            && !matches!(s[end], '"' | '<' | '>')
        {
            end += 1;
        }
        // Query: after '?', pairs split by '&' (fragment '#' ends it).
        let Some(q) = (start..end).find(|&k| s[k] == '?') else { continue };
        let mut k = q + 1;
        while k < end && s[k] != '#' {
            let pair_end = (k..end).find(|&m| s[m] == '&' || s[m] == '#').unwrap_or(end);
            if let Some(eq) = (k..pair_end).find(|&m| s[m] == '=') {
                let key: String = s[k..eq].iter().collect();
                let mut vend = pair_end;
                while vend > eq + 1 && matches!(s[vend - 1], '.' | ',' | ')' | ';') {
                    vend -= 1;
                }
                if vend - (eq + 1) >= 6 && sensitive_key(&key) {
                    out.push(Hit::new(Category::UrlToken, eq + 1..vend));
                }
            }
            k = pair_end + 1;
        }
    }
    out
}

// -- API keys and tokens ------------------------------------------------------------------

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '+')
}

/// Shannon entropy of `s` in bits per char.
pub fn entropy(s: &str) -> f64 {
    let n = s.chars().count();
    if n == 0 {
        return 0.0;
    }
    let mut counts = std::collections::HashMap::new();
    for c in s.chars() {
        *counts.entry(c).or_insert(0usize) += 1;
    }
    counts
        .values()
        .map(|&k| {
            let p = k as f64 / n as f64;
            -p * p.log2()
        })
        .sum()
}

fn rest_len(t: &str, prefix: &str) -> Option<usize> {
    t.strip_prefix(prefix).map(str::len)
}

/// Whether one whitespace-delimited token is an API key / token.
pub fn is_secret_token(t: &str) -> bool {
    let alnum = |s: &str| s.chars().filter(char::is_ascii_alphanumeric).count();
    if let Some(n) = rest_len(t, "sk-") {
        return n >= 20 && alnum(t) >= 20;
    }
    for p in ["ghp_", "gho_", "ghu_", "ghs_", "ghr_"] {
        if let Some(n) = rest_len(t, p) {
            return n >= 30;
        }
    }
    if let Some(n) = rest_len(t, "github_pat_") {
        return n >= 30;
    }
    if (t.starts_with("AKIA") || t.starts_with("ASIA"))
        && t.len() == 20
        && t.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return true;
    }
    if ["xoxb-", "xoxp-", "xoxa-", "xoxs-", "xoxr-"].iter().any(|p| t.starts_with(p)) {
        return t.len() >= 15;
    }
    if t.starts_with("eyJ") && t.matches('.').count() == 2 && t.len() >= 30 {
        return true;
    }
    // Generic: long, mixed letters and digits, random-looking.
    if t.len() < 32 || t.contains("://") || t.contains('.') {
        return false;
    }
    let has_digit = t.chars().any(|c| c.is_ascii_digit());
    let has_alpha = t.chars().any(|c| c.is_ascii_alphabetic());
    if !(has_digit && has_alpha) {
        return false;
    }
    let body = t.trim_end_matches('=');
    let hex = body.chars().all(|c| c.is_ascii_hexdigit());
    // Paths (a/b/c) and slugs (words-with-dashes) are not secrets.
    let words = body.split(['/', '-', '_']).filter(|w| w.len() >= 3).count();
    if body.matches('/').count() >= 3 || (body.matches('-').count() >= 3 && words >= 4 && !hex) {
        return false;
    }
    let e = entropy(body);
    if hex {
        e >= 3.0
    } else {
        e >= 3.8
    }
}

fn secrets(s: &[char]) -> Vec<Hit> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if !is_token_char(s[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < s.len() && is_token_char(s[i]) {
            i += 1;
        }
        let mut end = i;
        while end < s.len() && s[end] == '=' {
            end += 1;
        }
        i = end;
        // Trailing sentence punctuation is not part of the token.
        let mut tend = end;
        while tend > start && matches!(s[tend - 1], '.' | ',') {
            tend -= 1;
        }
        let token: String = s[start..tend].iter().collect();
        if is_secret_token(&token) {
            out.push(Hit::new(Category::Secret, start..tend));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cats(text: &str) -> Vec<(Category, String)> {
        let chars: Vec<char> = text.chars().collect();
        detect(text, |_| true).into_iter().map(|h| (h.category, chars[h.range].iter().collect())).collect()
    }

    fn one(text: &str, c: Category, matched: &str) {
        assert_eq!(cats(text), vec![(c, matched.to_string())], "{text}");
    }

    #[test]
    fn emails() {
        one("Contact: jane.doe+tag@mail.example.co.uk.", Category::Email, "jane.doe+tag@mail.example.co.uk");
        one("邮箱：张三zhang_san@qq.com谢谢", Category::Email, "zhang_san@qq.com");
        assert!(cats("@mention and user@localhost and a@b.c").is_empty());
    }

    #[test]
    fn cn_and_international_phones() {
        one("手机 13800138000", Category::Phone, "13800138000");
        one("Tel: 138 0013 8000.", Category::Phone, "138 0013 8000");
        one("call 138-0013-8000 now", Category::Phone, "138-0013-8000");
        one("+86 138 0013 8000", Category::Phone, "+86 138 0013 8000");
        one("US: +1 415-555-0123", Category::Phone, "+1 415-555-0123");
        // 12 digits / wrong prefix / embedded in a longer id.
        assert!(cats("12800138000 and 138001380001 and ID13800138000").is_empty());
    }

    #[test]
    fn id_cards_need_a_valid_checksum() {
        assert!(cn_id_valid("11010519491231002X"));
        assert!(!cn_id_valid("110105194912310021") && !cn_id_valid("11010519491331002X"));
        one("身份证号11010519491231002X", Category::IdCard, "11010519491231002X");
        // Wrong check digit: not an ID (and not a card either).
        assert!(cats("ID: 110105194912310021").is_empty());
    }

    #[test]
    fn bank_cards_need_luhn_and_a_card_prefix() {
        assert!(luhn_valid("4111111111111111") && !luhn_valid("4111111111111112") && !luhn_valid("12a"));
        one("Visa 4111 1111 1111 1111 exp", Category::BankCard, "4111 1111 1111 1111");
        one("卡号6222021234567890128", Category::BankCard, "6222021234567890128");
        one("Amex 3782 822463 10005", Category::BankCard, "3782 822463 10005");
        // Luhn-valid but a timestamp (prefix 2024) / wrong grouping.
        assert!(cats("ts 20240115123045 order 4111 11 1111 1111 1111").is_empty());
    }

    #[test]
    fn secrets_and_tokens() {
        one("OPENAI_API_KEY=sk-proj-AbCdEf1234567890GhIjKlMn", Category::Secret, "sk-proj-AbCdEf1234567890GhIjKlMn");
        one(
            "token ghp_1234567890abcdefghijABCDEFGHIJ123456",
            Category::Secret,
            "ghp_1234567890abcdefghijABCDEFGHIJ123456",
        );
        one("aws AKIAIOSFODNN7EXAMPLE", Category::Secret, "AKIAIOSFODNN7EXAMPLE");
        one("slack xoxb-123456789012-abcdefABCDEF", Category::Secret, "xoxb-123456789012-abcdefABCDEF");
        one(
            "Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk",
            Category::Secret,
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk",
        );
        one(
            "secret: 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b",
            Category::Secret,
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b",
        );
        one(
            "key Zm9vYmFyLXNlY3JldC1rZXktUXc4dDJ6WnBMbTNy==",
            Category::Secret,
            "Zm9vYmFyLXNlY3JldC1rZXktUXc4dDJ6WnBMbTNy==",
        );
    }

    #[test]
    fn ordinary_text_is_left_alone() {
        let benign = [
            "Version 2.4.1 released on 2024-05-01 at 10:30",
            "Lumora v0.5.0 (build 20241116) — 1,234,567 downloads",
            "/Users/jane/Library/Application/Support/Lumora/settings.toml",
            "the-quick-brown-fox-jumps-over-the-lazy-dog-again-and-again",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1",
            "Total ¥ 1999.00 · 共 3 件 · 订单 2024 1116 0001",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
        ];
        for b in benign {
            assert!(cats(b).is_empty(), "{b}: {:?}", cats(b));
        }
    }

    #[test]
    fn ipv4_addresses() {
        one("server 192.168.1.20 is up", Category::IpAddress, "192.168.1.20");
        one("ssh root@10.0.0.1", Category::IpAddress, "10.0.0.1");
        assert!(cats("v1.2.3.4 and 256.1.1.1 and 1.2.3 and 01.2.3.4 and 1.2.3.4.5").is_empty());
    }

    #[test]
    fn url_query_secrets() {
        let text = "https://api.example.com/v1/files?id=42&access_token=abcDEF123456&sig=zzzzzzzz#top";
        let found = cats(text);
        assert_eq!(
            found,
            vec![(Category::UrlToken, "abcDEF123456".to_string()), (Category::UrlToken, "zzzzzzzz".to_string())]
        );
        assert!(cats("https://example.com/search?q=hello&page=2").is_empty());
    }

    #[test]
    fn categories_can_be_disabled_and_overlaps_resolve() {
        let text = "jane@example.com 13800138000";
        assert_eq!(detect(text, |c| c != Category::Email).len(), 1);
        // A JWT inside a URL query is one hit, not two.
        let url = "https://x.io/cb?token=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.dBjftJeZ4CVPmB92K27uhbUJU1p1";
        assert_eq!(detect(url, |_| true).len(), 1);
        assert!(entropy("") == 0.0 && entropy("abab") == 1.0);
    }
}
