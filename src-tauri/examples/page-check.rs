//! `cargo run --example page-check -- <serial>`
//!
//! Reads the Facebook Pages an instance can post as, straight from the app's
//! own profile switcher, and prints them. The same code path the dashboard's
//! "Find Pages" button runs, without launching the UI.
//!
//! Run this when the dashboard shows no Pages for an account that has some.
//! It answers the one question the GUI cannot: whether the switcher opened and
//! what was actually on it.
//!
//! ```text
//! cargo run --example page-check -- emulator-5554
//! ```
//!
//! SECURITY: drives menus and reads labels. It never types into a field, never
//! taps a composer, and cannot post anything. No credential is involved: the
//! session lives inside the Android app and is not read here.

use media_downloader_lib::ldplayer::adb::Adb;
use media_downloader_lib::publish::connector::pages;

#[tokio::main]
async fn main() {
    let serial = std::env::args().nth(1).unwrap_or_else(|| "emulator-5554".into());

    let Some(adb) = Adb::discover(None, Some(std::path::Path::new(r"C:\LDPlayer\LDPlayer14")))
    else {
        eprintln!("no adb found — pass LDPlayer's folder or put adb on PATH");
        std::process::exit(1);
    };

    println!("Reading Pages from {serial}…\n");
    match pages::discover(&adb, &serial).await {
        Ok(found) => {
            match found.profile.as_deref() {
                Some(p) => println!("Profile: {p}"),
                None => println!("Profile: (not read)"),
            }
            if found.pages.is_empty() {
                println!("No Pages found. The switcher opened but listed none.");
            } else {
                println!("{} Page(s):", found.pages.len());
                for name in &found.pages {
                    println!("  - {name}");
                }
            }
        }
        Err(e) => {
            eprintln!("failed: {e}");
            std::process::exit(1);
        }
    }
}
