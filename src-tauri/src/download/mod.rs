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
//!   * **Downloading** works on posts anybody can already open in a browser.
//!     For YouTube, Facebook and TikTok it runs with no session at all. For
//!     Instagram - which answers anonymous requests with an empty response -
//!     it uses a session the user captured in the app's own login window, and
//!     only for Instagram links.
//!
//! So a Facebook, TikTok or YouTube link is downloadable exactly when it is
//! public, and connecting an account on the Accounts page changes nothing
//! about that - those requests are never handed a cookie or a token.
//!
//! The Instagram exception is deliberately narrow, and enforced rather than
//! promised: the cookie jar is built only for [`url::Source::Instagram`] (see
//! `DownloadManager::cookie_jar`), `--no-cookies-from-browser` is passed on
//! every invocation so the user's browser profile is never read, and the jar
//! is a 0600 temp file deleted when the job ends. See [`session`].

pub mod cookies;
pub mod manager;
pub mod quality;
pub mod session;
pub mod settings;
pub mod slideshow;
pub mod url;
pub mod ytdlp;
