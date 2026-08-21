//! Publishing: accounts, the job queue, and the platform connectors.
//!
//! ```text
//!   commands::publish              ← Tauri IPC
//!          ↓
//!   publish::queue                 ← orchestration, bounded workers
//!          ↓            ↘
//!   publish::store       publish::connector::*   ← the only platform-aware code
//!   (SQLite)                    ↓
//!                        ldplayer::manager → adb → Android
//! ```
//!
//! WHAT THIS FEATURE IS. Each "account" is an Android app on an LDPlayer
//! instance that the user has already signed into, by hand, inside the
//! emulator. This app copies a video onto that instance, makes Android's
//! gallery see it, and opens the app on its own composer. The session never
//! leaves the emulator; this app never sees it.
//!
//! WHAT THIS FEATURE IS NOT. It does not collect social-media passwords, does
//! not read cookies or tokens off the device, does not automate a login, and
//! does not work around a captcha, a checkpoint or a rate limit. When a
//! platform asks for any of those, the job stops with
//! [`model::JobStatus::NeedsAttention`] and the person finishes it in the
//! emulator themselves. See [`connector`] for the contract every platform
//! implementation is held to.

pub mod connector;
pub mod model;
pub mod queue;
pub mod store;
