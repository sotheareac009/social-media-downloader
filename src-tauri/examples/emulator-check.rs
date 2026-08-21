//! `cargo run --example emulator-check`
//!
//! Reports what the publishing feature can see on this machine — LDPlayer,
//! ADB, instances, devices and which social apps are installed — without
//! launching the UI.
//!
//! Run this first when publishing "doesn't work" on a new machine. It answers
//! the three questions the GUI can only hint at: is ADB there, does LDPlayer
//! answer, and does each instance actually have the app you think it has.
//!
//! Optional second argument transfers a file, so the whole PC → emulator path
//! can be verified end to end:
//!
//! ```text
//! cargo run --example emulator-check -- push ld:0 C:\videos\my-video.mp4
//! cargo run --example emulator-check -- push ld:0 C:\photos\shot.jpg
//! ```
//!
//! SECURITY: prints device names, serials and package names. No social-media
//! credential is involved anywhere in this path, so the output is safe to
//! paste into a bug report.

use std::path::PathBuf;

use media_downloader_lib::ldplayer::manager::LdPlayerManager;
use media_downloader_lib::publish::model::Platform;

#[tokio::main]
async fn main() {
    let data_dir = std::env::temp_dir().join("emulator-check");
    let manager = LdPlayerManager::new(data_dir);

    let env = manager.environment().await;
    println!("Environment");
    println!(
        "  ADB          {}",
        env.adb_path.as_deref().unwrap_or("NOT FOUND")
    );
    println!(
        "  ADB version  {}",
        env.adb_version.as_deref().unwrap_or("-")
    );
    println!(
        "  LDPlayer     {}",
        env.ldplayer_path.as_deref().unwrap_or(if env.ldplayer_supported {
            "NOT FOUND"
        } else {
            "not applicable on this OS"
        })
    );
    println!("  Upload dir   {}", env.remote_dir);
    println!();

    if !env.adb_available {
        println!("ADB was not found, so nothing else can be checked.");
        println!("Install LDPlayer, or put adb on PATH.");
        return;
    }

    let devices = match manager.list_devices(None).await {
        Ok(d) => d,
        Err(e) => {
            println!("Could not list devices: {e}");
            return;
        }
    };

    if devices.is_empty() {
        // The advice has to match the platform: telling a Mac user to start an
        // LDPlayer instance sends them after software that does not exist here.
        println!("No devices.");
        if env.ldplayer_supported {
            println!("Start an LDPlayer instance, and make sure ADB debugging is on in");
            println!("that instance's Settings -> Other settings, then run this again.");
        } else {
            println!("Attach an Android device or start an emulator, then run this again:");
            println!("  * a phone with USB debugging enabled, or");
            println!("  * an Android Studio AVD (`emulator -avd <name>`).");
        }
        return;
    }

    println!("Devices");
    for d in &devices {
        println!(
            "  {:<20} {:<10} {:<24} {}",
            d.id,
            format!("{:?}", d.state).to_lowercase(),
            d.serial.as_deref().unwrap_or("-"),
            d.name
        );
        if let Some(err) = &d.error {
            println!("    ! {err}");
        }

        if d.is_online() {
            match manager.packages(&d.id).await {
                Ok(pkgs) => {
                    let found: Vec<String> = pkgs
                        .iter()
                        .filter_map(|p| {
                            Platform::for_package(p).map(|pl| format!("{} ({p})", pl.label()))
                        })
                        .collect();
                    if found.is_empty() {
                        println!("    no supported social apps installed");
                    } else {
                        for f in found {
                            println!("    {f}");
                        }
                    }
                }
                Err(e) => println!("    could not list packages: {e}"),
            }
        }
    }

    // Optional: `push <device-id> <file>` proves the whole transfer path.
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 4 && args[1] == "push" {
        let device_id = &args[2];
        let file = PathBuf::from(&args[3]);
        println!();
        println!("Transferring {} to {device_id}", file.display());
        match manager.transfer_media(None, device_id, &file).await {
            Ok(media) => {
                println!("  copied and indexed at {}", media.remote_path);
                println!("  filed as {:?}", media.collection);
                match media.content_uri {
                    Some(uri) => println!("  handed to apps as {uri}"),
                    // Should not happen on the success path, but printing it
                    // beats a diagnostic that quietly implies all is well.
                    None => println!("  WARNING: no MediaStore URI - apps cannot be handed this file"),
                }
                println!("  it should now appear in the emulator's gallery");
            }
            Err(e) => println!("  failed: {e}"),
        }
    }
}
