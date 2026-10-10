//! Search & Redact patterns (Acrobat's Find Text ▸ Patterns): phone numbers, email addresses,
//! credit card numbers, US Social Security numbers and dates. Each matcher works on a page's
//! text as characters and returns character ranges; callers map them to glyphs and areas.

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pattern {
    Phone,
    Email,
    CreditCard,
    Ssn,
    Date,
}

pub const PATTERNS: [Pattern; 5] = [Pattern::Phone, Pattern::Email, Pattern::CreditCard, Pattern::Ssn, Pattern::Date];

impl Pattern {
    pub fn label(self) -> &'static str {
        match self {
            Pattern::Phone => "Phone Numbers",
            Pattern::Email => "Email Addresses",
            Pattern::CreditCard => "Credit Cards",
            Pattern::Ssn => "Social Security Numbers",
            Pattern::Date => "Dates",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Pattern::Phone => "phone",
            Pattern::Email => "email",
            Pattern::CreditCard => "credit-card",
            Pattern::Ssn => "ssn",
            Pattern::Date => "date",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        PATTERNS.into_iter().find(|p| p.id() == id)
    }
}

fn digit(c: Option<&char>) -> bool {
    c.is_some_and(char::is_ascii_digit)
}

fn word(c: Option<&char>) -> bool {
    c.is_some_and(|c| c.is_alphanumeric())
}

/// Digits and the separators allowed between them, from `start`: (end, digit count).
fn digit_run(s: &[char], start: usize, seps: &[char], max_len: usize) -> (usize, usize) {
    let (mut i, mut n) = (start, 0);
    while i < s.len() && i - start < max_len {
        let c = s[i];
        if c.is_ascii_digit() {
            n += 1;
        } else if !(seps.contains(&c) && (digit(s.get(i + 1)) || s.get(i + 1) == Some(&'(')) || (c == '(' || c == ')') && seps.contains(&c)) {
            break;
        }
        i += 1;
    }
    // Never end on a separator.
    while i > start && !s[i - 1].is_ascii_digit() {
        i -= 1;
    }
    (i, n)
}

fn phone(s: &[char], i: usize) -> Option<usize> {
    let start_ok = s[i] == '+' && digit(s.get(i + 1)) || s[i] == '(' && digit(s.get(i + 1)) || s[i].is_ascii_digit();
    if !start_ok || word(i.checked_sub(1).and_then(|p| s.get(p))) || s.get(i.wrapping_sub(1)) == Some(&'+') {
        return None;
    }
    let plus = s[i] == '+';
    let first = if plus { i + 1 } else { i };
    let (end, n) = digit_run(s, first, &[' ', '-', '.', '(', ')'], 28);
    let run = s.get(first..end)?;
    let seps = run.iter().filter(|c| !c.is_ascii_digit()).count();
    // Always grouped by at least one separator, so plain numbers never match.
    if seps == 0 || digit(s.get(end)) {
        return None;
    }
    let ok = if plus {
        // E.164: a `+` country code and up to 15 digits in all ("+49 2151 123456").
        (8..=15).contains(&n)
    } else {
        // North American: 10 digits, 11 with a country code ("(555) 123-4567").
        let nanp = (10..=11).contains(&n);
        // National with a trunk 0 and 6–14 more digits ("0211 123456", "01 23 45 67 89").
        // The first group needs two digits so decimals ("0.1234567") don't match, and
        // day-first dates ("01.10.2026") are left to the date pattern.
        let mut digits = run.iter().skip_while(|c| !c.is_ascii_digit());
        let trunk_zero = digits.clone().next() == Some(&'0');
        let lead = digits.by_ref().take_while(|c| c.is_ascii_digit()).count();
        let trunk = trunk_zero && (7..=15).contains(&n) && lead >= 2 && date(s, i).is_none();
        nanp || trunk
    };
    ok.then_some(end - i)
}

/// Scripts written without spaces between words (Chinese, Japanese, Korean, Thai, …): text in
/// them often runs straight into an address, so their letters never count as part of one.
fn spaceless(c: char) -> bool {
    matches!(
        u32::from(c),
        0x0E00..=0x0EFF | 0x1000..=0x109F | 0x1100..=0x11FF | 0x1780..=0x17FF | 0x2E80..=0x9FFF | 0xA960..=0xA97F | 0xAC00..=0xD7FF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x3FFFF
    )
}

/// A letter or digit of an address: ASCII, or of any script written with spaces, so
/// internationalized addresses (RFC 6531, IDN) such as "ayşe@örnek.com.tr" match.
fn address_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c.is_alphanumeric() && !spaceless(c)
}

fn email(s: &[char], i: usize) -> Option<usize> {
    let local = |c: &char| address_char(*c) || "._%+-".contains(*c);
    if !address_char(s[i]) || i.checked_sub(1).and_then(|p| s.get(p)).is_some_and(local) {
        return None;
    }
    let mut j = i;
    while j < s.len() && local(&s[j]) {
        j += 1;
    }
    if s.get(j) != Some(&'@') {
        return None;
    }
    let d0 = j + 1;
    let mut k = d0;
    while k < s.len() && (address_char(s[k]) || s[k] == '-' || s[k] == '.' && s.get(k + 1).is_some_and(|c| address_char(*c))) {
        k += 1;
    }
    let domain: String = s[d0..k].iter().collect();
    let tld = domain.rsplit('.').next().unwrap_or("");
    (domain.contains('.') && tld.chars().count() >= 2 && tld.chars().all(char::is_alphabetic)).then_some(k - i)
}

fn luhn(digits: &[u32]) -> bool {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(k, d)| {
            if k % 2 == 1 {
                let x = d * 2;
                if x > 9 { x - 9 } else { x }
            } else {
                *d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

fn credit_card(s: &[char], i: usize) -> Option<usize> {
    if !s[i].is_ascii_digit() || digit(i.checked_sub(1).and_then(|p| s.get(p))) {
        return None;
    }
    let (end, n) = digit_run(s, i, &[' ', '-'], 23);
    if !(13..=19).contains(&n) || digit(s.get(end)) {
        return None;
    }
    let digits: Vec<u32> = s[i..end].iter().filter_map(|c| c.to_digit(10)).collect();
    luhn(&digits).then_some(end - i)
}

fn ssn(s: &[char], i: usize) -> Option<usize> {
    if digit(i.checked_sub(1).and_then(|p| s.get(p))) {
        return None;
    }
    let g = |from: usize, n: usize| (from..from + n).all(|k| digit(s.get(k)));
    let sep = |k: usize| matches!(s.get(k), Some('-' | ' '));
    if !(g(i, 3) && sep(i + 3) && g(i + 4, 2) && sep(i + 6) && g(i + 7, 4)) || digit(s.get(i + 11)) {
        return None;
    }
    let area: String = s[i..i + 3].iter().collect();
    (area != "000" && area != "666" && !area.starts_with('9')).then_some(11)
}

/// Month names the date pattern knows: English, then the languages whose dates are written
/// "5 Ekim 2024" / "5. Oktober 2024" (day, month name, year).
const MONTHS: [[&str; 12]; 5] = [
    ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"],
    ["ocak", "şubat", "mart", "nisan", "mayıs", "haziran", "temmuz", "ağustos", "eylül", "ekim", "kasım", "aralık"],
    ["januar", "februar", "märz", "april", "mai", "juni", "juli", "august", "september", "oktober", "november", "dezember"],
    ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"],
    ["gennaio", "febbraio", "marzo", "aprile", "maggio", "giugno", "luglio", "agosto", "settembre", "ottobre", "novembre", "dicembre"],
];

/// Abbreviations that may end in a full stop: English and Turkish ("Oca.", "Şub", "Ağu").
const MONTH_ABBREVIATIONS: [[&str; 12]; 2] = [
    ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"],
    ["oca", "şub", "mar", "nis", "may", "haz", "tem", "ağu", "eyl", "eki", "kas", "ara"],
];

/// One lowercase character for `c`, with Turkish dotted and dotless i the same as i, so
/// "MAYIS", "Mayıs" and "EKİM" match and every character stays one character.
fn fold(c: char) -> char {
    match c {
        'I' | '\u{130}' | '\u{131}' => 'i',
        _ => c.to_lowercase().next().unwrap_or(c),
    }
}

/// Characters `name` covers at `i`, when it stands there as a whole word (any case).
fn word_at(s: &[char], i: usize, name: &str) -> Option<usize> {
    let mut n = 0;
    for c in name.chars() {
        if s.get(i + n).copied().map(fold) != Some(fold(c)) {
            return None;
        }
        n += 1;
    }
    (!s.get(i + n).is_some_and(|c| c.is_alphabetic())).then_some(n)
}

/// A month name or its abbreviation (optionally with a full stop) at `i`.
fn month(s: &[char], i: usize) -> Option<usize> {
    if word(i.checked_sub(1).and_then(|p| s.get(p))) {
        return None;
    }
    if let Some(n) = MONTHS.iter().flatten().filter_map(|m| word_at(s, i, m)).max() {
        return Some(n);
    }
    let n = MONTH_ABBREVIATIONS.iter().flatten().find_map(|m| word_at(s, i, m))?;
    Some(if s.get(i + n) == Some(&'.') { n + 1 } else { n })
}

fn number(s: &[char], i: usize, min: usize, max: usize) -> Option<usize> {
    let n = s[i.min(s.len())..].iter().take(max + 1).take_while(|c| c.is_ascii_digit()).count();
    (n >= min && n <= max).then_some(n)
}

fn date(s: &[char], i: usize) -> Option<usize> {
    if word(i.checked_sub(1).and_then(|p| s.get(p))) {
        return None;
    }
    let spaces = |k: usize| s[k.min(s.len())..].iter().take_while(|c| **c == ' ').count();
    // Numeric: m/d/yy(yy), m-d-yyyy, d.m.yyyy, yyyy-mm-dd.
    if let Some(a) = number(s, i, 1, 4) {
        let sep = s.get(i + a).copied();
        if matches!(sep, Some('/' | '-' | '.'))
            && let Some(b) = number(s, i + a + 1, 1, 2)
            && s.get(i + a + 1 + b).copied() == sep
            && let Some(c) = number(s, i + a + b + 2, if a == 4 { 1 } else { 2 }, if a == 4 { 2 } else { 4 })
            && (a <= 2 || a == 4)
            && !digit(s.get(i + a + b + 2 + c))
        {
            return Some(a + b + c + 2);
        }
        // "5 January 2024", "5 Ekim 2024", "5. Oktober 2024".
        let day = i + a + usize::from(s.get(i + a) == Some(&'.'));
        if a <= 2
            && let k = day + spaces(day)
            && k > day
            && let Some(m) = month(s, k)
        {
            let k2 = k + m + spaces(k + m);
            if let Some(y) = number(s, k2, 4, 4) {
                return Some(k2 + y - i);
            }
        }
        return None;
    }
    // "January 5, 2024" / "Jan. 5 2024".
    let m = month(s, i)?;
    let k = i + m + spaces(i + m);
    let d = number(s, k, 1, 2)?;
    let mut k2 = k + d;
    if s.get(k2) == Some(&',') {
        k2 += 1;
    }
    k2 += spaces(k2);
    let y = number(s, k2, 4, 4)?;
    Some(k2 + y - i)
}

/// Character ranges of `text` matching `pattern`.
pub fn find(pattern: Pattern, text: &[char]) -> Vec<Range<usize>> {
    let f: fn(&[char], usize) -> Option<usize> = match pattern {
        Pattern::Phone => phone,
        Pattern::Email => email,
        Pattern::CreditCard => credit_card,
        Pattern::Ssn => ssn,
        Pattern::Date => date,
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        match f(text, i) {
            Some(n) if n > 0 => {
                out.push(i..i + n);
                i += n;
            }
            _ => i += 1,
        }
    }
    out
}

/// The most entries a word list keeps (Find Text ▸ Multiple words or phrases).
pub const MAX_WORDS: usize = 1000;
/// The most characters an entry keeps.
pub const MAX_WORD_CHARS: usize = 256;

/// A word list from text with one word or phrase per line: trimmed, without empty lines or
/// repeats, in order, at most [`MAX_WORDS`] entries of at most [`MAX_WORD_CHARS`] characters.
pub fn word_list(text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    text.lines()
        .map(|l| l.trim().chars().take(MAX_WORD_CHARS).collect::<String>())
        .filter(|w| !w.is_empty() && seen.insert(w.clone()))
        .take(MAX_WORDS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_lists_are_trimmed_unique_and_bounded() {
        assert_eq!(word_list("  John Smith \n\n\tACME\r\nJohn Smith\n  \nő ű"), ["John Smith", "ACME", "ő ű"]);
        let long = "é".repeat(MAX_WORD_CHARS + 10);
        assert_eq!(word_list(&long)[0].chars().count(), MAX_WORD_CHARS);
        let many: String = (0..MAX_WORDS + 50).map(|i| format!("w{i}\n")).collect();
        let list = word_list(&many);
        assert_eq!((list.len(), list.last().map(String::as_str)), (MAX_WORDS, Some("w999")));
        assert!(word_list("").is_empty());
    }

    fn found(p: Pattern, s: &str) -> Vec<String> {
        let c: Vec<char> = s.chars().collect();
        find(p, &c).into_iter().map(|r| c[r].iter().collect()).collect()
    }

    #[test]
    fn phone_numbers() {
        assert_eq!(
            found(Pattern::Phone, "Call (555) 123-4567 or 555.987.6543, +1 555 222 3333; not 12345 or 5551234567890"),
            ["(555) 123-4567", "555.987.6543", "+1 555 222 3333"]
        );
        // #525: international numbers match whole; "+49" used to stay visible.
        assert_eq!(found(Pattern::Phone, "Tel. +49 2151 123456."), ["+49 2151 123456"]);
        assert_eq!(
            found(Pattern::Phone, "+44 20 7946 0018, +33 1 23 45 67 89, +49 (0) 2151 123456, (0211) 123456, 030 12345678, 01 23 45 67 89"),
            ["+44 20 7946 0018", "+33 1 23 45 67 89", "+49 (0) 2151 123456", "(0211) 123456", "030 12345678", "01 23 45 67 89"]
        );
        // Not phone numbers: day-first dates, decimals, unseparated, too short or too long.
        assert_eq!(
            found(
                Pattern::Phone,
                "on 01.10.2026 or 09-10-2026, ratio 0.1234567, id 0211123456, +4921511234567, +1 234 567, 0211 12, +49 1234 5678 9012 3456"
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn emails() {
        assert_eq!(
            found(Pattern::Email, "Write to ada.lovelace+pdf@example.co.uk, or bob@host (no TLD) or x@y.z."),
            ["ada.lovelace+pdf@example.co.uk"]
        );
    }

    #[test]
    fn internationalized_emails() {
        assert_eq!(
            found(Pattern::Email, "Yazın: ayşe.yılmaz@örnek.com.tr, ÇAĞLAR@ŞİRKET.COM.TR; müller@bücher.de, иван@пример.рф, josé@correo.es."),
            ["ayşe.yılmaz@örnek.com.tr", "ÇAĞLAR@ŞİRKET.COM.TR", "müller@bücher.de", "иван@пример.рф", "josé@correo.es"]
        );
        // Text in scripts without spaces stays out of the address around it.
        assert_eq!(found(Pattern::Email, "メールはtaro@example.jpまで、联系support@example.cn谢谢"), ["taro@example.jp", "support@example.cn"]);
        // Still no address without a dot in the domain or with a one-letter or numeric TLD.
        assert_eq!(found(Pattern::Email, "ayşe@örnek, ş@ğ.ü, kişi@alan.123, a@b.c"), Vec::<String>::new());
    }

    #[test]
    fn credit_cards_need_a_valid_checksum() {
        assert_eq!(
            found(Pattern::CreditCard, "Visa 4111 1111 1111 1111, bad 4111 1111 1111 1112, amex 3782-822463-10005"),
            ["4111 1111 1111 1111", "3782-822463-10005"]
        );
    }

    #[test]
    fn social_security_numbers() {
        assert_eq!(
            found(Pattern::Ssn, "SSN 123-45-6789 and 123 45 6789; invalid 000-12-3456, 666-12-3456, 912-12-3456, 1234-56-7890"),
            ["123-45-6789", "123 45 6789"]
        );
    }

    #[test]
    fn dates() {
        assert_eq!(
            found(Pattern::Date, "Due 10/01/2026, 2026-10-01, 1.10.26, March 5, 2024, Jan. 7 2025 and 5 June 2023; not 10/2026 or Mayday 12"),
            ["10/01/2026", "2026-10-01", "1.10.26", "March 5, 2024", "Jan. 7 2025", "5 June 2023"]
        );
    }

    #[test]
    fn dates_with_turkish_month_names_in_any_case() {
        assert_eq!(
            found(Pattern::Date, "Tarih: 10 Ekim 2026, 3 Mayıs 2025, 5 Şubat 2024, 5 ŞUBAT 2024, 3 MAYIS 2025, 1 EKİM 2026 ve 9 kasım 2023."),
            ["10 Ekim 2026", "3 Mayıs 2025", "5 Şubat 2024", "5 ŞUBAT 2024", "3 MAYIS 2025", "1 EKİM 2026", "9 kasım 2023"]
        );
        assert_eq!(
            found(Pattern::Date, "12 Oca 2024, 7 Şub. 2025, 30 Ağu 2023, 1 ARA. 2022, 2 Haz 2021"),
            ["12 Oca 2024", "7 Şub. 2025", "30 Ağu 2023", "1 ARA. 2022", "2 Haz 2021"]
        );
        // Words that only start like a month, or a month without a day and year, stay.
        assert_eq!(
            found(Pattern::Date, "5 Martı 2024, Ekimde 2026, 3 Mayısta 2025, ara 2024, Kasım ayında, 12 Aralık, Aralık 2024"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn dates_with_german_french_and_italian_month_names() {
        assert_eq!(
            found(Pattern::Date, "am 10. Oktober 2026, 3 März 2025, le 14 juillet 2024, 1 août 2023, il 25 dicembre 2022"),
            ["10. Oktober 2026", "3 März 2025", "14 juillet 2024", "1 août 2023", "25 dicembre 2022"]
        );
        // "Mai" alone, a dotted day without a month and an unknown word are not dates.
        assert_eq!(found(Pattern::Date, "Mai 2024, Kapitel 5. Absatz 2024, 5 Oktoberfest 2024"), Vec::<String>::new());
    }

    #[test]
    fn case_folding_keeps_one_character_per_character() {
        let s: Vec<char> = "\u{130}\u{131}I\u{15E}".chars().collect();
        assert_eq!(s.iter().copied().map(fold).collect::<String>(), "iii\u{15F}");
    }
}
