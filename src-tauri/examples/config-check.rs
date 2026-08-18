//! `cargo run --example config-check`
//!
//! Reports which providers this build would consider configured, and which
//! `.env` file it read, without launching the UI.
//!
//! SECURITY: prints only "set" / "not set". It never prints a value, so the
//! output is safe to paste into a bug report.

use media_downloader_lib::config;

const KEYS: &[(&str, &[&str])] = &[
    ("Google", &["GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET"]),
    (
        "Facebook",
        &[
            "FACEBOOK_CLIENT_ID",
            "FACEBOOK_CLIENT_SECRET",
            "FACEBOOK_REDIRECT_URI",
        ],
    ),
    (
        "Instagram",
        &[
            "INSTAGRAM_CLIENT_ID",
            "INSTAGRAM_CLIENT_SECRET",
            "INSTAGRAM_REDIRECT_URI",
        ],
    ),
    (
        "TikTok",
        &["TIKTOK_CLIENT_KEY", "TIKTOK_CLIENT_SECRET"],
    ),
];

/// Keys without which a provider cannot authorize at all.
const REQUIRED: &[&str] = &[
    "GOOGLE_CLIENT_ID",
    "FACEBOOK_CLIENT_ID",
    "FACEBOOK_CLIENT_SECRET",
    "FACEBOOK_REDIRECT_URI",
    "TIKTOK_CLIENT_KEY",
    "TIKTOK_CLIENT_SECRET",
    "INSTAGRAM_CLIENT_ID",
    "INSTAGRAM_CLIENT_SECRET",
    "INSTAGRAM_REDIRECT_URI",
];

fn main() {
    match config::load_dotenv() {
        Some(path) => println!("env file : {}", path.display()),
        None => println!("env file : none found (using process environment only)"),
    }

    for (provider, keys) in KEYS {
        let required: Vec<_> = keys.iter().filter(|k| REQUIRED.contains(k)).collect();
        let ready = required.iter().all(|k| config::read(k).is_some());

        println!("\n{provider}  {}", if ready { "READY" } else { "not configured" });
        for key in *keys {
            let set = config::read(key).is_some();
            let tag = if !REQUIRED.contains(key) {
                "optional"
            } else if set {
                "required"
            } else {
                "REQUIRED"
            };
            println!("  [{}] {key:<24} ({tag})", if set { "x" } else { " " });
        }
    }
}
