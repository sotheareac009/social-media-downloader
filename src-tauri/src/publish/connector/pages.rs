//! Reading the Pages a Facebook account can post as, out of the app itself.
//!
//! WHY IT IS DONE THIS WAY. There is a much easier route to a list of Pages —
//! the Graph API — and it is closed to this app on purpose: it needs an access
//! token, and nothing here handles tokens, cookies, passwords or session
//! files. The session lives inside the Android app and stays there. So the
//! Pages are read the same way a person would read them: by opening the
//! profile switcher and looking.
//!
//! THE SCREEN THIS IS BUILT FROM. Captured from a real instance, not guessed:
//!
//! ```text
//!   Menu (hamburger)  →  "Open profile switcher"  →  a bottom sheet:
//!
//!     ┌ Sambath Sotheareach ────────────────┐  ← the profile, no chip
//!     ├ Dodomes            [Page]           ┤  ← content-desc carries the name
//!     ├ Mad Charcoal II    [Page]           ┤
//!     ├ Eat Me             [Page]           ┤
//!     └ Create Facebook Page ───────────────┘  ← an action, not a Page
//! ```
//!
//! Each row is a clickable node whose `content-desc` is the Page's name, and a
//! Page row — unlike the profile or the create button — contains a child whose
//! text is exactly "Page". That chip is what makes the distinction readable
//! rather than positional, which matters because the order and the number of
//! rows are both the user's, not ours.

use std::time::Duration;

use crate::errors::{AppError, Result};
use crate::ldplayer::adb::{Adb, Match, UiNode};

/// How long to wait for each screen on the way to the switcher.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// How many times to scroll the sheet looking for more Pages.
///
/// Bounded because "scroll until nothing new" against a list that re-renders
/// as it scrolls could otherwise run forever. Ten screens of Pages is far
/// past what anyone administers from one profile.
const MAX_SCROLLS: usize = 10;

/// What one read of the profile switcher found.
#[derive(Debug, Clone, Default)]
pub struct Discovery {
    /// The profile's own display name, read from the row with no Page chip.
    /// This is what the app must be switched back to after posting as a Page.
    pub profile: Option<String>,
    pub pages: Vec<String>,
}

/// Open Facebook's profile switcher, leaving the sheet on screen.
///
/// Shared by reading the Pages and by switching to one, because they are the
/// same three taps and a second copy would drift.
///
/// THE TRAP THIS AVOIDS: "Menu" is not unique. The composer has a control with
/// exactly that content-desc, and tapping it opens a "More options" sheet that
/// has no switcher in it — which is how a Page list came back empty on an
/// account with four Pages. So the composer is detected first and refused by
/// name, rather than being navigated blindly.
async fn open_switcher(adb: &Adb, serial: &str) -> Result<()> {
    if composer_is_open(adb, serial).await {
        return Err(AppError::Internal(
            "Facebook has an unfinished post open on this instance, and its Pages \
             can't be read from there. Open LDPlayer, finish or discard that post, \
             then try again."
                .into(),
        ));
    }

    // 1. The hamburger. Its content-desc is a bare "Menu".
    let menu = adb
        .wait_for_node(serial, &[Match::Desc("Menu".into())], STEP_TIMEOUT)
        .await?
        .ok_or_else(|| {
            AppError::Internal(
                "couldn't find Facebook's menu button, so the Page list could not be \
                 opened. Open Facebook on this instance and check it is on its normal \
                 screen."
                    .into(),
            )
        })?;
    adb.tap_node(serial, &menu).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // 2. The switcher entry. Matched on a prefix: the label grows a ", you
    //    have notifications" tail whenever any Page has one.
    let switcher = adb
        .wait_for_node(
            serial,
            &[Match::DescContains("Open profile switcher".into())],
            STEP_TIMEOUT,
        )
        .await?
        .ok_or_else(|| {
            AppError::Internal(
                "Facebook's menu opened but there was no profile switcher on it. Either \
                 this account has no Pages, or the app was not on its own menu — check \
                 the instance in LDPlayer."
                    .into(),
            )
        })?;
    adb.tap_node(serial, &switcher).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    Ok(())
}

/// Switch the app to post as `name` — a Page, or the profile itself.
///
/// Facebook publishes as whoever is active, and no share intent can say
/// otherwise, so this is the only way a post reaches a Page. It does NOT
/// confirm the switch worked: that is checked where it matters, on the
/// composer, immediately before anything is submitted.
pub async fn switch_to(adb: &Adb, serial: &str, name: &str) -> Result<()> {
    open_switcher(adb, serial).await?;

    let wanted = name.trim();
    for pass in 0..=MAX_SCROLLS {
        let nodes = adb.ui_dump(serial).await?;

        // Match on the row's own label with its notification count stripped,
        // because "Dodomes" and "Dodomes, 1 notification" are the same Page
        // and which one the app renders is not ours to decide.
        let row = nodes.iter().find(|n| {
            n.clickable
                && !n.content_desc.trim().is_empty()
                && strip_notification_count(n.content_desc.trim()).eq_ignore_ascii_case(wanted)
        });

        if let Some(row) = row {
            adb.tap_node(serial, row).await?;
            // Switching reloads the app as the other identity, which is not
            // quick. Everything after this waits on its own anchor anyway.
            tokio::time::sleep(Duration::from_secs(5)).await;
            return Ok(());
        }

        if pass == MAX_SCROLLS {
            break;
        }
        let (w, h) = screen_size(&nodes);
        adb.swipe(serial, (w / 2, (h * 4) / 5), (w / 2, h / 3), 600).await?;
        tokio::time::sleep(Duration::from_millis(900)).await;
    }

    // Leave the sheet rather than stranding the app on it.
    adb.press_back(serial).await.ok();
    Err(AppError::Internal(format!(
        "`{wanted}` is not in Facebook's profile switcher on this instance. Run Find \
         Pages again — a Page that has been renamed or handed over is no longer \
         somewhere this account can post."
    )))
}

/// The profile's own name: the switcher row with no Page chip.
///
/// "Dismiss" and "Create Facebook Page" are controls on the same sheet, so the
/// row is taken as the first one that is neither, rather than by position —
/// position is exactly the sort of thing an app redesign moves.
pub fn parse_profile(nodes: &[UiNode]) -> Option<String> {
    let chips: Vec<&UiNode> = nodes.iter().filter(|n| n.text.trim() == "Page").collect();

    nodes
        .iter()
        .filter(|n| n.clickable && !n.content_desc.trim().is_empty())
        .filter(|row| !chips.iter().any(|chip| contains(row, chip)))
        .map(|row| strip_notification_count(row.content_desc.trim()))
        .find(|name| {
            let lower = name.to_lowercase();
            !lower.is_empty()
                && lower != "dismiss"
                && !lower.starts_with("create ")
                && !lower.starts_with("see all")
        })
}

/// Whether Facebook's composer is the screen in front.
///
/// THE BUG THIS IS THE FIX FOR: this used to match on labels, including
/// "What's on your mind" — which is on the composer AND in the status box at
/// the top of the ordinary feed. So a clean launch to the feed reported an
/// unfinished post, and reading the Page list refused to start on an account
/// whose Facebook was sitting there perfectly idle.
///
/// The activity name has no such ambiguity: the composer is its own activity
/// (`com.facebook.composer.activity.ComposerActivity`), while the feed is not.
async fn composer_is_open(adb: &Adb, serial: &str) -> bool {
    adb.foreground_activity(serial)
        .await
        .is_some_and(|a| a.to_lowercase().contains("composer"))
}

/// Open the profile switcher and read every Page it lists.
///
/// Leaves the sheet closed and the app on the screen it started from, because
/// a discovery that quietly parks the app somewhere else would make the next
/// publish start from a screen it does not expect.
pub async fn discover(adb: &Adb, serial: &str) -> Result<Discovery> {
    open_switcher(adb, serial).await?;

    // 3. Read the sheet, scrolling until it stops offering anything new.
    let mut found: Vec<String> = Vec::new();
    for pass in 0..=MAX_SCROLLS {
        let nodes = adb.ui_dump(serial).await?;
        let before = found.len();
        for name in parse_pages(&nodes) {
            if !found.iter().any(|f| f == &name) {
                found.push(name);
            }
        }

        // Nothing new on this screen means the end of the list — one pass is
        // allowed to find nothing before giving up, because the first dump
        // can land while the sheet is still animating in.
        if pass > 0 && found.len() == before {
            break;
        }
        if pass == MAX_SCROLLS {
            break;
        }

        // Scroll within the sheet. A slow drag rather than a fling: a fling
        // lands somewhere unpredictable and skips rows.
        let (w, h) = screen_size(&nodes);
        adb.swipe(serial, (w / 2, (h * 4) / 5), (w / 2, h / 3), 600).await?;
        tokio::time::sleep(Duration::from_millis(900)).await;
    }

    // Read the profile's own name from the same sheet. It is what the app
    // must be switched BACK to after posting as a Page, and what an ordinary
    // post is checked against — learning it here saves asking the person to
    // type their own name.
    let profile = adb
        .ui_dump(serial)
        .await
        .ok()
        .and_then(|nodes| parse_profile(&nodes));

    // Put the app back where it was found.
    adb.press_back(serial).await.ok();
    tokio::time::sleep(Duration::from_millis(600)).await;
    adb.press_back(serial).await.ok();

    Ok(Discovery { profile, pages: found })
}

/// The Page names on a switcher screen.
///
/// A Page row is a clickable node carrying the name as its `content-desc`,
/// with a child chip whose text is exactly "Page". Both halves are needed:
/// the chip alone has no name, and the content-desc alone would also match the
/// profile row and the "Create Facebook Page" button.
pub fn parse_pages(nodes: &[UiNode]) -> Vec<String> {
    let chips: Vec<&UiNode> = nodes.iter().filter(|n| n.text.trim() == "Page").collect();

    let mut out = Vec::new();
    for row in nodes.iter().filter(|n| n.clickable && !n.content_desc.trim().is_empty()) {
        if !chips.iter().any(|chip| contains(row, chip)) {
            continue;
        }
        let name = strip_notification_count(row.content_desc.trim());
        if name.is_empty() || out.iter().any(|n| n == &name) {
            continue;
        }
        out.push(name);
    }
    out
}

/// Whether `outer`'s rectangle encloses `inner`'s.
///
/// Bounds rather than tree structure because the dump this reads is a flat
/// list: containment is the only parent-child relationship left in it.
fn contains(outer: &UiNode, inner: &UiNode) -> bool {
    let (ol, ot, or_, ob) = outer.bounds;
    let (il, it, ir, ib) = inner.bounds;
    ol <= il && ot <= it && or_ >= ir && ob >= ib && (or_ - ol) > 0 && (ob - ot) > 0
}

/// Turn "Dodomes, 12 notifications" back into "Dodomes".
///
/// The count is part of the accessibility label, not the name, and it changes
/// on its own — a Page stored with a count in its name would stop matching
/// itself the moment someone commented, and the identity check before every
/// post compares exactly this string.
fn strip_notification_count(desc: &str) -> String {
    let Some(idx) = desc.rfind(", ") else {
        return desc.to_string();
    };
    let tail = desc[idx + 2..].trim();
    let is_count = tail
        .strip_suffix(" notifications")
        .or_else(|| tail.strip_suffix(" notification"))
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));

    if is_count {
        desc[..idx].trim().to_string()
    } else {
        desc.to_string()
    }
}

/// Screen size, taken from the widest and tallest node in the dump.
///
/// Read rather than assumed: these instances are configured by the user and
/// this one reports a 2160-wide screen, so a hard-coded swipe would miss the
/// sheet entirely on anything else.
fn screen_size(nodes: &[UiNode]) -> (i32, i32) {
    let w = nodes.iter().map(|n| n.bounds.2).max().unwrap_or(1080).max(1);
    let h = nodes.iter().map(|n| n.bounds.3).max().unwrap_or(1920).max(1);
    (w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldplayer::adb::parse_ui_nodes;

    /// Captured from the real switcher on a live instance: two Pages, the
    /// profile above them, and the create button below. Only the Pages are
    /// Pages.
    const SWITCHER: &str = concat!(
        r#"<node text="" resource-id="" class="android.view.ViewGroup" content-desc="Dismiss" clickable="true" enabled="true" bounds="[936,1086][1224,1182]"/>"#,
        r#"<node text="" resource-id="" class="android.view.ViewGroup" content-desc="Sambath Sotheareach" clickable="true" enabled="true" bounds="[96,1278][2064,1662]"/>"#,
        r#"<node text="Sambath Sotheareach" resource-id="" class="android.view.View" content-desc="" clickable="false" enabled="true" bounds="[504,1350][921,1475]"/>"#,
        r#"<node text="" resource-id="" class="android.view.ViewGroup" content-desc="Dodomes, 1 notification" clickable="true" enabled="true" bounds="[96,1662][2064,2204]"/>"#,
        r#"<node text="Dodomes" resource-id="" class="android.view.View" content-desc="" clickable="false" enabled="true" bounds="[504,1734][921,1859]"/>"#,
        r#"<node text="1 notification" resource-id="" class="android.view.View" content-desc="" clickable="false" enabled="true" bounds="[504,1880][900,1990]"/>"#,
        r#"<node text="Page" resource-id="" class="android.view.View" content-desc="" clickable="false" enabled="true" bounds="[504,2012][768,2132]"/>"#,
        r#"<node text="" resource-id="" class="android.view.ViewGroup" content-desc="Mad Charcoal II" clickable="true" enabled="true" bounds="[96,2204][2064,2623]"/>"#,
        r#"<node text="Mad Charcoal II" resource-id="" class="android.view.View" content-desc="" clickable="false" enabled="true" bounds="[504,2276][1178,2401]"/>"#,
        r#"<node text="Page" resource-id="" class="android.view.View" content-desc="" clickable="false" enabled="true" bounds="[504,2431][768,2551]"/>"#,
        r#"<node text="" resource-id="" class="android.view.ViewGroup" content-desc="Create Facebook Page" clickable="true" enabled="true" bounds="[96,3707][2064,3840]"/>"#,
        r#"<node text="Create Facebook Page" resource-id="" class="android.view.View" content-desc="" clickable="false" enabled="true" bounds="[504,3740][1400,3830]"/>"#,
    );

    /// The profile's own name is what the app is switched BACK to after a Page
    /// post, so reading it wrong strands the account on the Page — and the
    /// next job posts there. It is the row with no Page chip, and neither
    /// "Dismiss" nor "Create Facebook Page" is a person.
    #[test]
    fn the_profile_is_read_from_the_row_without_a_chip() {
        let nodes = parse_ui_nodes(SWITCHER);
        assert_eq!(parse_profile(&nodes).as_deref(), Some("Sambath Sotheareach"));
    }

    /// A sheet with no profile row must yield nothing rather than the first
    /// control it finds — switching to "Dismiss" is not a thing that can work.
    #[test]
    fn controls_are_never_mistaken_for_the_profile() {
        let controls = concat!(
            r#"<node text="" resource-id="" class="android.view.ViewGroup" content-desc="Dismiss" clickable="true" enabled="true" bounds="[936,1086][1224,1182]"/>"#,
            r#"<node text="" resource-id="" class="android.view.ViewGroup" content-desc="Create Facebook Page" clickable="true" enabled="true" bounds="[96,3707][2064,3840]"/>"#,
        );
        assert_eq!(parse_profile(&parse_ui_nodes(controls)), None);
    }

    #[test]
    fn the_switcher_yields_pages_and_nothing_else() {
        let pages = parse_pages(&parse_ui_nodes(SWITCHER));
        assert_eq!(pages, vec!["Dodomes", "Mad Charcoal II"]);
    }

    /// The profile is not a Page, and posting to it is the default anyway.
    /// "Create Facebook Page" is a button; adding it as a target would offer
    /// somewhere that does not exist.
    #[test]
    fn the_profile_and_the_create_button_are_not_pages() {
        let pages = parse_pages(&parse_ui_nodes(SWITCHER));
        assert!(!pages.iter().any(|p| p == "Sambath Sotheareach"));
        assert!(!pages.iter().any(|p| p.starts_with("Create")));
    }

    /// The bug this guards: the count is part of the accessibility label and
    /// changes on its own. A Page stored as "Dodomes, 1 notification" would
    /// fail its own identity check the moment someone commented.
    #[test]
    fn a_notification_count_is_not_part_of_the_name() {
        assert_eq!(strip_notification_count("Dodomes, 1 notification"), "Dodomes");
        assert_eq!(strip_notification_count("MerlFood, 12 notifications"), "MerlFood");
        assert_eq!(strip_notification_count("Mad Charcoal II"), "Mad Charcoal II");
        // A Page whose name genuinely ends that way keeps it.
        assert_eq!(
            strip_notification_count("Ten, no notification of it"),
            "Ten, no notification of it"
        );
        assert_eq!(strip_notification_count("Acme, Inc"), "Acme, Inc");
    }

    /// A chip that belongs to another row must not name this one. Bounds are
    /// the only parent-child relationship left in a flat dump.
    #[test]
    fn a_chip_outside_a_row_does_not_make_it_a_page() {
        let stray = concat!(
            r#"<node text="" resource-id="" class="android.view.ViewGroup" content-desc="Some Profile" clickable="true" enabled="true" bounds="[0,0][100,100]"/>"#,
            r#"<node text="Page" resource-id="" class="android.view.View" content-desc="" clickable="false" enabled="true" bounds="[200,200][260,240]"/>"#,
        );
        assert!(parse_pages(&parse_ui_nodes(stray)).is_empty());
    }
}
