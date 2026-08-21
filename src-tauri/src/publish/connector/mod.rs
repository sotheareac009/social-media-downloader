//! Platform connectors: the only place in the codebase that is allowed to know
//! how a particular social app behaves.
//!
//! ```text
//!   FacebookConnector ┐
//!   InstagramConnector├─ implement PlatformConnector
//!   TikTokConnector   │
//!   YouTubeConnector  ┘
//!            ↓ uses only the generic device API
//!   ldplayer::manager → adb → Android
//! ```
//!
//! THE CONTRACT. A connector receives a [`PublishContext`]: a device that is
//! already online, a video already copied onto it and already visible to the
//! gallery, and a caption. Its job is the last mile — get that app to its own
//! composer with that video attached.
//!
//! WHAT A CONNECTOR MUST NOT DO. This is not a style rule; it is the boundary
//! that keeps the feature legitimate:
//!
//!   * never type, read, or store a password;
//!   * never read cookies, tokens or session files off the device;
//!   * never automate a login screen, a two-factor prompt, or a captcha;
//!   * never work around a rate limit, a checkpoint, or a security interstitial.
//!
//! When an app asks for any of those, the connector returns
//! [`Outcome::NeedsUser`] and stops. The person finishes it in the emulator,
//! the same way they would on their phone. That is the whole reason this app
//! never asks for a social password: the session already exists inside the
//! Android app, and it stays there.

use std::sync::Arc;

use async_trait::async_trait;

use crate::errors::Result;
use crate::ldplayer::adb::MediaCollection;
use crate::ldplayer::manager::LdPlayerManager;
use crate::publish::model::Platform;

pub mod autopost;
pub mod share;

/// One asset sitting on the device, ready to hand to an app.
#[derive(Debug, Clone)]
pub struct StagedMedia {
    /// Original file name, for messages to the user.
    pub file_name: String,
    /// Absolute on-device path, already indexed by MediaStore.
    pub remote_path: String,
    /// `content://media/...`. Absent means the share-intent route is
    /// unavailable and the connector must fall back to opening the app and
    /// letting the user pick from the gallery.
    pub content_uri: Option<String>,
    /// Video or image. Decides the share MIME type, and is worth naming in a
    /// message — "the photo is attached" reads wrong for a video.
    pub collection: MediaCollection,
}

/// Everything a connector is given, and the only device access it gets.
pub struct PublishContext {
    /// Generic device layer. A connector may push files, launch apps and take
    /// screenshots through this; it has no other route to the emulator.
    pub manager: Arc<LdPlayerManager>,
    /// `ld:0`, `adb:emulator-5554`.
    pub device_id: String,
    /// Live adb serial, already resolved and verified online.
    pub serial: String,
    /// The Android package for this account — possibly a Lite variant, which
    /// is why it is passed rather than read from [`Platform::default_package`].
    pub package: String,
    /// Every asset, already copied to the device and indexed, in the order
    /// the user arranged them. Always at least one.
    pub media: Vec<StagedMedia>,
    pub caption: String,
    /// Platform name for messages, so the automation engine never has to know
    /// what platform it is driving.
    pub platform_label: &'static str,
    /// Whether the user asked for the final Post tap to be automated.
    pub auto_post: bool,
    /// Report a step to the UI. Called with a coarse 0–1 fraction and a
    /// sentence a non-technical person can read.
    pub report: Box<dyn Fn(f64, &str) + Send + Sync>,
}

impl PublishContext {
    pub fn step(&self, progress: f64, message: &str) {
        (self.report)(progress, message);
    }

    /// The first asset. Safe to call: the queue refuses to start a job with
    /// no media, so this list is never empty.
    pub fn first(&self) -> &StagedMedia {
        &self.media[0]
    }

    /// "video", "photo" or "files" — the word a message should use.
    pub fn noun(&self) -> &'static str {
        if self.is_mixed() {
            return "files";
        }
        match self.first().collection {
            crate::ldplayer::adb::MediaCollection::Video => {
                if self.is_album() { "videos" } else { "video" }
            }
            crate::ldplayer::adb::MediaCollection::Image => {
                if self.is_album() { "photos" } else { "photo" }
            }
        }
    }

    /// True when this is an album post rather than a single one.
    pub fn is_album(&self) -> bool {
        self.media.len() > 1
    }

    /// A mixed video+photo album, which some platforms refuse. Worth naming in
    /// the hand-off message rather than letting the app reject it silently.
    pub fn is_mixed(&self) -> bool {
        self.media
            .iter()
            .any(|m| m.collection != self.first().collection)
    }

    /// Capture the screen for the job's debug view. Best effort: a failed
    /// screenshot must never fail a publish.
    pub async fn screenshot(&self, label: &str) -> Option<String> {
        self.manager.screenshot(&self.device_id, Some(label)).await.ok()
    }
}

/// How a publishing attempt ended.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// The post is live. Only returned when the connector actually observed
    /// that — never assumed from "the intent didn't error".
    Published,
    /// Everything this app can do is done: the app is open on its composer
    /// with the media attached, and the person taps Post. This is a SUCCESS —
    /// the deliberate stopping point, not a problem — and the UI must not
    /// dress it as a failure.
    ReadyForUser(String),
    /// Something got in the way: a login prompt, a permission request, the app
    /// closing. The work so far is intact and the file is on the device, but
    /// the person has to go and look.
    Interrupted(String),
}

/// The interface every platform implements.
#[async_trait]
pub trait PlatformConnector: Send + Sync {
    fn platform(&self) -> Platform;

    /// Human-readable name of the strategy, shown in the job's step line so a
    /// user can tell which route was taken.
    fn strategy(&self) -> &'static str;

    async fn publish(&self, ctx: &PublishContext) -> Result<Outcome>;
}

/// Pick the connector for a platform.
///
/// A `match` rather than a registry map: with a handful of platforms the map
/// buys nothing, and the match makes the compiler tell you about the one you
/// forgot when you add a variant.
pub fn for_platform(platform: Platform) -> Arc<dyn PlatformConnector> {
    match platform {
        // Every platform currently uses the share-intent strategy. As each one
        // gets a purpose-built connector, only this match changes — the queue,
        // the device layer and the UI stay exactly as they are.
        Platform::Facebook
        | Platform::Instagram
        | Platform::Tiktok
        | Platform::Youtube => Arc::new(share::ShareConnector::new(platform)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_platform_has_a_connector_for_its_own_platform() {
        for p in Platform::ALL {
            assert_eq!(for_platform(*p).platform(), *p);
        }
    }
}
