//! Licence key tool. Run from `src-tauri/`.
//!
//!   One-time setup - create your signing keypair:
//!     cargo run --example license-keygen -- --init
//!
//!   Issue a key:
//!     LICENSE_SIGNING_KEY=... cargo run --example license-keygen -- \
//!        --ref buyer@example.com --plan pro --expires 2027-01-01
//!
//! THE PRIVATE KEY IS THE WHOLE SYSTEM. Anyone holding it can mint unlimited
//! keys for your app. It must never be committed, never be shipped, and never
//! be pasted anywhere the app can read it. Only `LICENSE_PUBLIC_KEY` belongs in
//! `.env` and in CI secrets.

use data_encoding::BASE32_NOPAD;
use ed25519_dalek::{Signer, SigningKey};
use media_downloader_lib::license::{customer_tag, format_key, License, Plan};
use time::{Date, Month, OffsetDateTime, Time};

fn main() {
    // Load .env so LICENSE_PUBLIC_KEY is visible. Without this the keypair
    // cross-check below silently degrades to "not set" and cannot catch a
    // mismatched signing key - which is the one mistake it exists to catch.
    let _ = media_downloader_lib::config::load_dotenv();

    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--init") {
        init();
        return;
    }
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        usage();
        return;
    }

    // Support path: check a key a customer says is not working, using the same
    // verifier the app uses. Needs LICENSE_PUBLIC_KEY, not the private key.
    if let Some(key) = flag(&args, "--verify") {
        match media_downloader_lib::license::verify(&key) {
            Ok(l) => {
                println!("valid");
                println!("  plan    : {}", l.plan.as_str());
                println!(
                    "  expires : {}",
                    l.expires_at.map_or("never".into(), |e| e.to_string()),
                );
                println!("  tag     : {}", l.tag_hex());
            }
            Err(e) => {
                println!("rejected: {} ({})", e, e.code());
                std::process::exit(1);
            }
        }
        return;
    }

    let reference = match flag(&args, "--ref") {
        Some(r) if !r.trim().is_empty() => r,
        _ => {
            eprintln!("error: --ref is required (an email or order id you can look up later)");
            std::process::exit(2);
        }
    };

    let plan = match flag(&args, "--plan").as_deref() {
        None | Some("standard") => Plan::Standard,
        Some("pro") => Plan::Pro,
        Some(other) => {
            eprintln!("error: unknown --plan {other:?} (expected: standard, pro)");
            std::process::exit(2);
        }
    };

    let expires_at = match flag(&args, "--expires") {
        None => None,
        Some(d) => Some(parse_date(&d).unwrap_or_else(|| {
            eprintln!("error: --expires must look like 2027-01-31");
            std::process::exit(2);
        })),
    };

    let signing = load_signing_key();
    guard_key_pair_matches(&signing);

    let license = License {
        plan,
        expires_at,
        customer_tag: customer_tag(&reference),
    };

    // Sign through the same encoder the app verifies with; see `format_key`.
    let payload = license_payload(&license);
    let signature = signing.sign(&payload);
    let key = format_key(&payload, &signature.to_bytes());

    println!("customer : {reference}");
    println!("plan     : {}", plan.as_str());
    println!(
        "expires  : {}",
        expires_at.map_or("never".to_string(), |e| format!("{e} (unix)")),
    );
    println!("tag      : {}", license.tag_hex());
    println!();
    println!("{key}");
}

/// The app owns the payload layout, so go through a round trip rather than
/// duplicating the byte order here - a divergence would only surface as
/// "invalid key" for a paying customer.
fn license_payload(license: &License) -> Vec<u8> {
    media_downloader_lib::license::encode_payload_for_signing(license)
}

fn init() {
    use rand::RngCore;
    let mut seed = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut seed);
    let signing = SigningKey::from_bytes(&seed);

    println!("Keypair generated.\n");
    println!("PRIVATE - keep this secret, store it in a password manager.");
    println!("Never commit it and never put it in .env:\n");
    println!("  LICENSE_SIGNING_KEY={}\n", BASE32_NOPAD.encode(&signing.to_bytes()));
    println!("PUBLIC - safe to embed. Put this in .env and in your CI secrets:\n");
    println!(
        "  LICENSE_PUBLIC_KEY={}\n",
        BASE32_NOPAD.encode(signing.verifying_key().as_bytes())
    );
    println!("Keys already issued stop verifying if you replace this pair, so");
    println!("generate it once and back it up.");
}

fn load_signing_key() -> SigningKey {
    let raw = std::env::var("LICENSE_SIGNING_KEY").unwrap_or_default();
    if raw.trim().is_empty() {
        eprintln!("error: LICENSE_SIGNING_KEY is not set. Run with --init to create one.");
        std::process::exit(2);
    }
    let bytes = BASE32_NOPAD
        .decode(raw.trim().to_ascii_uppercase().as_bytes())
        .unwrap_or_else(|_| {
            eprintln!("error: LICENSE_SIGNING_KEY is not valid base32");
            std::process::exit(2);
        });
    let arr: [u8; 32] = bytes.as_slice().try_into().unwrap_or_else(|_| {
        eprintln!("error: LICENSE_SIGNING_KEY must decode to 32 bytes");
        std::process::exit(2);
    });
    SigningKey::from_bytes(&arr)
}

/// Refuse to issue keys the app cannot verify.
///
/// It is easy to paste the PUBLIC key into `LICENSE_SIGNING_KEY` - they look
/// alike and sit next to each other in the `--init` output. Ed25519 accepts any
/// 32 bytes as a seed, so that mistake does not error: it silently mints keys
/// signed by a different pair, and every customer gets "that key isn't valid".
///
/// If a public key is configured, check the pair agrees before signing anything.
fn guard_key_pair_matches(signing: &SigningKey) {
    let Some(configured) = media_downloader_lib::config::read("LICENSE_PUBLIC_KEY") else {
        eprintln!("note: LICENSE_PUBLIC_KEY is not set, so the keypair could not be");
        eprintln!("      cross-checked. Make sure your build uses the public key");
        eprintln!("      that matches this signing key.\n");
        return;
    };

    let derived = BASE32_NOPAD.encode(signing.verifying_key().as_bytes());
    if derived.eq_ignore_ascii_case(configured.trim()) {
        return;
    }

    eprintln!("error: LICENSE_SIGNING_KEY does not match LICENSE_PUBLIC_KEY.\n");
    eprintln!("  the app trusts     : {}", configured.trim());
    eprintln!("  this key would sign: {derived}\n");
    eprintln!("Every key issued with this signing key would be rejected.");
    eprintln!("You have most likely pasted the PUBLIC key into LICENSE_SIGNING_KEY.");
    eprintln!("Use the value printed as PRIVATE by `--init`.");
    std::process::exit(2);
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

/// `YYYY-MM-DD`, interpreted as the very start of that day in UTC, so a key
/// dated 2027-01-31 stops working as that day begins.
fn parse_date(s: &str) -> Option<i64> {
    let mut parts = s.trim().split('-');
    let y: i32 = parts.next()?.parse().ok()?;
    let m: u8 = parts.next()?.parse().ok()?;
    let d: u8 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let date = Date::from_calendar_date(y, Month::try_from(m).ok()?, d).ok()?;
    Some(OffsetDateTime::new_utc(date, Time::MIDNIGHT).unix_timestamp())
}

fn usage() {
    println!("Licence key tool\n");
    println!("  --init                      create a signing keypair (run once)");
    println!("  --ref <email|order-id>      who the key is for   [required]");
    println!("  --plan <standard|pro>       default: standard");
    println!("  --expires <YYYY-MM-DD>      default: never expires");
    println!("  --verify <key>              check a key (needs LICENSE_PUBLIC_KEY)");
    println!("\nIssuing needs LICENSE_SIGNING_KEY in the environment.");
}
