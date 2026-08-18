//! Download domain: public media fetched anonymously.
//!
//! Layering rule, the mirror of the one in [`crate::auth`]: nothing in here
//! knows about accounts, tokens or the keychain. It takes a public URL and
//! produces a file on disk.
//!
//! That separation is the whole design. Signing in and downloading are two
//! unrelated capabilities in this app:
//!
//!   * **Signing in** proves who you are to a platform. It grants this build
//!     no access to media - the scopes requested are profile-only.
//!   * **Downloading** works on posts anybody can already open in a browser
//!     without logging in, and deliberately runs with no session at all.
//!
//! So a Facebook or TikTok link is downloadable exactly when it is public, and
//! connecting an account changes nothing about that. Private posts fail with
//! [`crate::errors::AppError::MediaNotPublic`] whether or not you are signed
//! in, because the engine is never handed a cookie or a token.

pub mod manager;
pub mod settings;
pub mod url;
pub mod ytdlp;
