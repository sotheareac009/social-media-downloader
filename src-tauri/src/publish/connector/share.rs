//! The share-intent connector: the first, generic platform connector.
//!
//! STRATEGY. Android has a supported way for one app to hand media to another:
//! `ACTION_SEND` with a `content://` URI and a read grant. Every social app
//! implements it, because it is what the system share sheet uses. Firing it
//! lands the user in that app's own composer with the video already attached
//! and — where the app honours `EXTRA_TEXT` — the caption already filled in.
//!
//! WHY THIS IS THE RIGHT FIRST CONNECTOR. It uses each platform's published
//! integration point rather than pretending to be a finger. Nothing here
//! depends on where a button sits this month, so it does not break when an app
//! updates; nothing here touches a login; and the final Post tap stays with
//! the person, which is both the safe default and, for the MVP, the honest
//! one.
//!
//! WHERE IT STOPS. It does not confirm a post went live, because it cannot.
//! It reports [`Outcome::NeedsUser`] and says so plainly rather than showing a
//! green tick nobody verified. A per-platform connector that drives the
//! composer to completion replaces this one platform at a time — see
//! [`super::for_platform`].

use async_trait::async_trait;

use crate::errors::Result;
use crate::publish::connector::{Outcome, PlatformConnector, PublishContext};
use crate::publish::model::Platform;

/// Videos are the only media this build publishes, so the MIME type is fixed.
const MIME: &str = "video/*";

pub struct ShareConnector {
    platform: Platform,
}

impl ShareConnector {
    pub fn new(platform: Platform) -> Self {
        Self { platform }
    }

    /// Whether this app puts `EXTRA_TEXT` into its caption box.
    ///
    /// Facebook's composer does. Instagram, TikTok and YouTube ignore it and
    /// open an empty caption field, so telling the user to paste is the
    /// truthful instruction — the caption is on their clipboard-equivalent,
    /// visible in the job, ready to copy.
    fn honours_caption(&self) -> bool {
        matches!(self.platform, Platform::Facebook)
    }
}

#[async_trait]
impl PlatformConnector for ShareConnector {
    fn platform(&self) -> Platform {
        self.platform
    }

    fn strategy(&self) -> &'static str {
        "Android share intent"
    }

    async fn publish(&self, ctx: &PublishContext) -> Result<Outcome> {
        let label = self.platform.label();

        // Wake the app first. Sending a share intent to an app that has not
        // run since boot frequently lands on a blank screen while it
        // initialises, and the user sees "nothing happened".
        ctx.step(0.72, &format!("Opening {label}…"));
        ctx.manager
            .launch_app(None, &ctx.device_id, &ctx.package)
            .await?;
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let adb = ctx.manager.adb()?;

        let Some(uri) = ctx.content_uri.as_deref() else {
            // No MediaStore URI means the gallery cannot offer the file
            // either, so the honest move is to say the file is on the device
            // and let the person pick it, rather than fire an intent that will
            // fail with a permission error.
            ctx.step(0.9, "Opened the app; pick the video from the gallery");
            return Ok(Outcome::NeedsUser(format!(
                "{label} is open on this instance. The video is on the device at {}, \
                 but Android did not index it, so choose it from the gallery manually.",
                ctx.remote_path
            )));
        };

        ctx.step(0.82, &format!("Handing the video to {label}…"));
        let caption = self.honours_caption().then(|| ctx.caption.as_str());
        adb.share_to(&ctx.serial, &ctx.package, uri, MIME, caption)
            .await?;

        // Give the composer a moment to come up before looking at the screen.
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;

        // Confirm the app is actually in the foreground. If it crashed or
        // bounced back to the launcher, saying "ready for you" would send the
        // user to look at a screen that isn't there.
        let foreground = adb.foreground_package(&ctx.serial).await;
        let arrived = foreground
            .as_deref()
            .is_some_and(|p| p == ctx.package || p.starts_with(&ctx.package));

        if let Some(path) = ctx.screenshot("composer").await {
            ctx.step(0.95, &format!("{label} composer captured"));
            let _ = path;
        }

        if !arrived {
            ctx.step(0.9, &format!("{label} did not come to the front"));
            return Ok(Outcome::NeedsUser(format!(
                "The video was sent to {label}, but it is not on screen — it may have \
                 asked for a permission or a login confirmation. Open LDPlayer for this \
                 instance and finish there."
            )));
        }

        ctx.step(0.98, "Waiting for you to post");
        Ok(Outcome::NeedsUser(if self.honours_caption() {
            format!(
                "{label} is open with the video and caption attached on this instance. \
                 Review it and tap Post."
            )
        } else {
            format!(
                "{label} is open with the video attached on this instance. \
                 {label} does not accept a caption from outside its app, so paste the \
                 caption there, then tap Post."
            )
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_facebook_is_claimed_to_honour_extra_text() {
        // If this changes, it must change because the app's behaviour was
        // re-tested — not because it seemed likely.
        assert!(ShareConnector::new(Platform::Facebook).honours_caption());
        for p in [Platform::Instagram, Platform::Tiktok, Platform::Youtube] {
            assert!(!ShareConnector::new(p).honours_caption(), "{p:?}");
        }
    }

    #[test]
    fn the_connector_reports_the_platform_it_was_built_for() {
        for p in Platform::ALL {
            assert_eq!(ShareConnector::new(*p).platform(), *p);
        }
    }
}
