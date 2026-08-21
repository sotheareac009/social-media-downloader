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
//! It reports [`Outcome::ReadyForUser`] and says so plainly rather than showing a
//! green tick nobody verified. A per-platform connector that drives the
//! composer to completion replaces this one platform at a time — see
//! [`super::for_platform`].

use async_trait::async_trait;

use crate::errors::Result;
use crate::ldplayer::adb::MediaCollection;
use crate::publish::connector::{autopost, Outcome, PlatformConnector, PublishContext};
use crate::publish::model::Platform;

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
        let noun = noun_for(ctx);

        // Wake the app first. Sending a share intent to an app that has not
        // run since boot frequently lands on a blank screen while it
        // initialises, and the user sees "nothing happened".
        ctx.step(0.72, &format!("Opening {label}…"));
        ctx.manager
            .launch_app(None, &ctx.device_id, &ctx.package)
            .await?;
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let adb = ctx.manager.adb()?;

        // An album cannot be pre-attached. `ACTION_SEND_MULTIPLE` needs
        // EXTRA_STREAM as a ParcelableArrayList<Uri>, and `am start` has no
        // flag that builds one — `--eu` takes a single URI, and `--esa` would
        // pass Strings, which every receiving app rejects with a
        // ClassCastException. So the files are staged and the person selects
        // them, which is honest, rather than firing an intent we know fails.
        if ctx.is_album() {
            // Not automatable, and not for want of trying: the files can only
            // be attached through the app's own gallery picker, and driving a
            // multi-select grid by label is exactly the kind of blind tapping
            // that posts the wrong thing.
            ctx.step(0.92, &format!("{} files are ready in the gallery", ctx.media.len()));
            return Ok(Outcome::ReadyForUser(album_message(ctx, label)));
        }

        let item = ctx.first();
        let Some(uri) = item.content_uri.as_deref() else {
            // No MediaStore URI means the gallery cannot offer the file
            // either, so the honest move is to say where it is and let the
            // person pick it, rather than fire an intent that will fail with a
            // permission error.
            ctx.step(0.9, &format!("Opened the app; pick the {noun} from the gallery"));
            return Ok(Outcome::ReadyForUser(format!(
                "{label} is open on this instance. The {noun} is on the device at {}, \
                 but Android did not index it, so choose it from the gallery manually.",
                item.remote_path
            )));
        };

        ctx.step(0.82, &format!("Handing the {noun} to {label}…"));
        let caption = self.honours_caption().then(|| ctx.caption.as_str());
        adb.share_to(&ctx.serial, &ctx.package, uri, item.collection.mime(), caption)
            .await?;

        // Give the composer time to come up. Twenty seconds is not generous —
        // a cold app on a loaded emulator regularly needs most of it.
        ctx.step(0.9, &format!("Waiting for the {label} composer…"));
        let seen = self
            .wait_for_foreground(ctx, &adb, std::time::Duration::from_secs(20))
            .await;
        self.capture(ctx).await;

        if let Foreground::Other(pkg) = &seen {
            ctx.step(0.92, &format!("{pkg} is on screen instead of {label}"));
            let because = explain_foreground(pkg)
                .map(|r| format!(" — {r}"))
                .unwrap_or_else(|| format!(" — `{pkg}` is on screen instead"));
            return Ok(Outcome::Interrupted(format!(
                "The {noun} was sent to {label}, but {label} isn't the screen in \
                 front{because}. Open LDPlayer for this instance and finish there; \
                 the {noun} is already in the gallery."
            )));
        }

        // Everything up to here is the same whether or not the last tap is
        // automated: the composer is open with the media attached. Only now
        // does the app get driven, and only if the user asked for it.
        if ctx.auto_post {
            match autopost::run(ctx, &adb, &autopost::recipe(self.platform)).await? {
                autopost::Run::Posted => {
                    ctx.step(1.0, &format!("Posted to {label}"));
                    return Ok(Outcome::Published);
                }
                // Not a failure: the media is attached and the composer is
                // open. The automation stopped somewhere it should not guess,
                // and said where.
                autopost::Run::HandedBack(why) => {
                    ctx.step(0.97, "Stopped — needs you");
                    return Ok(Outcome::Interrupted(why));
                }
            }
        }

        ctx.step(0.98, "Ready for you to post");
        Ok(Outcome::ReadyForUser(if self.honours_caption() {
            format!(
                "{label} is open with the {noun} and caption attached on this instance. \
                 Review it and tap Post."
            )
        } else {
            format!(
                "{label} is open with the {noun} attached on this instance. \
                 {label} does not accept a caption from outside its app, so paste the \
                 caption there, then tap Post."
            )
        }))
    }
}

/// What the emulator's screen showed after the share was sent.
enum Foreground {
    /// The target app is in front. The composer is up.
    Arrived,
    /// Something else is in front, named so the message can explain it.
    Other(String),
    /// The screen could not be read. NOT the same as failure — reporting a
    /// problem we cannot demonstrate would send people to look at a screen
    /// that is very likely fine.
    Unknown,
}

impl ShareConnector {
    /// Wait for the target app to come to the front.
    ///
    /// Polls rather than sleeping once and looking. A cold app on a loaded
    /// emulator routinely takes ten seconds or more to render its composer,
    /// and a single check four seconds in reports a healthy publish as broken
    /// — which is worse than waiting, because it sends the user hunting for a
    /// problem that does not exist.
    async fn wait_for_foreground(
        &self,
        ctx: &PublishContext,
        adb: &crate::ldplayer::adb::Adb,
        timeout: std::time::Duration,
    ) -> Foreground {
        let deadline = std::time::Instant::now() + timeout;
        let mut last: Option<String> = None;

        loop {
            match adb.foreground_package(&ctx.serial).await {
                Some(pkg) if pkg == ctx.package || pkg.starts_with(&ctx.package) => {
                    return Foreground::Arrived
                }
                Some(pkg) => last = Some(pkg),
                None => {}
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        }

        match last {
            Some(pkg) => Foreground::Other(pkg),
            None => Foreground::Unknown,
        }
    }

    /// Capture the screen for the job's debug view, best effort.
    async fn capture(&self, ctx: &PublishContext) {
        if ctx.screenshot("composer").await.is_some() {
            ctx.step(0.95, &format!("{} screen captured", self.platform.label()));
        }
    }
}

/// Turn "some other app is in front" into a sentence that names the likely
/// reason. These four cover almost every real case, and a user who is told
/// "it asked you to pick a Google account" fixes it in seconds, while one told
/// "it is not on screen" has to go and look.
fn explain_foreground(pkg: &str) -> Option<&'static str> {
    if pkg.starts_with("com.google.android.gms") {
        return Some("it asked you to choose or confirm a Google account");
    }
    if pkg.contains("permissioncontroller") || pkg.contains("packageinstaller") {
        return Some("it asked for a permission");
    }
    if pkg.contains("launcher") || pkg == "com.android.systemui" || pkg == "android" {
        return Some("it closed or a system dialog took over");
    }
    None
}

/// "video", "photo", or "files" for a mixed album — the word the message uses.
fn noun_for(ctx: &PublishContext) -> &'static str {
    if ctx.is_mixed() {
        return "files";
    }
    match ctx.first().collection {
        MediaCollection::Video => {
            if ctx.is_album() { "videos" } else { "video" }
        }
        MediaCollection::Image => {
            if ctx.is_album() { "photos" } else { "photo" }
        }
    }
}

/// What to tell the user once an album is staged.
///
/// Names the files and their order, because carousel order is the thing they
/// chose and the gallery will not preserve it — selection order in the app's
/// picker is what decides it.
fn album_message(ctx: &PublishContext, label: &str) -> String {
    let names: Vec<String> = ctx
        .media
        .iter()
        .enumerate()
        .map(|(i, m)| format!("{}. {}", i + 1, m.file_name))
        .collect();

    let mixed = if ctx.is_mixed() {
        " Some platforms refuse albums that mix photos and videos; if this one does, \
         publish them as separate posts instead."
    } else {
        ""
    };

    format!(
        "{label} is open on this instance and all {} files are in its gallery. \
         Start a post, then select them in this order:\n{}\n\n\
         They can't be attached from outside the app — Android has no way to hand an \
         app several files at once.{mixed}",
        ctx.media.len(),
        names.join("\n")
    )
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
    fn common_interruptions_are_explained_rather_than_just_named() {
        assert!(explain_foreground("com.google.android.gms").unwrap().contains("Google account"));
        assert!(explain_foreground("com.google.android.permissioncontroller")
            .unwrap()
            .contains("permission"));
        assert!(explain_foreground("com.android.launcher3").unwrap().contains("closed"));
        // An app we have no story for gets named, not guessed about.
        assert!(explain_foreground("com.some.other.app").is_none());
    }

    #[test]
    fn the_connector_reports_the_platform_it_was_built_for() {
        for p in Platform::ALL {
            assert_eq!(ShareConnector::new(*p).platform(), *p);
        }
    }
}
