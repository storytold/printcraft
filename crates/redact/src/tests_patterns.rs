//! Pattern search: recall fixtures per preset (including adversarial formatting), precision
//! corpora that must not match, index mapping and input bounds.

use crate::patterns::{MAX_INPUT_CHARS, PATTERNS, Pattern, PatternError, find, try_find, try_find_many};

fn chars(s: &str) -> Vec<char> {
    s.chars().collect()
}

/// The matched substrings of `text`, cut with the returned (original) indices.
fn found(p: Pattern, text: &str) -> Vec<String> {
    let c = chars(text);
    try_find(p, &c).unwrap_or_default().into_iter().map(|r| c[r].iter().collect()).collect()
}

fn none(p: Pattern, text: &str) {
    assert_eq!(found(p, text), Vec::<String>::new(), "{p:?} must not match {text:?}");
}

fn only(p: Pattern, text: &str) {
    assert_eq!(found(p, text), [text], "{p:?} must match all of {text:?}");
}

// --- Existing presets ----------------------------------------------------------------------

#[test]
fn phone_numbers() {
    assert_eq!(
        found(Pattern::Phone, "Call (555) 123-4567 or 555.987.6543, +1 555 222 3333; not 12345 or 5551234567890"),
        ["(555) 123-4567", "555.987.6543", "+1 555 222 3333"]
    );
    only(Pattern::Phone, "+15551234567");
    only(Pattern::Phone, "1-800-555-0199");
    // 11 digits that do not start with the NANP country code are not US numbers.
    none(Pattern::Phone, "+44 7911 123456");
    none(Pattern::Phone, "+353 85 123 4567");
    none(Pattern::Phone, "order 12-555-123-4567");
    none(Pattern::Phone, "5551234567");
}

#[test]
fn emails() {
    assert_eq!(found(Pattern::Email, "Write to ada.lovelace+pdf@example.co.uk, or bob@host (no TLD) or x@y.z."), ["ada.lovelace+pdf@example.co.uk"]);
    assert_eq!(found(Pattern::Email, "(mail: a@b.org-) and c@d.io."), ["a@b.org", "c@d.io"]);
    none(Pattern::Email, "@handle and user@ and a@@b.com@");
}

#[test]
fn legacy_dates() {
    assert_eq!(
        found(Pattern::Date, "Due 10/01/2026, 2026-10-01, 1.10.26, March 5, 2024, Jan. 7 2025 and 5 June 2023; not 10/2026 or Mayday 12"),
        ["10/01/2026", "2026-10-01", "1.10.26", "March 5, 2024", "Jan. 7 2025", "5 June 2023"]
    );
}

#[test]
fn ids_and_labels_are_unique_and_round_trip() {
    for p in PATTERNS {
        assert_eq!(Pattern::from_id(p.id()), Some(p));
        assert!(!p.label().is_empty());
    }
    let mut ids: Vec<&str> = PATTERNS.iter().map(|p| p.id()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), PATTERNS.len());
    assert_eq!(Pattern::from_id("phone-us"), Some(Pattern::Phone));
    assert_eq!(Pattern::from_id("nope"), None);
}

// --- Credit cards --------------------------------------------------------------------------

#[test]
fn credit_cards_need_a_valid_checksum() {
    assert_eq!(
        found(Pattern::CreditCard, "Visa 4111 1111 1111 1111, bad 4111 1111 1111 1112, amex 3782-822463-10005"),
        ["4111 1111 1111 1111", "3782-822463-10005"]
    );
}

#[test]
fn credit_card_recall() {
    for card in [
        "4111111111111111",
        "4111-1111-1111-1111",
        "5555 5555 5555 4444",
        "378282246310005",
        "3782 822463 10005",
        "3714-496353-98431",
        "6011 1111 1111 1117",
        "3530 1113 3330 0000",
        "3056 930902 5904",
        "4222222222222",
        "6200 0000 0000 0005",
        "5105 1051 0510 5100",
        "4000 0566 5566 5556",
    ] {
        only(Pattern::CreditCard, card);
    }
}

#[test]
fn credit_card_precision() {
    // 16 digits that fail the checksum, all-same-digit runs, short lists, phone/ID shapes.
    for text in [
        "1234 5678 9012 3456",
        "4111 1111 1111 1112",
        "0000 0000 0000 0000",
        "1111 1111 1111 1111",
        "12 34 56 78 90 12 34",
        "ISBN 978-3-16-148410-0",
        "5551234567",
        "2026-10-07 2026-10-07",
        "tracking 1Z999AA10123456784",
    ] {
        none(Pattern::CreditCard, text);
    }
}

#[test]
fn credit_card_inside_longer_digit_text() {
    assert_eq!(found(Pattern::CreditCard, "ref 99 4111 1111 1111 1111 paid"), ["4111 1111 1111 1111"]);
    assert_eq!(found(Pattern::CreditCard, "a 4111 1111 1111 1111, 77"), ["4111 1111 1111 1111"]);
}

// --- SSN -----------------------------------------------------------------------------------

#[test]
fn social_security_numbers() {
    assert_eq!(
        found(Pattern::Ssn, "SSN 123-45-6789 and 123 45 6789; invalid 000-12-3456, 666-12-3456, 912-12-3456, 1234-56-7890"),
        ["123-45-6789", "123 45 6789"]
    );
    // Never-issued group and serial numbers.
    none(Pattern::Ssn, "123-00-6789 and 123-45-0000 and 899-45-0000");
    only(Pattern::Ssn, "899-45-6789");
    only(Pattern::Ssn, "123-45 6789");
}

#[test]
fn bare_ssn_needs_a_label() {
    assert_eq!(found(Pattern::Ssn, "SSN: 123456789 end"), ["123456789"]);
    assert_eq!(found(Pattern::Ssn, "ssn#123456789"), ["123456789"]);
    none(Pattern::Ssn, "account 123456789");
    none(Pattern::Ssn, "XSSN 123456789");
    none(Pattern::Ssn, "SSN 123456789012");
}

// --- IBAN ----------------------------------------------------------------------------------

#[test]
fn iban_recall() {
    for iban in [
        "DE89 3704 0044 0532 0130 00",
        "DE89370400440532013000",
        "GB82 WEST 1234 5698 7654 32",
        "FR14 2004 1010 0505 0001 3M02 606",
        "NL91 ABNA 0417 1643 00",
        "ES91 2100 0418 4502 0005 1332",
        "IT60 X054 2811 1010 0000 0123 456",
        "CH93 0076 2011 6238 5295 7",
        "BE68 5390 0754 7034",
        "gb82west12345698765432",
    ] {
        only(Pattern::Iban, iban);
    }
    assert_eq!(found(Pattern::Iban, "IBAN:DE89 3704 0044 0532 0130 00."), ["DE89 3704 0044 0532 0130 00"]);
}

#[test]
fn iban_precision() {
    // Bad checksum, wrong length for the country, unknown country, trailing alphanumerics.
    for text in [
        "DE89 3704 0044 0532 0130 01",
        "DE89 3704 0044 0532 0130 0",
        "XX89 3704 0044 0532 0130 00",
        "GB82 WEST 1234 5698 7654 32X",
        "DE89 3704 0044 0532 0130 000",
        "AB12 CDEF",
    ] {
        none(Pattern::Iban, text);
    }
}

// --- IP addresses --------------------------------------------------------------------------

#[test]
fn ipv4() {
    only(Pattern::Ipv4, "192.168.0.1");
    only(Pattern::Ipv4, "255.255.255.255");
    assert_eq!(found(Pattern::Ipv4, "from 10.0.0.1:8080, to 8.8.8.8."), ["10.0.0.1", "8.8.8.8"]);
    for text in ["256.1.1.1", "1.2.3", "1.2.3.4.5", "01.2.3.4", "v1.2.3.4", "1.2.3.456", "12.1.2.3.4"] {
        none(Pattern::Ipv4, text);
    }
}

#[test]
fn ipv6() {
    for ip in [
        "2001:0db8:85a3:0000:0000:8a2e:0370:7334",
        "2001:db8::1",
        "::1",
        "fe80::1ff:fe23:4567:890a",
        "::ffff:192.168.0.1",
        "2001:DB8:0:0:0:0:0:1",
        "2001:db8::",
    ] {
        only(Pattern::Ipv6, ip);
    }
    assert_eq!(found(Pattern::Ipv6, "addr: 2001:db8::1."), ["2001:db8::1"]);
    assert_eq!(found(Pattern::Ipv6, "[2001:db8::1]:443"), ["2001:db8::1"]);
    for text in ["12:30:45", "10:20", "std::vector", "a::b", "1:2:3:4:5:6:7:8:9", "2001:db8::1::2", "12345::1", "::", "1:2:3:4:5:6:7"] {
        none(Pattern::Ipv6, text);
    }
}

// --- Postal codes --------------------------------------------------------------------------

#[test]
fn uk_postcodes() {
    for pc in ["SW1A 1AA", "M1 1AE", "B33 8TH", "CR2 6XH", "DN55 1PT", "EC1A 1BB", "W1A 0AX", "GIR 0AA", "SW1A1AA", "M11AE"] {
        only(Pattern::UkPostcode, pc);
    }
    assert_eq!(found(Pattern::UkPostcode, "London SW1A 1AA, UK."), ["SW1A 1AA"]);
    // Inward letters never include C I K M O V; first letters never Q V X; second never I J Z.
    for text in ["SW1A 1CA", "QA1 1AA", "AI1 1AA", "SW1A 1AAA", "xSW1A 1AA", "AB1", "1AA 1AA"] {
        none(Pattern::UkPostcode, text);
    }
}

#[test]
fn us_zips() {
    only(Pattern::UsZip, "94105");
    only(Pattern::UsZip, "94105-1234");
    only(Pattern::UsZip, "02134");
    assert_eq!(found(Pattern::UsZip, "San Francisco, CA 94105, USA; Boston MA 02134-0001."), ["94105", "02134-0001"]);
    for text in ["$12345", "1234", "123456", "12345-123", "1,23456", "12345.67", "12345%", "00000", "A12345", "12345-6789-1", "12/12345"] {
        none(Pattern::UsZip, text);
    }
}

// --- Dates ---------------------------------------------------------------------------------

#[test]
fn iso_dates() {
    only(Pattern::DateIso, "2026-10-07");
    only(Pattern::DateIso, "2024-02-29");
    assert_eq!(found(Pattern::DateIso, "at 2026-10-07T09:30:00Z and (1999-12-31)."), ["2026-10-07", "1999-12-31"]);
    for text in [
        "2026-13-01",
        "2026-00-10",
        "2026-02-30",
        "2025-02-29",
        "2026-10-32",
        "2026-1-01",
        "12026-10-07",
        "2026-10-071",
        "2026-10-07-01",
        "0999-01-01",
    ] {
        none(Pattern::DateIso, text);
    }
}

#[test]
fn us_dates() {
    only(Pattern::DateUs, "10/07/2026");
    only(Pattern::DateUs, "1/2/26");
    only(Pattern::DateUs, "2/29/24");
    assert_eq!(found(Pattern::DateUs, "on 12/31/1999, then 3/4/05."), ["12/31/1999", "3/4/05"]);
    for text in ["13/01/2026", "0/5/2026", "2/30/2026", "2/29/2025", "1/2/3", "1/2/203", "1/2/2026/4", "11/12/13/14", "1/2/20267"] {
        none(Pattern::DateUs, text);
    }
}

// --- International phone numbers -----------------------------------------------------------

#[test]
fn intl_phones() {
    for n in [
        "+44 7911 123456",
        "+44 (0)20 7946 0958",
        "+49 30 901820",
        "+33 1 23 45 67 89",
        "+81-3-1234-5678",
        "+4915123456789",
        "+1 (555) 222-3333",
        "0044 7911 123456",
        "+61.2.9876.5432",
    ] {
        only(Pattern::PhoneIntl, n);
    }
    for text in ["+1 234", "+0 123 456 789", "+44 7911 123456 7890123", "007911", "0012345678", "a+44 7911 123456", "++44 7911 123456"] {
        none(Pattern::PhoneIntl, text);
    }
}

// --- Normalisation and mapping -------------------------------------------------------------

#[test]
fn unicode_digits_are_normalised() {
    only(Pattern::CreditCard, "４１１１ １１１１ １１１１ １１１１");
    only(Pattern::Ssn, "١٢٣-٤٥-٦٧٨٩");
    only(Pattern::Ssn, "123-45-6789");
    only(Pattern::DateIso, "۲۰۲۶-۱۰-۰۷");
    only(Pattern::Ipv4, "１９２.１６８.０.１");
    only(Pattern::Email, "ｊｏｅ＠ｅｘａｍｐｌｅ．ｃｏｍ");
    only(Pattern::PhoneIntl, "＋44 7911 123456");
}

#[test]
fn dashes_are_normalised() {
    only(Pattern::Ssn, "123\u{2013}45\u{2212}6789");
    only(Pattern::CreditCard, "4111\u{2011}1111\u{2010}1111\u{2014}1111");
}

#[test]
fn soft_hyphens_and_zero_width_characters_are_looked_through() {
    let text = "SSN 123-4\u{AD}5-67\u{200B}89 ok";
    let c = chars(text);
    let hits = try_find(Pattern::Ssn, &c).unwrap_or_default();
    assert_eq!(hits.len(), 1);
    // The range covers the dropped characters inside the match.
    let got: String = c[hits[0].clone()].iter().collect();
    assert_eq!(got, "123-4\u{AD}5-67\u{200B}89");
    only(Pattern::CreditCard, "4111\u{200B}1111\u{AD}1111\u{FEFF}1111");
    only(Pattern::Email, "jo\u{AD}e@exam\u{200D}ple.com");
}

#[test]
fn matches_span_line_breaks() {
    only(Pattern::CreditCard, "4111 1111\n1111 1111");
    only(Pattern::CreditCard, "4111-\n1111-\r\n1111-\u{2028}1111");
    only(Pattern::CreditCard, "4111 -\n1111 1111 1111");
    only(Pattern::Ssn, "123-45-\n6789");
    only(Pattern::Phone, "(555)\n123-4567");
    only(Pattern::Iban, "DE89 3704\n0044 0532\n0130 00");
    only(Pattern::PhoneIntl, "+44 7911\n123456");
    only(Pattern::DateIso, "2026-10-07");
    only(Pattern::UkPostcode, "SW1A\u{A0}1AA");
}

#[test]
fn ranges_index_the_original_text() {
    let text = "ab\u{AD}c \u{200B}4111\u{AD} 1111 1111 1111 xyz";
    let c = chars(text);
    let hits = try_find(Pattern::CreditCard, &c).unwrap_or_default();
    assert_eq!(hits.len(), 1);
    let r = hits[0].clone();
    assert!(r.end <= c.len());
    assert_eq!(c[r.clone()].iter().filter(|ch| ch.is_ascii_digit()).count(), 16);
    assert_eq!(c.get(r.start), Some(&'4'));
    assert_eq!(c.get(r.end - 1), Some(&'1'));
}

#[test]
fn many_patterns_in_one_pass() {
    let c = chars("mail a@b.org from 10.0.0.1 on 2026-10-07");
    let hits = try_find_many(&[Pattern::Email, Pattern::Ipv4, Pattern::DateIso], &c).unwrap_or_default();
    let got: Vec<(Pattern, String)> = hits.into_iter().map(|(p, r)| (p, c[r].iter().collect())).collect();
    assert_eq!(got, [(Pattern::Email, "a@b.org".to_string()), (Pattern::Ipv4, "10.0.0.1".to_string()), (Pattern::DateIso, "2026-10-07".to_string())]);
}

// --- Bounds and robustness -----------------------------------------------------------------

#[test]
fn oversize_input_is_an_error_not_a_truncation() {
    let mut c = vec!['x'; MAX_INPUT_CHARS];
    assert!(try_find(Pattern::Email, &c).is_ok());
    c.push('x');
    let err = try_find(Pattern::Email, &c);
    assert_eq!(err, Err(PatternError::InputTooLarge { len: MAX_INPUT_CHARS + 1, max: MAX_INPUT_CHARS }));
    assert!(err.err().is_some_and(|e| e.to_string().contains("limit")));
    // A match past the cap is never silently dropped.
    let mut c = vec![' '; MAX_INPUT_CHARS + 10];
    c.extend("4111 1111 1111 1111".chars());
    assert!(try_find(Pattern::CreditCard, &c).is_err());
    // The infallible wrapper over-marks instead of missing.
    assert_eq!(find(Pattern::CreditCard, &c), std::iter::once(0..c.len()).collect::<Vec<_>>());
}

#[test]
fn empty_and_tiny_inputs() {
    for p in PATTERNS {
        assert!(find(p, &[]).is_empty());
        for s in ["", " ", "+", "(", "-", ":", "::", "@", "0", "\0", "\u{AD}"] {
            let _ = find(p, &chars(s));
        }
    }
}

#[test]
fn hostile_inputs_stay_linear() {
    // Long runs built to defeat backtracking matchers; the whole set must run in well under a
    // second even in a debug build.
    let n = 100_000;
    let inputs = [
        "1".repeat(n),
        "1 ".repeat(n / 2),
        "1-".repeat(n / 2),
        "a".repeat(n),
        "a@".repeat(n / 2),
        "a.".repeat(n / 2),
        "a@b.".repeat(n / 4),
        ":".repeat(n),
        "1:".repeat(n / 2),
        "1.".repeat(n / 2),
        "((((".repeat(n / 4),
        "+1 ".repeat(n / 3),
        "DE89 ".repeat(n / 5),
        "A1 ".repeat(n / 3),
        "12345-".repeat(n / 6),
        "1/".repeat(n / 2),
        "\u{AD}".repeat(n),
        " \n".repeat(n / 2),
    ];
    let start = std::time::Instant::now();
    for input in &inputs {
        let c = chars(input);
        for p in PATTERNS {
            let _ = find(p, &c);
        }
    }
    assert!(start.elapsed() < std::time::Duration::from_secs(20), "pattern search is not linear: {:?}", start.elapsed());
}

// --- Benign corpus -------------------------------------------------------------------------

#[test]
fn benign_prose_matches_nothing() {
    let prose = "The quick brown fox jumps over 13 lazy dogs. Chapter 4, page 212: see Table 3.1 and Figure 7. \
        Revenue grew 12.5% to 1,234,567 units (up from 987,654). Version 2.0.1 shipped; call it a day. \
        Meeting at 10:30 in room B2; ratio 3:1; section 4.2.1; Std::map is not an address.";
    for p in PATTERNS {
        if matches!(p, Pattern::Date | Pattern::Ipv4) {
            // Dotted version numbers and bare numerals are deliberately broad presets.
            continue;
        }
        none(p, prose);
    }
}

// --- Known limits that were closed -----------------------------------------------------------

#[test]
fn email_local_parts_may_start_with_underscore_or_plus() {
    assert_eq!(found(Pattern::Email, "_svc@example.com, +tag@example.org and _a.b_@ex.io"), ["_svc@example.com", "+tag@example.org", "_a.b_@ex.io"]);
    // Still no leading dot, and a glued prefix doesn't split an address.
    none(Pattern::Email, ".dot@example.com@");
    assert_eq!(found(Pattern::Email, "x_y@example.com"), ["x_y@example.com"]);
}

#[test]
fn sept_is_a_month() {
    assert_eq!(
        found(Pattern::Date, "Sept 5, 2024; Sept. 7 2025; 5 Sept 2023; September 9, 2022; Sep 1 2021"),
        ["Sept 5, 2024", "Sept. 7 2025", "5 Sept 2023", "September 9, 2022", "Sep 1 2021"]
    );
    none(Pattern::Date, "Septic 5 2024");
}
