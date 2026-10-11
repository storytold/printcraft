# pdfcraft-sign

Layer L4: digital signatures (execution plan M9; ISO 32000-2 §12.8; PAdES, ETSI EN 319 142).

```rust
let id = pkcs12::open(&std::fs::read("me.p12")?, "password")?;          // a digital ID
let signed = sign(&doc, &id, &SignOptions { page: 0, rect: Some(r), date, ..Default::default() })?;
for s in signatures(&doc, &bytes, &trust) {                                // list + validate
    println!("{}: {} — {:?}", s.field, s.summary(), s.details);
}
```

- **Formats, on RustCrypto primitives:** a small DER reader and writer (`der`), X.509
  certificates (`x509`), CMS SignedData (`cms`), and PKCS #12 digital ID files (`pkcs12`):
  PBES2 (PBKDF2 + AES/3DES), the legacy SHA-1 3DES/RC2 schemes, and MAC checks. Writing uses
  OpenSSL 3's defaults (AES-256-CBC, HMAC-SHA-256).
- **Keys:** RSA, ECDSA P-256 and P-384. Verification uses RustCrypto everywhere. RSA
  private-key operations (signing, key generation) use `aws-lc-rs` on native targets and are
  refused in the browser (ADR-0009: the `rsa` crate's Marvin advisory, RUSTSEC-2023-0071, has
  no fix, so `rsa` is only used to verify). ECDSA signs with deterministic nonces (RFC 6979).
- **Signing:** PAdES B-B (`ETSI.CAdES.detached`, signing-certificate-v2, SHA-256/384, no SHA-1).
  An existing unsigned field or a new one (visible with Acrobat's name-and-details appearance,
  or invisible); certification with DocMDP P=1/2/3. The document is written incrementally
  with a zero-filled `/Contents` and fixed-width `/ByteRange`, which are then patched in
  place. Encrypted documents are refused for now.
- **Timestamps (PAdES B-T):** the `timestamp` module builds and parses RFC 3161 requests,
  responses and TSTInfo tokens (size-capped, imprint- and signature-checked). `sign_with_timestamp`
  attaches the TSA's token as an unsigned attribute over the signature value; the transport is
  the caller's (`TimestampAuthority` — this crate never opens a socket). Validation verifies
  embedded tokens; when the TSA chains to the trust store it reports the trusted time in
  `SignatureInfo::timestamp_time`, which then anchors certificate-validity and revocation
  checks. An untrusted TSA's time is reported as an unverified timestamp and never used as the
  validation time (the signer's claimed time is). `timestamp::respond` is the TSA-side signer behind the
  deterministic test authority.
- **Document timestamps and LTV:** `timestamp_document` appends a standalone RFC 3161
  document timestamp (`/ETSI.RFC3161`) covering the whole file; validation discovers and
  verifies these dictionaries (imprint over the signed bytes, token signature, TSA cert
  validity). `dss::embed` merges revocation evidence into the catalog's `/DSS` with `/VRI`
  entries keyed per signature (uppercase-hex SHA-1 of `/Contents`), deduplicating
  byte-identical blobs — sign → DSS → timestamp makes a B-LTA file, and the change classifier
  treats the store as a permitted change: only objects reached through `/Certs`, `/CRLs`,
  `/OCSPs` and `/VRI` with the shape of their role, and new or only grown since the signature,
  count (a `/DSS` entry naming a page's contents, or a `/Type /DSS` label, does not).
  `revocation` parses and verifies RFC 5280 CRLs and
  RFC 6960 OCSP responses (responder identity, OCSP-signing EKU for delegated responders,
  CertID hash matching, validity windows); validation checks embedded evidence against the
  signer's chain and a verified revocation invalidates the signature.

Not yet: revocation fetching (AIA/CRLDP extraction and a fetcher), timestamp-server
configuration, FieldMDP locks, certificate security, direct PKCS #11 access to tokens.
macOS Keychain and Windows Current User Personal (My) store identities sign through
`ExternalKey` without exporting private keys. Windows CNG supports RSA PKCS #1 v1.5 and
ECDSA P-256/P-384; the store integration is tested with software-backed keys. A smart card
or token whose driver registers a CNG key storage provider is such an identity too: the key
is opened for signing without the silent flag, so the provider shows its own PIN dialog
(checked by hand with a SafeNet token; CI has no hardware). A store identity signs with the
issuers Windows chains its certificate to, without the root. Store certificates that can't
sign (no private key, another key type, a key CNG can't open) are returned by
`windows::list` as `unusable` with the reason and the Windows error code, shown greyed in
the Sign dialog and by `sign_windows_ids`, instead of being left out silently; a key that
needs a prompt to open is listed and asks at signing (issue #179).
- **Validation:** `/ByteRange` and the CMS are read from the file's own bytes; the digest,
  the signature value and the signer's chain (against a `TrustStore`) are checked. Later
  revisions are diffed against the signed one, and the changes are classified (signing, form
  fill, comments, metadata, page content, document structure) under the DocMDP permissions.
  The verdict follows Acrobat: valid, unknown (intact but the identity isn't trusted) or invalid.
- **Chains are checked, not just linked.** `x509::build_chain` only takes an issuer whose key
  verifies the certificate below it *and* that may issue: `basicConstraints` CA:TRUE (an old v1
  self-signed root without constraints counts), `keyCertSign` when it has a key usage, a
  `pathLenConstraint` that allows the CAs below it, and, given the signing time, validity then.
  Certificates embedded in a document are in the pool too, so an ordinary subscriber certificate
  can never vouch for another one; when an issuer is refused, the signature's details say why.
- **The signer's certificate must allow signing documents** (`Certificate::signing_problem`): a
  key usage needs digitalSignature or nonRepudiation, an extended key usage needs a
  document-signing purpose (any, emailProtection, codeSigning, RFC 9336 documentSigning, Adobe
  Authentic Documents, Microsoft document signing), and no critical extension may be one PdfCraft
  doesn't process. Otherwise the verdict is unknown, never valid, and the details say why.
- **BER as well as DER.** The CMS is read with `der::Tlv::parse_ber`, as Windows CryptoAPI, Adobe
  PPKMS, DocuSign, Documenso and `openssl cms -stream` write it: indefinite lengths and an
  OCTET STRING split into segments. Only the structure is read that way; certificates and signed
  attributes are still verified over their own exact bytes (signed attributes written with an
  indefinite length are re-encoded as DER first, as RFC 5652 §5.4 has them signed). Nesting is
  bounded, and the details say that the signature was BER. High tag numbers are not read.
- **Later changes are classified narrowly.** Only the catalog's own `/Metadata` stream is the
  document's XMP, and only objects reached through the `/DSS` keys above are the security store. A
  `/Type /Metadata` or `/Type /DSS` *label* on any other dictionary or stream (a page's Form
  XObject, say) makes nothing permitted, at every DocMDP level.
- **Strict in what it checks, tolerant where signers differ.** RSA PKCS #1 signatures are checked
  against the one encoding the digest should have, built here and compared byte for byte; the
  `DigestInfo` may omit its NULL parameter, as older signers write it (RFC 8017 App. B.1), but a
  signature over the bare digest, which doesn't say which hash it uses, is refused. A signature
  value must be below the modulus (RFC 8017 §5.2.2). RSA-PSS uses the salt length it declares.
  ECDSA is read as DER, non-minimal DER or raw `r ‖ s`. `adbe.x509.rsa_sha1` is validated. What the
  check tolerated is listed in the signature's details: a DigestInfo without its NULL, an RSA value
  that isn't modulus-length, ECDSA that isn't canonical DER, a SignerInfo whose digest algorithm
  differs from the one the signature algorithm names. What PdfCraft can't check (an unknown
  algorithm or curve) is *unknown*, never *invalid*.
- **Weak algorithms still validate, and say so.** SHA-1 and RIPEMD-160 signatures are common in
  documents signed years ago, so they validate (new signatures never use SHA-1), but the signature's
  details say that the algorithm is weak and shouldn't be relied on.
- **Validation algorithms:** digests SHA-1, SHA-224/256/384/512, SHA-512/224, SHA-512/256,
  SHA3-224/256/384/512 and RIPEMD-160. RSA PKCS #1 v1.5 and RSASSA-PSS (the hash, MGF1 hash and
  salt length the signature declares), ECDSA on P-256, P-384, P-521, brainpoolP256r1, P384r1 and
  P512r1 (also BSI "plain" ECDSA), and Ed25519 (RFC 8419, over the signed attributes).
  Signing: RSA, P-256, P-384 with SHA-256/384/512. The RSA padding checks and brainpoolP512r1 are
  written here (`rsa_pad`, `ec512`) on `crypto-bigint`, because the typed APIs of the `rsa` crate
  and RustCrypto don't cover them.

Oracles: poppler's `pdfsig` reports our signatures valid; OpenSSL reads our `.p12` files and
verifies our CMS; `tests/data/openssl-signed.pdf` is a signature OpenSSL made, which we validate.

## e-Aadhaar verification (`cca`, `verify`)

Official e-Aadhaar PDFs carry a UIDAI signature that chains to India's Controller of
Certifying Authorities (CCA) roots. Most viewers show `?` for it because they do not ship
those roots. PdfCraft bundles them and verifies natively — no separate app, no upload:

- **Trust bundle** (`cca`): `CCA India 2022` (2022-02-02 – 2042-02-02, SHA-256
  `9A:3F:D3:17:…:A9:4C:D8`) and `CCA India 2022 SPL` (2022-09-20 – 2042-09-20, SHA-256
  `B7:24:68:9B:…:D8:C9:1A`), public roots from <https://cca.gov.in/root_certificate.html>,
  cross-checked against the MIT-licensed `eaadhaar-pdf-signature-verifier` repository's
  copies (only the certificates are reused; the check itself is this crate's own
  validation, the same family as that tool's pyHanko checks).
- **Reports** (`verify`): one per signature field with *separate* verdicts — integrity
  (intact / altered / unknown-unsupported), trust (trusted, incl. whether the anchor is
  a CCA root, or untrusted), certificate validity at the signing time, revocation from
  embedded evidence (good / revoked / **unknown**: offline verification cannot fetch
  CRLs/OCSP), signing-time source (signer's clock claim vs validated timestamp), later
  changes (none / allowed / disallowed), and recommended next steps. A matching subject
  name alone never counts as trusted (tested with a spoofed `CN=CCA India 2022`).
- **Use it:** fresh sessions already trust the bundle, so genuine e-Aadhaar PDFs
  validate green on open. The Signatures panel shows *CCA India roots trusted*, or
  *Verify with CCA India roots* to re-add them after the store was cleared; headless
  `sign_verify` takes `trust: "both"` (default), `"session"`, or `"cca"` — see
  `Session::verify_signatures` and `VerifyTrust`. Encrypted e-Aadhaar PDFs open with
  their user password first.
- **Canvas stamp:** a successfully verified, trusted signature gets the reference
  verifier's Adobe-style stamp as a viewer-only overlay: opaque white cover over the
  widget's baked appearance, the green tick ribbon (#00a651 with black shadow and
  outline, triangulated from the reference polygon), the "Signature valid" title, and
  signer identity + signing-date lines scaled to the widget. Anything else keeps a
  distinct warning plate (untrusted, invalid, modified, or not yet checkable).

Limitations: revocation needs evidence embedded in the file (`/DSS`); without it the
report says unknown and suggests an online OCSP/CRL check for high assurance.
Timestamps from untrusted authorities are reported but never used as validation time.
