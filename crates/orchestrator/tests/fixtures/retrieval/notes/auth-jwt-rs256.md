# JWT RS256 validation requires the public key in PEM

The `jsonwebtoken` crate's `DecodingKey::from_rsa_pem` expects a
SPKI-format PEM with `-----BEGIN PUBLIC KEY-----` headers, NOT a
raw RSA key (`BEGIN RSA PUBLIC KEY`). Mixing the two surfaces as
an opaque `InvalidKeyFormat` error. The supervisor's JWT validator
in `auth/jwt.rs` normalises both formats before calling decode.
