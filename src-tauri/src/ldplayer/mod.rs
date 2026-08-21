//! Emulator control: LDPlayer instances, and the ADB channel to them.
//!
//! This is the generic half of the publishing feature. It moves files onto an
//! Android device, starts apps, and reports what it sees. It has no idea which
//! social network any of that is for — that knowledge lives one layer up, in
//! [`crate::publish`].
//!
//! ```text
//!   commands::ldplayer          ← Tauri IPC
//!          ↓
//!   ldplayer::manager           ← the service the rest of the app uses
//!          ↓                ↘
//!   ldplayer::console        ldplayer::adb
//!   (ldconsole.exe)          (adb, generic Android)
//! ```
//!
//! SECURITY POSTURE. Nothing here handles a social-media credential, because
//! there is never one to handle: the account stays signed in *inside* the
//! Android app, and this app only ever moves a file and taps "open". No cookie
//! or token is read off the device, and no login screen is ever automated.

pub mod adb;
pub mod console;
pub mod manager;
pub mod settings;
