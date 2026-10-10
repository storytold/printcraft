use super::*;

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

#[test]
fn rc4_reference_vectors() {
    assert_eq!(rc4(b"Key", b"Plaintext"), hex("bbf316e8d940af0ad3"));
    assert_eq!(rc4(b"Wiki", b"pedia"), hex("1021bf0420"));
    assert_eq!(rc4(b"Secret", b"Attack at dawn"), hex("45a01f645fc35b383552544b9bf5"));
}

#[test]
fn aes_cbc_nist_sp800_38a_vectors() {
    let iv: [u8; 16] = hex("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
    let plain = hex("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51");
    let k128 = hex("2b7e151628aed2a6abf7158809cf4f3c");
    let c128 = aes_cbc_encrypt(&k128, &iv, &plain, false);
    assert_eq!(c128, hex("7649abac8119b246cee98e9b12e9197d5086cb9b507219ee95db113a917678b2"));
    assert_eq!(aes_cbc_decrypt(&k128, &iv, &c128, false), plain);
    let k256 = hex("603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4");
    let c256 = aes_cbc_encrypt(&k256, &iv, &plain, false);
    assert_eq!(c256, hex("f58c4c04d6e5f1ba779eabfb5f7bfbd69cfc4e967edb808d679f777bc6702c7d"));
    assert_eq!(aes_cbc_decrypt(&k256, &iv, &c256, false), plain);
}

#[test]
fn aes_padding_is_added_and_tolerantly_removed() {
    let key = [7u8; 16];
    let iv = [1u8; 16];
    for n in [0, 1, 15, 16, 17, 100] {
        let data: Vec<u8> = (0..n as u8).collect();
        let c = aes_cbc_encrypt(&key, &iv, &data, true);
        assert_eq!(c.len() % 16, 0);
        assert_eq!(aes_cbc_decrypt(&key, &iv, &c, true), data);
    }
    // Garbage (bad padding, partial block) decrypts without panicking.
    let _ = aes_cbc_decrypt(&key, &iv, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17], true);
}

const ID: &[u8] = b"0123456789abcdef";

fn new(alg: Algorithm, user: &str, owner: &str, perms: i32) -> SecurityHandler {
    create(
        &NewEncryption { algorithm: alg, user_password: user, owner_password: owner, permissions: perms, encrypt_metadata: true, seed: [42; 32] },
        ID,
    )
    .unwrap()
}

const ALL: [Algorithm; 4] = [Algorithm::Rc4_40, Algorithm::Rc4_128, Algorithm::Aes128, Algorithm::Aes256];

#[test]
fn created_encryption_authenticates_owner_and_user() {
    for alg in ALL {
        let made = new(alg, "user pw", "owner pw", 0b0100); // print only
        let d = made.dict().clone();
        let owner = SecurityHandler::open(d.clone(), ID, Some("owner pw")).unwrap();
        assert_eq!(owner.auth(), Auth::Owner, "{alg:?}");
        assert_eq!(owner.file_key(), made.file_key(), "{alg:?}");
        assert!(owner.permissions().modify(), "owners can do everything");
        let user = SecurityHandler::open(d.clone(), ID, Some("user pw")).unwrap();
        assert_eq!(user.auth(), Auth::User, "{alg:?}");
        assert_eq!(user.file_key(), made.file_key(), "{alg:?}");
        assert!(user.permissions().print() && !user.permissions().modify() && !user.permissions().copy(), "{alg:?}");
        assert_eq!(SecurityHandler::open(d.clone(), ID, Some("nope")).unwrap_err(), CryptError::WrongPassword, "{alg:?}");
        assert_eq!(SecurityHandler::open(d, ID, None).unwrap_err(), CryptError::WrongPassword, "{alg:?}");
    }
}

#[test]
fn empty_user_password_opens_without_asking() {
    for alg in ALL {
        let d = new(alg, "", "secret", -1).dict().clone();
        let h = SecurityHandler::open(d, ID, None).unwrap();
        assert_eq!(h.auth(), Auth::User, "{alg:?}");
    }
}

#[test]
fn strings_and_streams_round_trip_and_are_scrambled() {
    for alg in ALL {
        let h = new(alg, "", "o", -1);
        let text = b"Confidential quarterly numbers".to_vec();
        let enc = h.encrypt_string(12, 0, &text);
        assert_ne!(enc, text, "{alg:?}");
        assert_eq!(h.decrypt_string(12, 0, &enc), text, "{alg:?}");
        if alg != Algorithm::Aes256 {
            // R2–R4 derive a key per object; R6 uses the file key everywhere (§7.6.3.3).
            assert_ne!(h.decrypt_string(13, 0, &enc), text, "object keys differ per object ({alg:?})");
        }
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7) as u8).collect();
        let enc = h.encrypt_stream(5, 0, &data, StreamKind::Normal);
        assert_eq!(h.decrypt_stream(5, 0, &enc, StreamKind::Normal), data, "{alg:?}");
        // Deterministic: the same object encrypts to the same bytes (reproducible saves).
        assert_eq!(h.encrypt_stream(5, 0, &data, StreamKind::Normal), enc);
    }
}

#[test]
fn identity_crypt_filters_leave_data_alone() {
    let mut d = new(Algorithm::Aes256, "", "o", -1).dict().clone();
    // Embedded-file-only encryption: strings and normal streams are not encrypted.
    d.str_f = b"Identity".to_vec();
    d.stm_f = b"Identity".to_vec();
    d.ef_f = b"StdCF".to_vec();
    let h = SecurityHandler::open(d, ID, None).unwrap();
    assert!(!h.encrypts_strings());
    assert_eq!(h.decrypt_stream(3, 0, b"plain", StreamKind::Normal), b"plain");
    let enc = h.encrypt_stream(3, 0, b"attachment", StreamKind::EmbeddedFile);
    assert_ne!(enc, b"attachment");
    assert_eq!(h.decrypt_stream(3, 0, &enc, StreamKind::EmbeddedFile), b"attachment");
    assert_eq!(h.decrypt_stream(3, 0, b"x", StreamKind::Named(b"Identity")), b"x");
}

#[test]
fn r6_passwords_are_saslprepped() {
    // U+00AD (soft hyphen) is mapped to nothing by SASLprep; "ª" normalises to "a" (NFKC).
    let d = new(Algorithm::Aes256, "SªSL\u{AD}prep", "o", -1).dict().clone();
    assert!(SecurityHandler::open(d.clone(), ID, Some("SaSLprep")).is_ok());
    assert!(SecurityHandler::open(d, ID, Some("SaSL-prep")).is_err());
}

#[test]
fn unsupported_handlers_are_reported() {
    let mut d = new(Algorithm::Aes128, "", "o", -1).dict().clone();
    d.filter = b"Adobe.PubSec".to_vec();
    assert!(matches!(SecurityHandler::open(d.clone(), ID, None), Err(CryptError::Unsupported(_))));
    d.filter = b"Standard".to_vec();
    d.r = 7;
    assert!(matches!(SecurityHandler::open(d, ID, None), Err(CryptError::Unsupported(_))));
}

#[test]
fn permission_bits_follow_table_22() {
    let p = Permissions { bits: -3904, owner: false }; // 0xFFFFF0C0: only reserved bits set
    assert!(!p.print() && !p.modify() && !p.copy() && !p.annotate() && !p.assemble());
    let p = Permissions { bits: -1, owner: false };
    assert!(p.print() && p.modify() && p.copy() && p.annotate() && p.assemble() && p.fill_forms());
    let p = Permissions { bits: 0, owner: true };
    assert!(p.modify(), "owners ignore /P");
}

proptest::proptest! {
    #[test]
    fn malformed_dictionaries_never_panic(o in proptest::collection::vec(proptest::num::u8::ANY, 0..60),
                                          u in proptest::collection::vec(proptest::num::u8::ANY, 0..60),
                                          r in 0i64..9, len in -10i64..300) {
        let d = EncryptDict { filter: b"Standard".to_vec(), v: 4, r, length_bits: len, o, u, p: -1, encrypt_metadata: true, ..Default::default() };
        let _ = SecurityHandler::open(d, ID, Some("x"));
    }
}

#[test]
fn owner_authentication_recovers_the_user_password() {
    for alg in [Algorithm::Rc4_40, Algorithm::Rc4_128, Algorithm::Aes128] {
        let d = new(alg, "user pw", "owner pw", -1).dict().clone();
        let h = SecurityHandler::open(d.clone(), ID, Some("owner pw")).unwrap();
        assert_eq!(h.recovered_user_password().as_deref(), Some(&b"user pw"[..]), "{alg:?}");
        assert_eq!(SecurityHandler::open(d, ID, Some("user pw")).unwrap().recovered_user_password(), None);
    }
    let d = new(Algorithm::Rc4_128, "", "owner", -1).dict().clone();
    assert_eq!(SecurityHandler::open(d, ID, Some("owner")).unwrap().recovered_user_password().as_deref(), Some(&b""[..]));
}

/// /O, /U and the first /ID of files qpdf 12.4.2 wrote (`--static-id`), with the file keys
/// `qpdf --show-encryption-key` reports.
fn qpdf_file(r: i64, o: &str, u: &str) -> EncryptDict {
    let mut d = EncryptDict {
        filter: b"Standard".to_vec(),
        v: if r == 4 { 4 } else { 2 },
        r,
        length_bits: 128,
        o: hex(o),
        u: hex(u),
        p: -4,
        encrypt_metadata: true,
        ..Default::default()
    };
    if r == 4 {
        d.crypt_filters = vec![(b"StdCF".to_vec(), Method::Aes128)];
        d.stm_f = b"StdCF".to_vec();
        d.str_f = b"StdCF".to_vec();
    }
    d
}

const QPDF_ID: &str = "56b57383aafa8314f3f98222995f5604";

#[test]
fn legacy_passwords_are_pdfdoc_encoded_with_a_utf8_fallback() {
    let id = hex(QPDF_ID);
    // R3 RC4: user "ılık€" in PDFDocEncoding (ı = 0x9A, € = 0xA0); owner "Öğretmen", which
    // PDFDocEncoding can't hold, so qpdf wrote its UTF-8 bytes.
    let r3 = qpdf_file(
        3,
        "d519aa8120ce8c54812b45e5391916fed167b1c84961d90c4ee4c182ad83534b",
        "4985c03ddfa3eeb8a90c343512913ec40021446990b9e4114071a4d9104984c1",
    );
    let user = SecurityHandler::open(r3.clone(), &id, Some("ılık€")).unwrap();
    assert_eq!((user.auth(), user.file_key().to_vec()), (Auth::User, hex("3564ffcea186122595ad55ee157da71f")));
    let owner = SecurityHandler::open(r3.clone(), &id, Some("Öğretmen")).unwrap();
    assert_eq!((owner.auth(), owner.file_key().to_vec()), (Auth::Owner, hex("3564ffcea186122595ad55ee157da71f")));
    assert_eq!(owner.recovered_user_password(), Some(vec![0x9A, b'l', 0x9A, b'k', 0xA0]));
    // Dropping ı and € (the old Latin-1 conversion) or a near miss doesn't open it.
    for wrong in ["lk", "ilik€", "ılık", "Ogretmen"] {
        assert_eq!(SecurityHandler::open(r3.clone(), &id, Some(wrong)).unwrap_err(), CryptError::WrongPassword, "{wrong}");
    }
    // R4 AES-128: user "şifre" (UTF-8 bytes), owner "owner".
    let r4 = qpdf_file(
        4,
        "bb4f8f4bd2236569e765906caf64e4429a4c20d6e996fdef963e9b5080f9e083",
        "4a965fb91882772cd09ce8bbff2c33270021446990b9e4114071a4d9104984c1",
    );
    let user = SecurityHandler::open(r4.clone(), &id, Some("şifre")).unwrap();
    assert_eq!((user.auth(), user.file_key().to_vec()), (Auth::User, hex("9f19d465cfd8e525d32aa0bdf10acd06")));
    assert_eq!(SecurityHandler::open(r4.clone(), &id, Some("owner")).unwrap().auth(), Auth::Owner);
    assert_eq!(SecurityHandler::open(r4, &id, Some("ifre")).unwrap_err(), CryptError::WrongPassword);
}

#[test]
fn pdfdoc_encoding_round_trips() {
    for b in 0..=255u8 {
        if b != 0x9F {
            assert_eq!(pdfdoc_byte(pdfdoc_char(b)), Some(b), "{b:#04x}");
        }
    }
    assert_eq!(pdfdoc_byte('ı'), Some(0x9A));
    assert_eq!(pdfdoc_byte('€'), Some(0xA0));
    assert_eq!(pdfdoc_byte('ç'), Some(0xE7));
    for c in ['ğ', 'ş', 'İ', '\u{A0}', '\u{85}', 'Ж', '日'] {
        assert_eq!(pdfdoc_byte(c), None, "{c}");
    }
    assert_eq!(legacy_candidates("Çok"), [vec![0xC7, b'o', b'k']]);
    assert_eq!(legacy_candidates("şifre"), [b"ifre".to_vec(), "şifre".as_bytes().to_vec()]);
}
