//! Offline licence keys.
//!
//! A key is a small payload plus an Ed25519 signature over it, base32-encoded.
//! Only the PUBLIC key is compiled into the application, so a key cannot be
//! forged without the private key, which never leaves the developer's machine.
//!
//! WHAT THIS DOES AND DOES NOT DO. Verification runs on the user's computer, so
//! a determined person can patch the binary and skip it. That is true of every
//! client-side licence check and is not solvable by making this code cleverer.
//! What it does achieve: a key cannot be invented, a key cannot be edited (the
//! signature covers plan and expiry), and an expired key stops working.
//!
//! What it deliberately does NOT do is detect the same key being used on many
//! machines - that needs a server to count activations. The payload carries a
//! version byte so an online check can be added later without invalidating keys
//! already issued.

pub mod store;

use data_encoding::BASE32_NOPAD;
use ed25519_dalek::{Signature, Verifier, VerifyingKey, PUBLIC_KEY_LENGTH, SIGNATURE_LENGTH};
use serde::{Deserialize, Serialize};

use crate::auth::now_unix;
use crate::config;
use crate::errors::{AppError, Result};

/// Current payload layout version.
const PAYLOAD_VERSION: u8 = 1;

/// version(1) + plan(1) + expires_at(8) + customer_tag(4)
const PAYLOAD_LEN: usize = 14;

/// Human-facing prefix, so a key is recognisable when pasted into a support
/// email and cannot be confused with an API token.
const KEY_PREFIX: &str = "SMD1";

/// Groups of this many characters, dash-separated, purely for legibility.
const GROUP: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Plan {
    Standard,
    Pro,
}

impl Plan {
    fn to_byte(self) -> u8 {
        match self {
            Plan::Standard => 0,
            Plan::Pro => 1,
        }
    }

    fn from_byte(b: u8) -> Result<Self> {
        match b {
            0 => Ok(Plan::Standard),
            1 => Ok(Plan::Pro),
            // An unknown plan means a key issued by a NEWER build. Refuse
            // rather than silently downgrade the customer to Standard.
            _ => Err(AppError::LicenseUnsupported),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Plan::Standard => "Standard",
            Plan::Pro => "Pro",
        }
    }
}

/// The verified contents of a licence key. Safe to show the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct License {
    pub plan: Plan,
    /// Unix seconds, or `None` for a perpetual licence.
    pub expires_at: Option<i64>,
    /// First 4 bytes of SHA-256 over whatever reference you issued the key
    /// against (an email, an order id). Enough to match a key to a customer in
    /// your own records without putting their address in the key.
    pub customer_tag: [u8; 4],
}

impl License {
    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(exp) => now_unix() >= exp,
            None => false,
        }
    }

    /// Short identifier to quote in support conversations.
    pub fn tag_hex(&self) -> String {
        self.customer_tag.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn encode_payload(&self) -> [u8; PAYLOAD_LEN] {
        let mut out = [0u8; PAYLOAD_LEN];
        out[0] = PAYLOAD_VERSION;
        out[1] = self.plan.to_byte();
        out[2..10].copy_from_slice(&self.expires_at.unwrap_or(0).to_be_bytes());
        out[10..14].copy_from_slice(&self.customer_tag);
        out
    }

    fn decode_payload(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != PAYLOAD_LEN {
            return Err(AppError::LicenseMalformed);
        }
        if bytes[0] != PAYLOAD_VERSION {
            return Err(AppError::LicenseUnsupported);
        }
        let plan = Plan::from_byte(bytes[1])?;
        let raw_exp = i64::from_be_bytes(bytes[2..10].try_into().map_err(|_| AppError::LicenseMalformed)?);
        let mut tag = [0u8; 4];
        tag.copy_from_slice(&bytes[10..14]);

        Ok(License {
            plan,
            expires_at: (raw_exp != 0).then_some(raw_exp),
            customer_tag: tag,
        })
    }
}

/// Expose the payload encoder to the key-issuing tool.
///
/// The tool must sign exactly the bytes this module later verifies; letting it
/// build its own copy of the layout would turn any future change into "invalid
/// key" for real customers.
pub fn encode_payload_for_signing(license: &License) -> Vec<u8> {
    license.encode_payload().to_vec()
}

/// Derive the 4-byte tag stored in a key from a customer reference.
pub fn customer_tag(reference: &str) -> [u8; 4] {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(reference.trim().to_lowercase().as_bytes());
    let mut tag = [0u8; 4];
    tag.copy_from_slice(&digest[..4]);
    tag
}

/// Render a signed key for distribution.
///
/// Lives here rather than in the key-issuing tool so that the format has one
/// definition; a mismatch between issuing and verifying would only show up as
/// "invalid key" for real customers.
pub fn format_key(payload: &[u8], signature: &[u8]) -> String {
    let mut blob = Vec::with_capacity(payload.len() + signature.len());
    blob.extend_from_slice(payload);
    blob.extend_from_slice(signature);

    let body = BASE32_NOPAD.encode(&blob);
    let grouped: Vec<String> = body
        .as_bytes()
        .chunks(GROUP)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect();

    format!("{KEY_PREFIX}-{}", grouped.join("-"))
}

/// Strip formatting so a pasted key survives dashes, spaces and lower case.
fn normalise(raw: &str) -> Result<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_uppercase();

    let body = cleaned
        .strip_prefix(KEY_PREFIX)
        .ok_or(AppError::LicenseMalformed)?;
    if body.is_empty() {
        return Err(AppError::LicenseMalformed);
    }
    Ok(body.to_string())
}

/// The public half of the signing key, as base32.
///
/// Absent in a development build, which disables licensing entirely - see
/// [`is_enforced`]. A release build must supply it.
fn verifying_key() -> Option<VerifyingKey> {
    let encoded = config::read("LICENSE_PUBLIC_KEY")?;
    let bytes = BASE32_NOPAD
        .decode(encoded.trim().to_ascii_uppercase().as_bytes())
        .ok()?;
    let arr: [u8; PUBLIC_KEY_LENGTH] = bytes.try_into().ok()?;
    VerifyingKey::from_bytes(&arr).ok()
}

/// Whether this build checks licences at all.
///
/// False when no public key was compiled in, so `npm run tauri dev` is not
/// gated behind a key. Release builds set `LICENSE_PUBLIC_KEY`.
pub fn is_enforced() -> bool {
    // Escape hatch: a build can ship licence-free — usable with no key — even
    // when a public key is compiled in, by setting LICENSE_DISABLED=true. For
    // free or internal builds.
    if let Some(v) = config::read("LICENSE_DISABLED") {
        let v = v.trim().to_ascii_lowercase();
        if v == "1" || v == "true" || v == "yes" {
            return false;
        }
    }
    verifying_key().is_some()
}

/// Verify a pasted key and return what it grants.
///
/// SECURITY: the signature is checked before any field is trusted, and an
/// expired licence is rejected here rather than by the caller, so no code path
/// can accidentally honour one.
pub fn verify(raw_key: &str) -> Result<License> {
    let key = verifying_key().ok_or(AppError::LicenseNotConfigured)?;

    let body = normalise(raw_key)?;
    let blob = BASE32_NOPAD
        .decode(body.as_bytes())
        .map_err(|_| AppError::LicenseMalformed)?;

    if blob.len() != PAYLOAD_LEN + SIGNATURE_LENGTH {
        return Err(AppError::LicenseMalformed);
    }
    let (payload, sig_bytes) = blob.split_at(PAYLOAD_LEN);

    let sig_arr: [u8; SIGNATURE_LENGTH] =
        sig_bytes.try_into().map_err(|_| AppError::LicenseMalformed)?;
    let signature = Signature::from_bytes(&sig_arr);

    key.verify(payload, &signature)
        .map_err(|_| AppError::LicenseInvalid)?;

    let license = License::decode_payload(payload)?;
    if license.is_expired() {
        return Err(AppError::LicenseExpired);
    }
    Ok(license)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    /// Deterministic keypair so tests do not depend on the environment.
    fn test_keys() -> (SigningKey, String) {
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let public = BASE32_NOPAD.encode(signing.verifying_key().as_bytes());
        (signing, public)
    }

    fn issue(signing: &SigningKey, license: &License) -> String {
        let payload = license.encode_payload();
        let sig = signing.sign(&payload);
        format_key(&payload, &sig.to_bytes())
    }

    /// `LICENSE_PUBLIC_KEY` is process-global, and cargo runs tests in
    /// parallel. Without this lock, a test that clears the key races one that
    /// sets it: `verify` then gets past the "not configured" branch and fails
    /// later as `license_malformed`, so the suite fails intermittently on an
    /// assertion that has nothing to do with the bug.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Take the lock even if a previous test panicked while holding it - the
    /// data it guards is the environment, which we set explicitly anyway.
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn with_public_key<T>(public: &str, f: impl FnOnce() -> T) -> T {
        let _guard = env_guard();
        std::env::remove_var("LICENSE_DISABLED");
        std::env::set_var("LICENSE_PUBLIC_KEY", public);
        let out = f();
        std::env::remove_var("LICENSE_PUBLIC_KEY");
        out
    }

    fn perpetual() -> License {
        License {
            plan: Plan::Pro,
            expires_at: None,
            customer_tag: customer_tag("buyer@example.com"),
        }
    }

    #[test]
    fn a_genuine_key_round_trips() {
        let (signing, public) = test_keys();
        let key = issue(&signing, &perpetual());
        with_public_key(&public, || {
            let got = verify(&key).expect("should verify");
            assert_eq!(got.plan, Plan::Pro);
            assert!(got.expires_at.is_none());
            assert_eq!(got.customer_tag, customer_tag("Buyer@Example.com "));
        });
    }

    #[test]
    fn formatting_is_forgiving_when_pasted_back() {
        let (signing, public) = test_keys();
        let key = issue(&signing, &perpetual());
        let mangled = format!("  {}  ", key.replace('-', " ").to_lowercase());
        with_public_key(&public, || {
            assert!(verify(&mangled).is_ok(), "a pasted key must survive spacing and case");
        });
    }

    #[test]
    fn a_key_from_another_signer_is_rejected() {
        let (_, public) = test_keys();
        let impostor = SigningKey::from_bytes(&[9u8; 32]);
        let key = issue(&impostor, &perpetual());
        with_public_key(&public, || {
            assert_eq!(verify(&key).unwrap_err().code(), "license_invalid");
        });
    }

    /// The signature covers the payload, so upgrading yourself to Pro by
    /// editing the key breaks it.
    #[test]
    fn tampering_with_the_plan_invalidates_the_key() {
        let (signing, public) = test_keys();
        let mut license = perpetual();
        license.plan = Plan::Standard;
        let payload = license.encode_payload();
        let sig = signing.sign(&payload);

        let mut forged = payload;
        forged[1] = Plan::Pro.to_byte();
        let key = format_key(&forged, &sig.to_bytes());

        with_public_key(&public, || {
            assert_eq!(verify(&key).unwrap_err().code(), "license_invalid");
        });
    }

    #[test]
    fn an_expired_key_is_refused() {
        let (signing, public) = test_keys();
        let key = issue(
            &signing,
            &License {
                plan: Plan::Standard,
                expires_at: Some(now_unix() - 60),
                customer_tag: [0; 4],
            },
        );
        with_public_key(&public, || {
            assert_eq!(verify(&key).unwrap_err().code(), "license_expired");
        });
    }

    #[test]
    fn a_key_valid_for_another_hour_is_accepted() {
        let (signing, public) = test_keys();
        let key = issue(
            &signing,
            &License {
                plan: Plan::Standard,
                expires_at: Some(now_unix() + 3600),
                customer_tag: [0; 4],
            },
        );
        with_public_key(&public, || assert!(verify(&key).is_ok()));
    }

    #[test]
    fn junk_is_rejected_without_panicking() {
        let (_, public) = test_keys();
        with_public_key(&public, || {
            for junk in ["", "SMD1", "hello world", "SMD1-!!!!!!", "XXXX-AAAAAA"] {
                assert!(verify(junk).is_err(), "accepted junk: {junk:?}");
            }
        });
    }

    #[test]
    fn licensing_is_off_when_no_public_key_is_compiled_in() {
        let _guard = env_guard();
        std::env::remove_var("LICENSE_PUBLIC_KEY");
        // A free build sets LICENSE_DISABLED; it must not leak into this test.
        std::env::remove_var("LICENSE_DISABLED");

        // A release build bakes the key in with `option_env!`, so "no key
        // configured" is not reachable there - removing the runtime variable
        // cannot unset a compile-time one. CI sets LICENSE_PUBLIC_KEY for the
        // whole job, so this test has to describe both kinds of build rather
        // than assuming the developer's.
        if option_env!("LICENSE_PUBLIC_KEY").is_some() {
            assert!(
                is_enforced(),
                "a build with the key compiled in must enforce licensing"
            );
            return;
        }

        assert!(!is_enforced(), "dev builds must not be gated");
        assert_eq!(verify("SMD1-AAAAAA").unwrap_err().code(), "license_not_configured");
    }

    #[test]
    fn a_key_is_pasteable_rather_than_typable() {
        let (signing, _) = test_keys();
        let key = issue(&signing, &perpetual());
        assert!(key.starts_with("SMD1-"));
        // Ed25519 signatures are 64 bytes, so the key is long by construction.
        // Worth asserting so nobody "shortens" it by truncating the signature.
        assert!(key.len() > 100, "unexpectedly short key: {}", key.len());
    }
}
