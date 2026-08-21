//! Driving an app's composer to completion.
//!
//! WHAT THIS IS. After the share intent has put the media into the app's own
//! composer, this taps the remaining buttons — the ones the user would tap —
//! so a publish needs no manual step.
//!
//! WHAT THIS IS NOT, and the line is not negotiable:
//!
//!   * it never types a password or touches a login screen;
//!   * it never answers a captcha, a checkpoint, or a "was this you?" prompt;
//!   * it never dismisses a security or age-verification interstitial;
//!   * it never retries past a rate limit.
//!
//! [`Guard::inspect`] watches for exactly those screens, and the run STOPS when
//! one appears — the emulator is handed back to the person, who finishes it the
//! way they would on their phone. That is the same rule the rest of this
//! feature follows: the session is theirs, and anything that exists to confirm
//! a human is present must actually get one.
//!
//! FRAGILITY, stated plainly. This reads on-screen labels, so it breaks when an
//! app redesigns its composer. That is why it is opt-in, why every step has a
//! timeout, and why failing to find a button is a hand-off rather than a wild
//! tap: a mis-tap in a composer can post the wrong thing, which cannot be
//! undone.

use std::time::Duration;

use crate::errors::Result;
use crate::ldplayer::adb::{can_type, Adb, Match, UiNode};
use crate::publish::connector::PublishContext;
use crate::publish::model::{Platform, VideoFormat};

/// How long to wait for any single expected control to appear.
const STEP_TIMEOUT: Duration = Duration::from_secs(20);

/// One action in a composer recipe.
pub enum Step {
    /// Wait for any of these to appear, then tap the first one found.
    Tap {
        /// What it is, for the progress line and for the failure message.
        label: &'static str,
        any_of: Vec<Match>,
        /// When true, not finding it is fine — some apps show a step only
        /// sometimes (a "Next" that appears only for long videos, say).
        optional: bool,
    },
    /// Confirm we are on the screen we think we are, before touching anything.
    ///
    /// WHY THIS EXISTS: without it, the automation types and taps wherever it
    /// happens to be. A real case — YouTube opened on its home screen instead
    /// of the upload composer, and the caption went into the SEARCH BOX,
    /// because a text field was found and nothing had checked which one.
    Expect { label: &'static str, any_of: Vec<Match> },
    /// Type the caption into a matched field.
    ///
    /// `optional` is for flows where the field may genuinely not be there:
    /// Facebook's Reels editor has no caption box at all until its second
    /// screen, and refusing to post a reel because a field we were not sure
    /// about was missing helps nobody.
    Caption { into: Vec<Match>, optional: bool },
    /// Answer the Reel / Post / Story chooser, if the app puts one up.
    ///
    /// Sharing a video to Facebook sometimes offers a choice of what it should
    /// become, and the answer is the person's — it is carried on the job, not
    /// decided here. Skipped silently when no chooser appears, because whether
    /// one does depends on the video and the app version.
    ChooseFormat,
    /// Confirm the composer is posting as the identity the account expects.
    ///
    /// WHY THIS EXISTS: the app posts as whoever is currently active, and
    /// nothing in a share intent says otherwise. A session left switched to
    /// another profile publishes to that profile silently, and a post cannot
    /// be taken back. Reading the name off the composer is the one moment
    /// where that is answerable rather than assumed.
    ///
    /// Skipped when the account has no expected name, so this can ship
    /// without changing what existing accounts do.
    VerifyAuthor,
    /// Let the screen settle. Used sparingly; waiting on a control is better.
    Settle(u64),
}

impl Step {
    fn tap(label: &'static str, any_of: Vec<Match>) -> Self {
        Step::Tap { label, any_of, optional: false }
    }

    fn maybe(label: &'static str, any_of: Vec<Match>) -> Self {
        Step::Tap { label, any_of, optional: true }
    }

    fn expect(label: &'static str, any_of: Vec<Match>) -> Self {
        Step::Expect { label, any_of }
    }

    fn caption(into: Vec<Match>) -> Self {
        Step::Caption { into, optional: false }
    }

    fn caption_if_present(into: Vec<Match>) -> Self {
        Step::Caption { into, optional: true }
    }
}

/// How an automated run ended.
pub enum Run {
    /// Every step completed. The post was submitted.
    Posted,
    /// Stopped deliberately, with a sentence for the user explaining why and
    /// what is left to do.
    HandedBack(String),
}

/// Screens that must never be automated past.
///
/// Matched on labels rather than package names because these appear *inside*
/// the social app, not as a separate app: an Instagram checkpoint is still
/// Instagram. The cost of a false positive is a hand-off, which is safe; the
/// cost of a false negative is automating past a security check, which is not.
struct Guard;

impl Guard {
    const BLOCKING_LABELS: &'static [&'static str] = &[
        "log in",
        "login",
        "sign in",
        "password",
        "verify",
        "verification",
        "confirm your identity",
        "captcha",
        "security check",
        "suspicious",
        "we detected",
        "try again later",
        "action blocked",
        "enter the code",
        "two-factor",
    ];

    /// Whether the screen is one this must not touch, and why.
    fn inspect(nodes: &[UiNode]) -> Option<String> {
        for node in nodes {
            let haystack = format!("{} {}", node.text, node.content_desc).to_lowercase();
            if haystack.trim().is_empty() {
                continue;
            }
            for label in Self::BLOCKING_LABELS {
                if haystack.contains(label) {
                    let shown = if node.text.trim().is_empty() {
                        node.content_desc.trim()
                    } else {
                        node.text.trim()
                    };
                    return Some(shown.to_string());
                }
            }
        }
        None
    }
}

/// Fields the caption must never be typed into.
///
/// A search box is the dangerous one: it is an `EditText` like any other, it
/// is present on most app home screens, and typing into it looks to the
/// platform like a person searching for the text of their own caption.
fn is_safe_caption_field(node: &UiNode) -> bool {
    let haystack = format!(
        "{} {} {}",
        node.resource_id, node.text, node.content_desc
    )
    .to_lowercase();
    !["search", "query", "find"].iter().any(|bad| haystack.contains(bad))
}

/// Whether a hierarchy carries any readable label at all.
///
/// The distinction that matters: a dump that SUCCEEDS but describes a wall of
/// unlabelled `ViewGroup`s is a permanently unautomatable app, not a transient
/// failure and not the wrong screen.
fn has_labels(nodes: &[UiNode]) -> bool {
    nodes
        .iter()
        .any(|n| !n.text.trim().is_empty() || !n.content_desc.trim().is_empty())
}

/// The option to tap for `want` on a Reel / Post / Story chooser.
///
/// SAFETY RULE, and it is the whole design of this function: it returns
/// something only when at least two different formats are on screen. A lone
/// "Post" is not a chooser — it is the composer's submit button, and tapping
/// that before the caption is typed publishes an empty post. Requiring a
/// visible alternative is what tells a choice apart from a button.
fn format_option(nodes: &[UiNode], want: VideoFormat) -> Option<UiNode> {
    let kinds: [(&str, &[&str]); 3] = [
        ("post", &["Post", "Feed post", "Feed"]),
        ("reel", &["Reel", "Reels"]),
        ("story", &["Story", "Stories"]),
    ];

    let hit = |labels: &[&str]| -> Option<UiNode> {
        labels.iter().find_map(|l| {
            let by_text = Match::Text((*l).into());
            let by_desc = Match::Desc((*l).into());
            nodes
                .iter()
                .find(|n| n.is_tappable() && (by_text.matches(n) || by_desc.matches(n)))
                .cloned()
        })
    };

    let present: Vec<&str> = kinds
        .iter()
        .filter(|(_, labels)| hit(labels).is_some())
        .map(|(kind, _)| *kind)
        .collect();
    if present.len() < 2 {
        return None;
    }

    let wanted = match want {
        VideoFormat::Post => kinds[0].1,
        VideoFormat::Reel => kinds[1].1,
    };
    hit(wanted)
}

/// Whether the composer says it is posting as `expected`.
///
/// Compared trimmed, because the app pads some of these labels — a real dump
/// carried both "Sambath Sotheareach" and "Sambath Sotheareach " — and an
/// identity check that fails on a trailing space would block every post.
///
/// Exact rather than substring: "Acme Store" must not satisfy a check for
/// "Acme", or the safeguard would wave through the very confusion it exists
/// to catch.
fn author_is(nodes: &[UiNode], expected: &str) -> bool {
    let expected = expected.trim();
    nodes.iter().any(|n| {
        n.text.trim().eq_ignore_ascii_case(expected)
            || n.content_desc.trim().eq_ignore_ascii_case(expected)
    })
}

/// Whether the caption is already sitting in a text field on this screen.
///
/// WHY THIS EXISTS: the share intent delivers the caption via `EXTRA_TEXT`,
/// and once it lands the field stops reading "What's on your mind?" and starts
/// reading the caption. Matchers written against the placeholder therefore
/// find nothing on exactly the posts where the hand-off worked best — the step
/// waited out its timeout and handed back a composer that was complete.
///
/// Restricted to real text-entry widgets. "Does any node contain the caption"
/// would also match the caption rendered elsewhere on screen, and a short one
/// ("Hi") turns up inside ordinary words.
fn caption_field_holds_it(nodes: &[UiNode], caption: &str) -> bool {
    nodes.iter().any(|n| {
        n.class.contains("EditText") && is_safe_caption_field(n) && field_already_has(n, caption)
    })
}

/// Whether the caption appears anywhere on screen, in any kind of node.
///
/// Looser, and used only once the placeholder has already failed to appear:
/// at that point the choice is between posting with a caption the app itself
/// inserted and handing back a finished composer, and the first is what the
/// person asked for.
fn caption_is_on_screen(nodes: &[UiNode], caption: &str) -> bool {
    nodes.iter().any(|n| field_already_has(n, caption))
}

/// Whether a field already contains the caption.
///
/// Compared on a prefix rather than in full: apps ellipsize long values in the
/// hierarchy ("Check out my new vid…"), so an equality test would retype a
/// caption that is already there and post it twice over.
fn field_already_has(node: &UiNode, caption: &str) -> bool {
    let caption = caption.trim();
    if caption.is_empty() {
        return true;
    }
    let head: String = caption.chars().take(15).collect();
    node.text.contains(head.as_str())
}

/// A short, readable summary of what the screen is showing.
///
/// Deduplicated and capped: a hierarchy has hundreds of nodes and most carry
/// no text, but the handful that do are usually enough to recognise the screen
/// ("Home, Shorts, Subscriptions" is a home screen, not a composer).
fn visible_labels(nodes: &[UiNode]) -> String {
    let mut seen: Vec<String> = Vec::new();
    for n in nodes {
        let label = if n.text.trim().is_empty() {
            n.content_desc.trim()
        } else {
            n.text.trim()
        };
        if label.is_empty() || label.len() > 40 {
            continue;
        }
        if !seen.iter().any(|s| s == label) {
            seen.push(label.to_string());
        }
        if seen.len() == 8 {
            break;
        }
    }
    seen.iter()
        .map(|s| format!("\u{201c}{s}\u{201d}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether this is Facebook's Reels editor rather than its post composer.
///
/// Sharing a video to Facebook does not always land in the same place: a video
/// can open the Reels editor instead, which has no caption box, no author
/// line, and a different order — trim and effects first, Next, and only then a
/// description and "Share now". Running the post recipe there fails at the
/// screen check, which is what it is for, but the answer is a different recipe
/// rather than a hand-off.
///
/// Matched on the editor's own tools. "Add audio" is the giveaway: the post
/// composer has no such control.
fn is_reel_editor(nodes: &[UiNode]) -> bool {
    let anchors = [
        Match::TextContains("Add audio".into()),
        Match::DescContains("Add audio".into()),
        Match::TextContains("Discover suggestions".into()),
    ];
    anchors.iter().any(|m| nodes.iter().any(|n| m.matches(n)))
}

/// The recipe to run, chosen from what is actually on screen.
pub fn recipe_for(platform: Platform, nodes: &[UiNode]) -> Vec<Step> {
    if platform == Platform::Facebook && is_reel_editor(nodes) {
        return reel_recipe();
    }
    recipe(platform)
}

/// Facebook's Reels flow.
///
/// Built from a real editor screen: a row of Audio / Edit / Effects / Text /
/// Stickers tools and a Next arrow, then a second screen carrying the
/// description and "Share now".
///
/// The caption step is optional here, and deliberately: the second screen's
/// field has not been read from a live instance, so its labels are the best
/// guess in this file. A reel that posts without its description is a worse
/// outcome than one with it — but far better than one that does not post at
/// all because a matcher was wrong.
fn reel_recipe() -> Vec<Step> {
    vec![
        Step::Settle(2),
        Step::ChooseFormat,
        Step::Settle(2),
        Step::expect(
            "Facebook's Reels editor",
            vec![
                Match::TextContains("Add audio".into()),
                Match::DescContains("Add audio".into()),
                Match::TextContains("Discover suggestions".into()),
            ],
        ),
        // The editor's Next arrow. Required, not optional: without it the run
        // would sit on the editing screen and tap Share on nothing.
        Step::tap(
            "the Reels editor's Next button",
            vec![Match::Text("Next".into()), Match::Desc("Next".into())],
        ),
        Step::Settle(3),
        Step::expect(
            "the Reels description screen",
            vec![
                Match::TextContains("Share now".into()),
                Match::DescContains("Share now".into()),
                Match::TextContains("Describe your reel".into()),
                Match::TextContains("Add a description".into()),
            ],
        ),
        // Only meaningful once an account has been told who it posts as; it
        // skips itself otherwise. Placed here because this is the first Reels
        // screen that names an audience at all.
        Step::VerifyAuthor,
        Step::caption_if_present(vec![
            Match::TextContains("Describe your reel".into()),
            Match::TextContains("Add a description".into()),
            Match::TextContains("Say something about".into()),
        ]),
        Step::tap(
            "the Share now button",
            vec![
                Match::Text("Share now".into()),
                Match::Desc("Share now".into()),
                Match::Text("Share".into()),
                Match::Text("Post".into()),
            ],
        ),
    ]
}

/// The composer recipe for a platform.
///
/// Labels only — no coordinates. A tap at a fixed position is a different
/// button on a different screen size, and these instances vary.
pub fn recipe(platform: Platform) -> Vec<Step> {
    match platform {
        // The share intent attaches the video and, for most post types, the
        // caption too — so little is left to drive here, which makes Facebook
        // the least fragile of the four. Labels below come from real composer
        // screenshots, not guesses: "Create post", "New post", "What's on your
        // mind?", "Say something about this photo…", "Next", "Post".
        Platform::Facebook => vec![
            Step::Settle(2),
            // Answering this decides WHICH composer opens next, so it has to
            // come before the screen check that identifies one.
            Step::ChooseFormat,
            Step::Settle(2),
            Step::expect(
                "Facebook's composer",
                vec![
                    // "New post" is the current header; "Create post" is the
                    // older one. Both stay, because instances run different
                    // app versions. The chips are the sturdier signal — they
                    // are unmistakably the composer and survived the redesign
                    // that renamed the header.
                    Match::TextContains("New post".into()),
                    Match::TextContains("Create post".into()),
                    Match::TextContains("Tag/collaborate".into()),
                    Match::TextContains("Feeling/activity".into()),
                    Match::TextContains("What's on your mind".into()),
                    Match::TextContains("Say something about".into()),
                    Match::Text("Post".into()),
                ],
            ),
            // The share intent usually delivers the caption via EXTRA_TEXT, but
            // not for every post type — the photo composer comes up empty. So
            // type it, and let the step skip itself when it is already there.
            //
            // BEFORE Next, not after: "What's on your mind?" lives on the
            // first screen of the current composer, and Next is what leaves
            // that screen. Tapping Next first left the caption step waiting
            // out its timeout on a screen the field was no longer on, which
            // handed back a post that was one tap from going out.
            // Before Next, because the author's name is only rendered on this
            // first screen — the "Post settings" screen that carries SHARE
            // does not show it, so checking there would be checking nothing.
            // Tapping Next cannot change who is posting, so confirming here
            // holds for the submit two screens later.
            Step::VerifyAuthor,
            Step::caption(vec![
                    Match::TextContains("Say something about".into()),
                    Match::TextContains("What's on your mind".into()),
                ]),
            Step::maybe("the composer's Next button", vec![Match::Text("Next".into())]),
            // Next lands on a "Post settings" screen whose submit button is
            // SHARE, not Post — so both names are here, and `Match::Text` is
            // an exact (case-insensitive) comparison, which is what keeps
            // "Share" off that screen's "Share to Story" and "Share to
            // Facebook Groups" rows.
            Step::tap(
                "the Post button",
                vec![
                    Match::Text("Post".into()),
                    Match::Desc("Post".into()),
                    Match::ResourceId("id/post_button".into()),
                    Match::Text("Share".into()),
                    Match::Desc("Share".into()),
                    Match::Text("Share now".into()),
                ],
            ),
        ],

        // Instagram ignores EXTRA_TEXT, so the caption is typed, then two
        // screens: the edit step, then Share.
        Platform::Instagram => vec![
            Step::Settle(3),
            Step::maybe("the Next button", vec![Match::Text("Next".into())]),
            Step::maybe("the second Next button", vec![Match::Text("Next".into())]),
            Step::expect(
                "Instagram's caption screen",
                vec![
                    Match::TextContains("Write a caption".into()),
                    Match::TextContains("New post".into()),
                    Match::TextContains("New reel".into()),
                ],
            ),
            Step::caption(vec![Match::TextContains("Write a caption".into())]),
            Step::tap(
                "the Share button",
                vec![Match::Text("Share".into()), Match::Desc("Share".into())],
            ),
        ],

        Platform::Tiktok => vec![
            Step::Settle(3),
            Step::maybe("the Next button", vec![Match::Text("Next".into())]),
            Step::expect(
                "TikTok's post screen",
                vec![
                    Match::TextContains("Describe your video".into()),
                    Match::TextContains("Post to".into()),
                ],
            ),
            Step::caption(vec![Match::TextContains("Describe your video".into())]),
            Step::tap(
                "the Post button",
                vec![Match::Text("Post".into()), Match::Desc("Post".into())],
            ),
        ],

        // YouTube's upload flow wants a title before it will let you continue,
        // and the final control is named differently across versions.
        Platform::Youtube => vec![
            Step::Settle(3),
            // Sharing to YouTube can land on its home screen instead of the
            // upload flow — which is exactly how a caption ended up in the
            // search box. Prove we are on the upload screen first.
            Step::expect(
                "YouTube's upload screen",
                vec![
                    Match::TextContains("Add a title".into()),
                    Match::TextContains("Add a description".into()),
                    Match::TextContains("Upload video".into()),
                    Match::TextContains("Details".into()),
                ],
            ),
            Step::caption(vec![
                    Match::TextContains("Add a title".into()),
                    Match::TextContains("Title".into()),
                ]),
            Step::maybe("the Next button", vec![Match::Text("Next".into())]),
            Step::tap(
                "the Upload button",
                vec![
                    Match::Text("Upload".into()),
                    Match::Text("Publish".into()),
                    Match::Desc("Upload".into()),
                ],
            ),
        ],
    }
}

/// Run a recipe against the device.
///
/// Every exit that is not `Posted` carries a sentence naming what it was
/// looking for and what is left to do, because a half-finished composer the
/// user cannot interpret is worse than never having automated it.
pub async fn run(ctx: &PublishContext, adb: &Adb, platform: Platform) -> Result<Run> {
    let label = ctx.platform_label;
    let noun = ctx.noun();

    // Can this app be automated at all?
    //
    // Some apps — Facebook Lite is the one that proved it — render their whole
    // UI to a canvas and expose NOTHING to Android's accessibility tree. A dump
    // of its composer returns 56 nodes with not one label between them. No
    // matcher can ever succeed there, so trying step after step and blaming the
    // screen is dishonest: the answer is "not this app", and it will not change
    // on a retry.
    let opening = match adb.ui_dump(&ctx.serial).await {
        Ok(nodes) if !has_labels(&nodes) => {
            return Ok(Run::HandedBack(format!(
                "{label} draws its own screen and gives Android no readable labels, so \
                 the Post button can't be found — this app can't be automated, and \
                 retrying won't change that. The {noun} is attached and ready: tap Post \
                 in LDPlayer. (If this is the Lite version, the full app supports \
                 automation.)"
            )));
        }
        Ok(nodes) => nodes,
        Err(e) => {
            return Ok(Run::HandedBack(format!(
                "Couldn't read {label}'s screen ({e}), so nothing was tapped. The {noun} \
                 is attached — finish this one in LDPlayer."
            )))
        }
    };

    // Which composer did the app actually open? Sharing a video to Facebook
    // lands in the Reels editor as readily as the post composer, and the two
    // want different steps in a different order. Choosing from the screen in
    // front of us beats assuming the app did what it did last time.
    let steps = recipe_for(platform, &opening);
    let total = steps.len() as f64;

    for (i, step) in steps.iter().enumerate() {
        // Progress across the automation band, 0.80 -> 0.97.
        let progress = 0.80 + 0.17 * (i as f64) / total.max(1.0);

        // Before every action: is this a screen we must not touch?
        if let Ok(nodes) = adb.ui_dump(&ctx.serial).await {
            if let Some(what) = Guard::inspect(&nodes) {
                return Ok(Run::HandedBack(format!(
                    "{label} is asking you to confirm something (\u{201c}{what}\u{201d}), so \
                     this app stopped. Open LDPlayer for this instance, deal with it the \
                     way you normally would, then tap Post — the {noun} is already attached."
                )));
            }
        }

        match step {
            Step::Settle(secs) => {
                tokio::time::sleep(Duration::from_secs(*secs)).await;
            }

            Step::Expect { label: what, any_of } => {
                ctx.step(progress, &format!("Checking for {what}…"));
                let found = match adb.wait_for_node(&ctx.serial, any_of, STEP_TIMEOUT).await {
                    Ok(found) => found,
                    // Could not READ the screen. Saying "wrong screen" here is
                    // what sent someone to inspect a composer that was showing
                    // exactly the right thing.
                    Err(e) => {
                        return Ok(Run::HandedBack(format!(
                            "{label} is open, but this app couldn't read what's on its \
                             screen ({e}), so it didn't tap anything. Finish this one in \
                             LDPlayer; the {noun} is already attached."
                        )))
                    }
                };
                if found.is_none() {
                    // Say what WAS there. "This doesn't look like the upload
                    // screen" is unactionable on its own — the labels and the
                    // foreground package are what turn it into a fix, and
                    // gathering them costs one dump on a path that has already
                    // failed.
                    let seen = adb.ui_dump(&ctx.serial).await.unwrap_or_default();
                    let front = adb.foreground_package(&ctx.serial).await;
                    let gone = front
                        .as_deref()
                        .is_none_or(|p| !p.starts_with(&ctx.package));

                    let detail = if gone {
                        match front {
                            Some(p) => format!(" {label} is no longer in front — `{p}` is."),
                            None => format!(" {label} appears to have closed."),
                        }
                    } else {
                        match visible_labels(&seen) {
                            labels if labels.is_empty() => String::new(),
                            labels => format!(" What's on screen: {labels}."),
                        }
                    };

                    return Ok(Run::HandedBack(format!(
                        "This doesn't look like {what}, so nothing was typed or tapped \
                         — acting on the wrong screen is how a caption ends up in a \
                         search box.{detail} The {noun} is already in the gallery; open \
                         LDPlayer for this instance and finish there."
                    )));
                }
            }

            Step::Tap { label: what, any_of, optional } => {
                ctx.step(progress, &format!("Looking for {what}…"));
                let found = match adb.wait_for_node(&ctx.serial, any_of, STEP_TIMEOUT).await {
                    Ok(found) => found,
                    Err(e) => {
                        return Ok(Run::HandedBack(format!(
                            "Couldn't read {label}'s screen to find {what} ({e}). Nothing \
                             was tapped — finish this one in LDPlayer; the {noun} is \
                             already attached."
                        )))
                    }
                };
                match found {
                    Some(node) => {
                        ctx.step(progress, &format!("Tapping {what}"));
                        adb.tap_node(&ctx.serial, &node).await?;
                        tokio::time::sleep(Duration::from_millis(1500)).await;
                    }
                    None if *optional => continue,
                    None => {
                        // Never guess. A tap at the wrong place in a composer
                        // can publish the wrong thing, and that is not undoable.
                        return Ok(Run::HandedBack(format!(
                            "Couldn't find {what} in {label} — the app's layout may have \
                             changed in an update. Nothing was tapped. Open LDPlayer for \
                             this instance and finish there; the {noun} is already attached."
                        )));
                    }
                }
            }

            Step::ChooseFormat => {
                let want = ctx.video_format;
                let Ok(nodes) = adb.ui_dump(&ctx.serial).await else {
                    // No chooser can be confirmed, so none is answered. The
                    // steps after this one wait on their own anchors anyway.
                    continue;
                };
                let Some(option) = format_option(&nodes, want) else {
                    continue;
                };
                ctx.step(progress, &format!("Choosing {}", want.label()));
                adb.tap_node(&ctx.serial, &option).await?;
                tokio::time::sleep(Duration::from_millis(1800)).await;
            }

            Step::VerifyAuthor => {
                let Some(expected) = ctx.expected_author.as_deref() else {
                    continue;
                };
                ctx.step(progress, &format!("Checking this posts as {expected}…"));

                let nodes = match adb.ui_dump(&ctx.serial).await {
                    Ok(nodes) => nodes,
                    // Cannot read the screen, so cannot confirm the identity.
                    // Posting anyway would be the guess this step exists to
                    // refuse.
                    Err(e) => {
                        return Ok(Run::HandedBack(format!(
                            "Couldn't read {label}'s screen to confirm this would post as                              {expected} ({e}), so nothing was submitted. The {noun} is                              attached — finish this one in LDPlayer."
                        )))
                    }
                };

                if !author_is(&nodes, expected) {
                    return Ok(Run::HandedBack(format!(
                        "This composer isn't posting as {expected}, so nothing was                          submitted — a post cannot be taken back, and publishing to the                          wrong profile or Page is the one mistake worth stopping for.                          What's on screen: {}. Check which profile {label} is switched                          to on this instance.",
                        visible_labels(&nodes)
                    )));
                }
            }

            Step::Caption { into, optional } => {
                if ctx.caption.trim().is_empty() {
                    continue;
                }
                if !can_type(&ctx.caption) {
                    return Ok(Run::HandedBack(format!(
                        "The {noun} is attached and {label} is open, but this caption has \
                         characters Android can't type from outside an app (it only \
                         accepts plain ASCII this way). Paste it in {label} and tap Post."
                    )));
                }
                // Already delivered by the share intent? Then the field no
                // longer shows the placeholder these matchers look for, and
                // waiting on it would burn the timeout before handing back a
                // composer that is finished.
                if let Ok(nodes) = adb.ui_dump(&ctx.serial).await {
                    if caption_field_holds_it(&nodes, &ctx.caption) {
                        ctx.step(progress, "Caption is already filled in");
                        continue;
                    }
                }

                ctx.step(progress, "Typing the caption…");
                let found = match adb.wait_for_node(&ctx.serial, into, STEP_TIMEOUT).await {
                    Ok(found) => found,
                    Err(e) => {
                        return Ok(Run::HandedBack(format!(
                            "Couldn't read {label}'s screen to find the caption field \
                             ({e}). Add the caption there and tap Post; the {noun} is \
                             already attached."
                        )))
                    }
                };
                match found {
                    // Refuse a search box even if a matcher somehow reached
                    // one. Belt and braces: the Expect step above should have
                    // caught the wrong screen, and this catches the wrong field
                    // on the right screen.
                    Some(field) if !is_safe_caption_field(&field) => {
                        return Ok(Run::HandedBack(format!(
                            "The caption field {label} offered looks like a search box, \
                             so nothing was typed. Add the caption in {label} and tap \
                             Post; the {noun} is already attached."
                        )));
                    }
                    // Already carrying the caption — EXTRA_TEXT delivered it.
                    // Typing again would duplicate it in the post.
                    Some(field) if field_already_has(&field, &ctx.caption) => {
                        ctx.step(progress, "Caption is already filled in");
                    }
                    Some(field) => {
                        adb.tap_node(&ctx.serial, &field).await?;
                        tokio::time::sleep(Duration::from_millis(700)).await;
                        if let Err(e) = adb.type_text(&ctx.serial, &ctx.caption).await {
                            return Ok(Run::HandedBack(format!(
                                "The {noun} is attached but the caption couldn't be typed \
                                 ({e}). Add it in {label} and tap Post."
                            )));
                        }
                        // Close the keyboard so it does not cover the button
                        // the next step needs to find.
                        adb.press_back(&ctx.serial).await.ok();
                        tokio::time::sleep(Duration::from_millis(600)).await;
                    }
                    None => {
                        // One last look before giving up. An app that renders
                        // its caption box as something other than an EditText
                        // — Facebook's composer is Litho-drawn and has moved
                        // between widget classes across releases — passes the
                        // strict check above while still showing the caption,
                        // and handing back a post that needs only its final
                        // tap is the worse of the two mistakes.
                        let showing = adb
                            .ui_dump(&ctx.serial)
                            .await
                            .map(|nodes| caption_is_on_screen(&nodes, &ctx.caption))
                            .unwrap_or(false);
                        if showing {
                            ctx.step(progress, "Caption is already filled in");
                            continue;
                        }
                        // A flow that may not have a caption box here says so,
                        // and the post still goes out. Refusing to publish
                        // over a field we were never sure existed would be the
                        // automation inventing a problem.
                        if *optional {
                            ctx.step(progress, "No caption field on this screen — posting without one");
                            continue;
                        }
                        return Ok(Run::HandedBack(format!(
                            "The {noun} is attached but the caption field never appeared in \
                             {label}. Add the caption there and tap Post."
                        )));
                    }
                }
            }
        }
    }

    // One last look: if a security screen came up in response to the final tap,
    // the post did not go out and saying it did would be a lie.
    tokio::time::sleep(Duration::from_secs(3)).await;
    if let Ok(nodes) = adb.ui_dump(&ctx.serial).await {
        if let Some(what) = Guard::inspect(&nodes) {
            return Ok(Run::HandedBack(format!(
                "{label} responded with \u{201c}{what}\u{201d} after the post was submitted, \
                 so it may not have gone out. Check this instance in LDPlayer."
            )));
        }
    }

    Ok(Run::Posted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldplayer::adb::{find_node, parse_ui_nodes};

    /// The recipe's screen check. Found rather than indexed: steps get added
    /// ahead of it — answering the Reel/Post chooser, for one — and a test
    /// that hard-codes position 1 fails for a reason that has nothing to do
    /// with what it is checking.
    fn screen_check(steps: &[Step]) -> &'static str {
        steps
            .iter()
            .find_map(|s| match s {
                Step::Expect { label, .. } => Some(*label),
                _ => None,
            })
            .expect("every Facebook recipe confirms the screen before acting")
    }

    fn nodes(text: &str) -> Vec<UiNode> {
        parse_ui_nodes(&format!(
            r#"<node text="{text}" resource-id="" class="x" content-desc="" clickable="true" enabled="true" bounds="[0,0][100,50]"/>"#
        ))
    }

    #[test]
    fn every_platform_has_a_recipe_ending_in_a_required_tap() {
        for p in Platform::ALL {
            let steps = recipe(*p);
            assert!(!steps.is_empty(), "{p:?} has no recipe");
            match steps.last().unwrap() {
                Step::Tap { optional, .. } => {
                    assert!(!optional, "{p:?} ends on an optional step, so it may never post")
                }
                _ => panic!("{p:?} does not end by tapping a submit control"),
            }
        }
    }

    /// The bug this exists for: "What's on your mind?" is on the FIRST screen
    /// of Facebook's composer and Next is what leaves it, so tapping Next
    /// before typing stranded the caption step on a screen the field was no
    /// longer on — a hand-off one tap short of posting. Instagram is the
    /// opposite (its caption screen sits behind two Nexts), which is why the
    /// order is asserted per platform rather than globally.
    #[test]
    fn facebook_captions_before_it_leaves_the_first_screen() {
        let steps = recipe(Platform::Facebook);
        let position = |pred: &dyn Fn(&Step) -> bool| steps.iter().position(|s| pred(s));

        let caption = position(&|s| matches!(s, Step::Caption { .. }))
            .expect("Facebook's recipe types a caption when EXTRA_TEXT did not carry one");
        let next = position(&|s| match s {
            Step::Tap { any_of, .. } => any_of
                .iter()
                .any(|m| matches!(m, Match::Text(t) if t.eq_ignore_ascii_case("next"))),
            _ => false,
        })
        .expect("Facebook's recipe advances past the first screen");

        assert!(caption < next, "the caption must be typed before Next leaves its screen");
    }

    /// Built from a real dump of the composer this failed on: the header now
    /// reads "New post", and none of "Create post", "What's on your mind" or
    /// "Say something about" was anywhere on screen — the caption had already
    /// replaced the placeholder — so the screen check rejected the very
    /// composer it was waiting for.
    #[test]
    fn the_current_facebook_composer_is_recognised() {
        let screen = concat!(
            r#"<node text="" resource-id="" class="android.widget.Button" content-desc="Close" clickable="true" enabled="true" bounds="[20,90][80,150]"/>"#,
            r#"<node text="New post" resource-id="" class="android.widget.TextView" content-desc="" clickable="false" enabled="true" bounds="[200,90][500,150]"/>"#,
            r#"<node text="Tag/collaborate" resource-id="" class="android.widget.TextView" content-desc="" clickable="true" enabled="true" bounds="[20,340][300,400]"/>"#,
            r#"<node text="Hi" resource-id="" class="android.widget.EditText" content-desc="" clickable="true" enabled="true" bounds="[20,450][660,520]"/>"#,
            r#"<node text="Next" resource-id="" class="android.widget.Button" content-desc="" clickable="true" enabled="true" bounds="[510,1170][670,1240]"/>"#,
        );
        let nodes = parse_ui_nodes(screen);

        let steps = recipe(Platform::Facebook);
        let expect = steps
            .iter()
            .find_map(|s| match s {
                Step::Expect { any_of, .. } => Some(any_of.clone()),
                _ => None,
            })
            .expect("the composer is confirmed before anything is typed");
        assert!(
            expect.iter().any(|m| find_node(&nodes, m, false).is_some()),
            "this composer must be recognised, not handed back"
        );
    }

    /// The other half of the same failure: the share intent had already put
    /// the caption in, so the field no longer read "What's on your mind?" and
    /// the placeholder matchers could never find it.
    #[test]
    fn a_caption_the_intent_already_delivered_is_not_hunted_for() {
        let filled = parse_ui_nodes(
            r#"<node text="Hi" resource-id="" class="android.widget.EditText" content-desc="" clickable="true" enabled="true" bounds="[20,450][660,520]"/>"#,
        );
        assert!(caption_field_holds_it(&filled, "Hi"));

        let empty = parse_ui_nodes(
            r#"<node text="What's on your mind?" resource-id="" class="android.widget.EditText" content-desc="" clickable="true" enabled="true" bounds="[20,450][660,520]"/>"#,
        );
        assert!(!caption_field_holds_it(&empty, "Hi"), "an empty box must still be typed into");

        // A label that merely contains the caption is not a filled-in field:
        // short captions turn up inside ordinary words, and skipping on one
        // would post with no caption at all.
        let coincidence = parse_ui_nodes(
            r#"<node text="History" resource-id="" class="android.widget.TextView" content-desc="" clickable="true" enabled="true" bounds="[20,450][660,520]"/>"#,
        );
        assert!(!caption_field_holds_it(&coincidence, "Hi"));

        // The search box rule still applies to a field that looks filled.
        let search = parse_ui_nodes(
            r#"<node text="Hi" resource-id="com.facebook.katana:id/search_box" class="android.widget.EditText" content-desc="" clickable="true" enabled="true" bounds="[20,450][660,520]"/>"#,
        );
        assert!(!caption_field_holds_it(&search, "Hi"));
    }

    /// Built from the "Post settings" screen Next actually lands on: the
    /// submit button reads SHARE, and the page is a list of rows several of
    /// which start with the word Share. The submit button must be the one
    /// found, and none of the rows may be.
    #[test]
    fn the_share_button_is_found_on_the_post_settings_screen() {
        let screen = concat!(
            r#"<node text="Post settings" resource-id="" class="android.widget.TextView" content-desc="" clickable="false" enabled="true" bounds="[60,110][250,160]"/>"#,
            r#"<node text="SHARE" resource-id="" class="android.widget.Button" content-desc="" clickable="true" enabled="true" bounds="[486,105][614,165]"/>"#,
            r#"<node text="Post audience" resource-id="" class="android.widget.TextView" content-desc="" clickable="true" enabled="true" bounds="[84,200][400,240]"/>"#,
            r#"<node text="Share to Story" resource-id="" class="android.widget.TextView" content-desc="" clickable="true" enabled="true" bounds="[84,740][400,780]"/>"#,
            r#"<node text="Share to Facebook Groups" resource-id="" class="android.widget.TextView" content-desc="" clickable="true" enabled="true" bounds="[84,840][540,880]"/>"#,
        );
        let nodes = parse_ui_nodes(screen);

        let submit = match recipe(Platform::Facebook).pop() {
            Some(Step::Tap { any_of, .. }) => any_of,
            _ => panic!("Facebook's recipe ends by tapping submit"),
        };
        let hit = submit
            .iter()
            .find_map(|m| find_node(&nodes, m, false))
            .expect("SHARE must be found");
        assert_eq!(hit.text, "SHARE", "a Share-to-… row is not the submit button");
    }

    /// The name is compared trimmed because a real dump of this account's own
    /// profile carried both "Sambath Sotheareach" and "Sambath Sotheareach "
    /// — an identity check that tripped on a trailing space would block every
    /// post instead of the wrong ones.
    #[test]
    fn the_author_check_reads_the_name_the_app_renders() {
        let composer = concat!(
            r#"<node text="New post" resource-id="" class="android.widget.TextView" content-desc="" clickable="false" enabled="true" bounds="[200,90][500,150]"/>"#,
            r#"<node text="Sambath Sotheareach " resource-id="" class="android.widget.TextView" content-desc="" clickable="false" enabled="true" bounds="[160,220][490,270]"/>"#,
        );
        let nodes = parse_ui_nodes(composer);
        assert!(author_is(&nodes, "Sambath Sotheareach"));

        // A different identity is exactly what this must catch: the app posts
        // as whoever is active, and a switched session is silent.
        assert!(!author_is(&nodes, "Acme Store"));
    }

    /// Exact, not substring: a Page called "Acme" must not be satisfied by a
    /// composer posting as "Acme Store", or the check waves through the
    /// confusion it exists to stop.
    #[test]
    fn a_similar_name_is_not_the_same_identity() {
        let nodes = parse_ui_nodes(
            r#"<node text="Acme Store" resource-id="" class="android.widget.TextView" content-desc="" clickable="false" enabled="true" bounds="[160,220][490,270]"/>"#,
        );
        assert!(!author_is(&nodes, "Acme"));
        assert!(author_is(&nodes, "acme store"), "case alone must not fail a post");
    }

    /// The name is only rendered on the composer; the "Post settings" screen
    /// that carries SHARE does not show it. Checking after Next would be
    /// checking an empty screen, so the order is part of the safeguard.
    #[test]
    fn the_author_is_checked_before_the_composer_is_left() {
        let steps = recipe(Platform::Facebook);
        let verify = steps
            .iter()
            .position(|s| matches!(s, Step::VerifyAuthor))
            .expect("Facebook confirms who it posts as");
        let next = steps
            .iter()
            .position(|s| match s {
                Step::Tap { any_of, .. } => any_of
                    .iter()
                    .any(|m| matches!(m, Match::Text(t) if t.eq_ignore_ascii_case("next"))),
                _ => false,
            })
            .expect("Facebook advances past the first screen");
        assert!(verify < next, "the identity must be confirmed while it is on screen");
    }

    /// Built from the Reels editor a shared video actually opened: an audio
    /// bar across the top, a tool row, and a Next arrow. None of the post
    /// composer's labels are on it, which is why the post recipe rejected the
    /// screen and handed back a video that was one flow away from posting.
    const REEL_EDITOR: &str = concat!(
        r#"<node text="Add audio" resource-id="" class="android.widget.TextView" content-desc="" clickable="true" enabled="true" bounds="[240,80][560,130]"/>"#,
        r#"<node text="Discover suggestions" resource-id="" class="android.widget.TextView" content-desc="" clickable="false" enabled="true" bounds="[240,130][620,170]"/>"#,
        r#"<node text="Audio" resource-id="" class="android.widget.TextView" content-desc="" clickable="true" enabled="true" bounds="[40,1330][120,1370]"/>"#,
        r#"<node text="Effects" resource-id="" class="android.widget.TextView" content-desc="" clickable="true" enabled="true" bounds="[290,1330][380,1370]"/>"#,
        r#"<node text="Next" resource-id="" class="android.widget.Button" content-desc="" clickable="true" enabled="true" bounds="[650,1230][760,1370]"/>"#,
    );

    #[test]
    fn a_shared_video_that_opens_reels_gets_the_reels_recipe() {
        let nodes = parse_ui_nodes(REEL_EDITOR);
        assert!(is_reel_editor(&nodes));

        // And the recipe chosen for it is not the post one, which would fail
        // its screen check here.
        let steps = recipe_for(Platform::Facebook, &nodes);
        assert!(screen_check(&steps).contains("Reels"), "{}", screen_check(&steps));
    }

    /// The post composer must keep its own recipe. The two flows differ in
    /// step order, so picking the wrong one is not a near miss.
    #[test]
    fn the_ordinary_composer_still_gets_the_post_recipe() {
        let composer = parse_ui_nodes(
            r#"<node text="New post" resource-id="" class="android.widget.TextView" content-desc="" clickable="false" enabled="true" bounds="[200,90][500,150]"/>"#,
        );
        assert!(!is_reel_editor(&composer));
        let steps = recipe_for(Platform::Facebook, &composer);
        assert!(screen_check(&steps).contains("composer"), "{}", screen_check(&steps));
    }

    /// Reels goes Next FIRST and describes the post afterwards — the reverse
    /// of the post composer, where the caption box is on the first screen and
    /// Next leaves it. Getting this backwards is what the two recipes exist to
    /// keep apart.
    #[test]
    fn reels_advances_before_it_describes_the_post() {
        let steps = reel_recipe();
        let next = steps
            .iter()
            .position(|s| match s {
                Step::Tap { any_of, .. } => any_of
                    .iter()
                    .any(|m| matches!(m, Match::Text(t) if t.eq_ignore_ascii_case("next"))),
                _ => false,
            })
            .expect("the editor advances");
        let caption = steps
            .iter()
            .position(|s| matches!(s, Step::Caption { .. }))
            .expect("a reel can carry a description");
        assert!(next < caption, "the description screen is behind Next");

        // The description field has not been read from a live instance, so a
        // missing one must not block the post.
        match &steps[caption] {
            Step::Caption { optional, .. } => assert!(*optional),
            _ => unreachable!(),
        }
    }

    /// THE HAZARD this guards: "Post" is a chooser option AND the composer's
    /// submit button. Tapping it as a chooser while the composer is up would
    /// publish before the caption is typed — unrecoverable. So a choice is
    /// only recognised when a second format is on screen to choose between.
    #[test]
    fn a_lone_post_button_is_not_a_format_chooser() {
        let composer = parse_ui_nodes(concat!(
            r#"<node text="New post" resource-id="" class="android.widget.TextView" content-desc="" clickable="false" enabled="true" bounds="[200,90][500,150]"/>"#,
            r#"<node text="Post" resource-id="" class="android.widget.Button" content-desc="" clickable="true" enabled="true" bounds="[486,105][614,165]"/>"#,
        ));
        assert!(format_option(&composer, VideoFormat::Post).is_none());
        assert!(format_option(&composer, VideoFormat::Reel).is_none());
    }

    #[test]
    fn a_real_chooser_answers_with_the_format_the_job_asked_for() {
        let chooser = parse_ui_nodes(concat!(
            r#"<node text="Reel" resource-id="" class="android.widget.TextView" content-desc="" clickable="true" enabled="true" bounds="[100,600][400,700]"/>"#,
            r#"<node text="Post" resource-id="" class="android.widget.TextView" content-desc="" clickable="true" enabled="true" bounds="[100,720][400,820]"/>"#,
            r#"<node text="Story" resource-id="" class="android.widget.TextView" content-desc="" clickable="true" enabled="true" bounds="[100,840][400,940]"/>"#,
        ));

        assert_eq!(format_option(&chooser, VideoFormat::Reel).unwrap().text, "Reel");
        assert_eq!(format_option(&chooser, VideoFormat::Post).unwrap().text, "Post");
    }

    /// A chooser offering only Story and Reel cannot answer "Post". Tapping
    /// the nearest thing would publish the wrong kind of post, so it taps
    /// nothing and the screen check downstream reports what it found.
    #[test]
    fn an_unavailable_format_is_not_substituted() {
        let chooser = parse_ui_nodes(concat!(
            r#"<node text="Reel" resource-id="" class="android.widget.TextView" content-desc="" clickable="true" enabled="true" bounds="[100,600][400,700]"/>"#,
            r#"<node text="Story" resource-id="" class="android.widget.TextView" content-desc="" clickable="true" enabled="true" bounds="[100,840][400,940]"/>"#,
        ));
        assert!(format_option(&chooser, VideoFormat::Post).is_none());
        assert!(format_option(&chooser, VideoFormat::Reel).is_some());
    }

    #[test]
    fn security_screens_stop_the_run() {
        for blocking in [
            "Log in",
            "Enter your password",
            "Security check",
            "Confirm your identity",
            "We detected unusual activity",
            "Action blocked",
            "Enter the code we sent you",
        ] {
            assert!(
                Guard::inspect(&nodes(blocking)).is_some(),
                "{blocking:?} must stop the automation"
            );
        }
    }

    #[test]
    fn an_ordinary_composer_is_not_blocked() {
        for ordinary in ["Post", "Share", "Next", "Write a caption...", "Add a title"] {
            assert!(
                Guard::inspect(&nodes(ordinary)).is_none(),
                "{ordinary:?} is a normal composer control and must not be blocked"
            );
        }
    }

    /// Facebook's recipe must not type a caption: the share intent already
    /// delivered it, and typing would duplicate it.
    /// The bug this exists for: YouTube opened on its home screen, a blanket
    /// `EditText` matcher found the SEARCH BOX, and the caption was typed into
    /// it.
    /// Captured from a real Facebook Lite composer: every node is a bare
    /// `ViewGroup` with no text and no content-desc, so no matcher can ever
    /// find its Post button.
    #[test]
    fn an_unlabelled_app_is_recognised_as_unautomatable() {
        let lite = concat!(
            r#"<node text="" resource-id="" class="androidx.recyclerview.widget.RecyclerView" content-desc="" clickable="false" enabled="true" bounds="[0,66][1080,2148]"/>"#,
            r#"<node text="" resource-id="" class="android.view.ViewGroup" content-desc="" clickable="true" enabled="true" bounds="[624,66][1056,198]"/>"#,
            r#"<node text="" resource-id="" class="android.view.View" content-desc="" clickable="true" enabled="true" bounds="[44,1409][1036,1521]"/>"#,
        );
        assert!(!has_labels(&parse_ui_nodes(lite)));

        let normal = r#"<node text="Post" resource-id="" class="android.widget.Button" content-desc="" clickable="true" enabled="true" bounds="[0,0][10,10]"/>"#;
        assert!(has_labels(&parse_ui_nodes(normal)));
    }

    #[test]
    fn visible_labels_summarise_a_screen() {
        let xml = concat!(
            r#"<node text="Home" resource-id="" class="x" content-desc="" clickable="true" enabled="true" bounds="[0,0][10,10]"/>"#,
            r#"<node text="" resource-id="" class="x" content-desc="Search" clickable="true" enabled="true" bounds="[0,0][10,10]"/>"#,
            r#"<node text="Home" resource-id="" class="x" content-desc="" clickable="true" enabled="true" bounds="[0,0][10,10]"/>"#,
            r#"<node text="" resource-id="" class="x" content-desc="" clickable="false" enabled="true" bounds="[0,0][10,10]"/>"#,
        );
        let out = visible_labels(&parse_ui_nodes(xml));
        assert!(out.contains("Home") && out.contains("Search"));
        assert_eq!(out.matches("Home").count(), 1, "duplicates add nothing");
    }

    #[test]
    fn a_search_box_is_never_accepted_as_a_caption_field() {
        let search = parse_ui_nodes(
            r#"<node text="" resource-id="com.google.android.youtube:id/search_edit_text" class="android.widget.EditText" content-desc="Search YouTube" clickable="true" enabled="true" bounds="[0,0][100,50]"/>"#,
        );
        assert!(!is_safe_caption_field(&search[0]));

        let title = parse_ui_nodes(
            r#"<node text="Add a title" resource-id="com.google.android.youtube:id/title_edit" class="android.widget.EditText" content-desc="" clickable="true" enabled="true" bounds="[0,0][100,50]"/>"#,
        );
        assert!(is_safe_caption_field(&title[0]));
    }

    /// No recipe may match a bare text field. That is what let the caption
    /// reach a search box, and the screen check alone is not enough insurance.
    #[test]
    fn no_recipe_types_into_an_unqualified_text_field() {
        for p in Platform::ALL {
            for step in recipe(*p) {
                if let Step::Caption { into, .. } = step {
                    for m in &into {
                        assert!(
                            !matches!(m, Match::Class(_)),
                            "{p:?} would type into any text field on screen"
                        );
                    }
                }
            }
        }
    }

    /// Anything that types must first prove which screen it is on.
    #[test]
    fn every_recipe_that_types_checks_the_screen_first() {
        for p in Platform::ALL {
            let steps = recipe(*p);
            let types_at = steps.iter().position(|s| matches!(s, Step::Caption { .. }));
            let Some(types_at) = types_at else { continue };
            let checked_before = steps[..types_at]
                .iter()
                .any(|s| matches!(s, Step::Expect { .. }));
            assert!(checked_before, "{p:?} types before confirming the screen");
        }
    }

    #[test]
    fn a_prefilled_caption_is_not_typed_twice() {
        let filled = parse_ui_nodes(
            r#"<node text="Check out my new vid…" resource-id="" class="android.widget.EditText" content-desc="" clickable="true" enabled="true" bounds="[0,0][10,10]"/>"#,
        );
        assert!(
            field_already_has(&filled[0], "Check out my new video!"),
            "an ellipsized value is still the same caption"
        );

        let empty = parse_ui_nodes(
            r#"<node text="Say something about this photo…" resource-id="" class="android.widget.EditText" content-desc="" clickable="true" enabled="true" bounds="[0,0][10,10]"/>"#,
        );
        assert!(!field_already_has(&empty[0], "Check out my new video!"));
        // No caption to add is the same as already done.
        assert!(field_already_has(&empty[0], "   "));
    }
}
