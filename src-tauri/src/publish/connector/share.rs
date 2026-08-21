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

        // Wake the app first. Sending a share intent to an app that has not
        // run since boot frequently lands on a blank screen while it
        // initialises, and the user sees "nothing happened".
        ctx.step(0.72, &format!("Opening {label}…"));
        ctx.manager
            .launch_app(None, &ctx.device_id, &ctx.package)
            .await?;
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let adb = ctx.manager.adb()?;

        // An album cannot be pre-attached as ONE post. `ACTION_SEND_MULTIPLE`
        // needs EXTRA_STREAM as a ParcelableArrayList<Uri>, and `am start` has
        // no flag that builds one — checked against the device's own `am
        // help`, which offers lists of Strings, Integers, Longs, Floats and
        // Doubles, and single Uris via `--eu`. Nothing else.
        //
        // So a carousel is out, but the files need not be: each is posted on
        // its own through the route that does work, which finishes the job
        // instead of handing it back. That needs the final tap automated —
        // without it, N posts would mean N manual taps, and staging the album
        // for one hand-picked carousel is the better trade, because it is at
        // least the post the person asked for.
        if ctx.is_album() {
            if !ctx.auto_post {
                ctx.step(0.92, &format!("{} files are ready in the gallery", ctx.media.len()));
                return Ok(Outcome::ReadyForUser(album_message(ctx, label)));
            }
            return self.publish_separately(ctx, &adb, label).await;
        }

        self.publish_one(ctx, &adb, ctx.first(), None).await
    }
}

impl ShareConnector {
    /// Post every file in an album as its own post, in the order the person
    /// arranged them.
    ///
    /// Stops at the first one that does not go out. Carrying on would mean
    /// firing more intents at an app that has already shown it is not in a
    /// state to post — most often a login or a checkpoint, which the next file
    /// would hit too — and every attempt past that point leaves one more
    /// half-finished composer on someone's account.
    async fn publish_separately(
        &self,
        ctx: &PublishContext,
        adb: &crate::ldplayer::adb::Adb,
        label: &str,
    ) -> Result<Outcome> {
        let total = ctx.media.len();
        ctx.step(
            0.75,
            &format!("Posting {total} files to {label} as {total} separate posts…"),
        );

        let mut posted = 0usize;
        for (i, item) in ctx.media.iter().enumerate() {
            if i > 0 {
                // Let the app finish returning to its feed. Firing the next
                // share intent while the previous composer is still animating
                // away lands it on the wrong screen.
                tokio::time::sleep(std::time::Duration::from_secs(4)).await;
            }

            match self.publish_one(ctx, adb, item, Some((i + 1, total))).await? {
                Outcome::Published => posted += 1,
                stopped => return Ok(stopped_partway(posted, total, i + 1, stopped)),
            }
        }

        ctx.step(1.0, &format!("Posted {posted} separate posts to {label}"));
        Ok(Outcome::Published)
    }

    /// Hand one file to the app and, when asked, drive the composer to Post.
    ///
    /// `seq` is `(n, total)` when this is one file of an album being posted
    /// separately, and `None` for an ordinary single post. It only shapes the
    /// progress lines, and that is worth its weight: "2 of 3: Waiting for the
    /// Facebook composer…" is the difference between a job that looks stuck
    /// and one a person can follow.
    async fn publish_one(
        &self,
        ctx: &PublishContext,
        adb: &crate::ldplayer::adb::Adb,
        item: &crate::publish::connector::StagedMedia,
        seq: Option<(usize, usize)>,
    ) -> Result<Outcome> {
        let Some(page) = ctx.post_as_page.as_deref() else {
            return self.post_as_current_identity(ctx, adb, item, seq).await;
        };

        // Posting as a Page means switching the whole app over, so the switch
        // has to be undone whatever happens next — including a failure. An app
        // left active as a Page publishes the NEXT job there too, and that job
        // did nothing wrong.
        // Start clean before switching. The profile switcher is reached from
        // the app's own home screen, and a publish that stopped half way
        // leaves a composer sitting on top of it — that composer has a "Menu"
        // control of its own and no switcher behind it, so the switch would
        // fail on exactly the accounts that had already had one bad run.
        ctx.step(0.76, &format!("Restarting {}…", self.platform.label()));
        adb.relaunch_app(&ctx.serial, &ctx.package).await?;
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        ctx.step(0.78, &format!("Switching {} to {page}…", self.platform.label()));
        crate::publish::connector::pages::switch_to(adb, &ctx.serial, page).await?;

        let outcome = self.post_as_current_identity(ctx, adb, item, seq).await;
        self.restore_identity(ctx, adb).await;
        outcome
    }

    /// Switch back to the profile after a Page post.
    ///
    /// Best effort, and deliberately not fatal: the post has already happened
    /// by the time this runs, so failing the job over the cleanup would report
    /// a successful publish as a failure. The identity check on the next job
    /// is what actually protects against a session left switched — this just
    /// means it rarely has to.
    async fn restore_identity(&self, ctx: &PublishContext, adb: &crate::ldplayer::adb::Adb) {
        let Some(profile) = ctx.profile_name.as_deref() else {
            // Nothing to switch back TO. Say so loudly: the app is still the
            // Page, and the person needs to know before they queue anything.
            ctx.step(
                0.99,
                "Posted, but this account has no profile name saved, so the app is still                  switched to the Page — run Find Pages to learn it",
            );
            return;
        };

        // Clean task again: if the post FAILED, its composer is still open,
        // and that is the one screen the switcher cannot be reached from. The
        // post has already happened or already failed by now, so there is
        // nothing left in that composer worth keeping.
        ctx.step(0.99, &format!("Switching back to {profile}…"));
        adb.relaunch_app(&ctx.serial, &ctx.package).await.ok();
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        if crate::publish::connector::pages::switch_to(adb, &ctx.serial, profile)
            .await
            .is_err()
        {
            ctx.step(
                0.99,
                &format!(
                    "Posted, but couldn't switch back to {profile} — check the instance                      before the next job"
                ),
            );
        }
    }

    /// One post, as whoever the app is currently signed in as.
    async fn post_as_current_identity(
        &self,
        ctx: &PublishContext,
        adb: &crate::ldplayer::adb::Adb,
        item: &crate::publish::connector::StagedMedia,
        seq: Option<(usize, usize)>,
    ) -> Result<Outcome> {
        let label = self.platform.label();
        let noun = noun_for_item(item);
        let tag = match seq {
            Some((n, total)) => format!("{n} of {total}: "),
            None => String::new(),
        };

        let Some(uri) = item.content_uri.as_deref() else {
            // No MediaStore URI means the gallery cannot offer the file
            // either, so the honest move is to say where it is and let the
            // person pick it, rather than fire an intent that will fail with a
            // permission error.
            ctx.step(0.9, &format!("{tag}Opened the app; pick the {noun} from the gallery"));
            return Ok(Outcome::ReadyForUser(format!(
                "{label} is open on this instance. The {noun} is on the device at {}, \
                 but Android did not index it, so choose it from the gallery manually.",
                item.remote_path
            )));
        };

        ctx.step(0.82, &format!("{tag}Handing the {noun} to {label}…"));
        let caption = self.honours_caption().then(|| ctx.caption.as_str());
        adb.share_to(&ctx.serial, &ctx.package, uri, item.collection.mime(), caption)
            .await?;

        // Give the composer time to come up. Twenty seconds is not generous —
        // a cold app on a loaded emulator regularly needs most of it.
        ctx.step(0.9, &format!("{tag}Waiting for the {label} composer…"));
        let seen = self
            .wait_for_foreground(ctx, adb, std::time::Duration::from_secs(20))
            .await;
        self.capture(ctx).await;

        if let Foreground::Other(pkg) = &seen {
            ctx.step(0.92, &format!("{tag}{pkg} is on screen instead of {label}"));
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
            match autopost::run(ctx, adb, self.platform).await? {
                autopost::Run::Posted => {
                    ctx.step(1.0, &format!("{tag}Posted to {label}"));
                    return Ok(Outcome::Published);
                }
                // Not a failure: the media is attached and the composer is
                // open. The automation stopped somewhere it should not guess,
                // and said where.
                autopost::Run::HandedBack(why) => {
                    ctx.step(0.97, &format!("{tag}Stopped — needs you"));
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

/// "video" or "photo" for one file — the word a per-file message uses.
///
/// Deliberately per-file rather than per-job: when an album is posted file by
/// file, each post really is one photo, and "2 of 3: Handing the photos to
/// Facebook" reads as though all three went into that one post.
fn noun_for_item(item: &crate::publish::connector::StagedMedia) -> &'static str {
    match item.collection {
        MediaCollection::Video => "video",
        MediaCollection::Image => "photo",
    }
}

/// Report an album that stopped part way through.
///
/// The count leads, because it is the thing the person has to act on: posts
/// that already went out cannot be taken back, and a message that only
/// explains the failure leaves them guessing how much of the album is live.
fn stopped_partway(posted: usize, total: usize, at: usize, stopped: Outcome) -> Outcome {
    let reason = match stopped {
        Outcome::ReadyForUser(why) | Outcome::Interrupted(why) => why,
        // Not reachable: the caller only calls this for a non-Published
        // outcome. Worth a sentence rather than a panic — a publish is a bad
        // place to discover an unwrap.
        Outcome::Published => "it stopped without saying why".to_string(),
    };

    let done = if posted == 0 {
        "Nothing was posted".to_string()
    } else {
        format!("{posted} of {total} went out as separate posts, and they are live")
    };

    Outcome::Interrupted(format!("{done}. File {at} stopped: {reason}"))
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
    use crate::publish::connector::StagedMedia;

    /// The bug this exists for: an album job stopped dead with "they can't be
    /// attached from outside the app". That is true of a carousel — verified
    /// against the device's own `am help`, which has no list-of-Uri extra —
    /// but it is not true of the files, which can each be posted on their own.
    /// So each file gets the per-file word, not the album's plural.
    #[test]
    fn each_file_of_a_split_album_is_described_on_its_own() {
        let photo = StagedMedia {
            file_name: "a.jpg".into(),
            remote_path: "/sdcard/Pictures/a.jpg".into(),
            content_uri: Some("content://media/external/images/media/1".into()),
            collection: MediaCollection::Image,
        };
        let video = StagedMedia { collection: MediaCollection::Video, ..photo.clone() };

        assert_eq!(noun_for_item(&photo), "photo");
        assert_eq!(noun_for_item(&video), "video");
    }

    /// Posts that already went out cannot be taken back, so a partial album
    /// has to lead with how many are live — a message that only explains the
    /// failure leaves someone guessing what is on their profile.
    #[test]
    fn a_part_posted_album_says_how_much_is_already_live() {
        let out = stopped_partway(2, 3, 3, Outcome::Interrupted("Facebook asked you to log in".into()));
        let Outcome::Interrupted(msg) = out else {
            panic!("a part-posted album is not a clean success");
        };
        assert!(msg.contains("2 of 3"), "{msg}");
        assert!(msg.contains("live"), "{msg}");
        assert!(msg.contains("log in"), "the reason must survive: {msg}");

        // Nothing posted is a different sentence, not "0 of 3 are live".
        let none = stopped_partway(0, 2, 1, Outcome::Interrupted("the composer never opened".into()));
        let Outcome::Interrupted(msg) = none else { panic!("still not a success") };
        assert!(msg.starts_with("Nothing was posted"), "{msg}");
    }

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
