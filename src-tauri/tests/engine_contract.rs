//! End-to-end check of the yt-dlp contract, using a stub binary.
//!
//! Unit tests cover parsing in isolation; this covers the part that only
//! breaks in reality - that we spawn a real child, that our flags are accepted
//! in the order given, that progress lines survive the pipe, and that a
//! non-zero exit becomes the right error.
//!
//! A stub rather than the real yt-dlp on purpose: the real one needs a network
//! and a live public post, so a test using it would fail for reasons that have
//! nothing to do with this code.

#![cfg(unix)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use media_downloader_lib::download::ytdlp;
use media_downloader_lib::errors::AppError;
use tokio::sync::mpsc;

/// Write an executable stub that behaves like yt-dlp for the flags we pass.
///
/// Each stub gets its own filename rather than overwriting one path: a
/// previous stub's child may still be running (step 4 deliberately leaves a
/// `sleep` behind), and rewriting a script the kernel is executing is a race
/// that surfaces as an intermittent "text file busy" or a half-read script.
fn install_stub(dir: &Path, tag: &str, body: &str) -> PathBuf {
    let path = dir.join(format!("yt-dlp-{tag}"));
    let mut f = std::fs::File::create(&path).unwrap();
    write!(f, "#!/bin/sh\n{body}\n").unwrap();
    drop(f);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var("MEDIA_DOWNLOADER_YTDLP", &path);
    path
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("md-engine-test-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// One test function, because `MEDIA_DOWNLOADER_YTDLP` is process-global and
/// parallel tests would fight over it.
#[tokio::test]
async fn engine_contract_holds_against_a_stub() {
    let dir = scratch("main");

    // ---- 1. the hardening flags are actually passed -----------------------
    // The stub records its own argv; the assertion is that a session could
    // never be handed to the engine.
    let argv_log = dir.join("argv.txt");
    install_stub(
        &dir,
        "probe",
        &format!(
            r#"printf '%s\n' "$@" > {log}
echo '{{"id":"abc123","title":"A public reel","uploader":"someone","duration":12.5,"ext":"mp4","filesize_approx":1048576}}'"#,
            log = argv_log.display()
        ),
    );

    let url = url::Url::parse("https://www.tiktok.com/@u/video/7300000000000000000").unwrap();
    let info = ytdlp::probe(&url).await.expect("probe should succeed");

    assert_eq!(info.id, "abc123");
    assert_eq!(info.title, "A public reel");
    assert_eq!(info.uploader.as_deref(), Some("someone"));
    assert_eq!(info.estimated_bytes, Some(1_048_576));

    let argv = std::fs::read_to_string(&argv_log).unwrap();
    for required in [
        "--ignore-config",
        "--no-cookies",
        "--no-cookies-from-browser",
        "--no-playlist",
    ] {
        assert!(
            argv.lines().any(|l| l == required),
            "{required} must be passed on every invocation; got:\n{argv}"
        );
    }
    assert!(
        argv.lines().any(|l| l == "--no-playlist"),
        "a single-video probe must not expand a playlist; got:\n{argv}"
    );
    // `--netrc` is opt-in, so its absence is what keeps a `.netrc` unread.
    // Asserting the absence also guards against re-adding `--no-netrc`, which
    // yt-dlp does not accept and which aborted every download when it was.
    assert!(
        !argv.lines().any(|l| l.starts_with("--netrc") || l == "--no-netrc"),
        "no netrc flag may be passed; got:\n{argv}"
    );
    assert!(
        !argv.to_lowercase().contains("token") && !argv.to_lowercase().contains("password"),
        "no credential may ever reach the engine; got:\n{argv}"
    );

    // ---- 2. progress lines cross the pipe and parse ------------------------
    install_stub(
        &dir,
        "progress",
        r#"echo "MDPROGRESS 0 4000 NA NA"
echo "[download] Destination: video.mp4"
echo "MDPROGRESS 2000 4000 500000.0 4"
echo "MDPROGRESS 4000 4000 500000.0 0"
exit 0"#,
    );

    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut running = ytdlp::start(&url, &dir, tx).expect("spawn");
    ytdlp::wait(&mut running).await.expect("clean exit");

    let mut seen = Vec::new();
    while let Ok(p) = rx.try_recv() {
        seen.push(p);
    }
    assert_eq!(seen.len(), 3, "chatter must be skipped, progress kept");
    assert_eq!(seen[0].downloaded_bytes, 0);
    assert_eq!(seen[1].fraction, Some(0.5));
    assert_eq!(seen[2].fraction, Some(1.0));
    assert_eq!(seen[2].eta_seconds, Some(0));

    // ---- 3. a login wall becomes MediaNotPublic, not a generic failure -----
    install_stub(
        &dir,
        "loginwall",
        r#"echo "ERROR: [facebook] 123: Login required to view this video" >&2
exit 1"#,
    );
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut running = ytdlp::start(&url, &dir, tx).unwrap();
    match ytdlp::wait(&mut running).await {
        Err(AppError::MediaNotPublic) => {}
        other => panic!("expected MediaNotPublic, got {other:?}"),
    }

    // ---- 4. cancellation actually kills the child --------------------------
    install_stub(&dir, "hang", r#"sleep 30"#);
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut running = ytdlp::start(&url, &dir, tx).unwrap();
    running.kill().await;
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        ytdlp::wait(&mut running),
    )
    .await;
    assert!(outcome.is_ok(), "a killed child must not leave wait() hanging");

    // ---- 5. a profile listing is parsed into queueable entries ------------
    // JSON shaped exactly like a real `--flat-playlist` response: a nested
    // `entries` array whose items carry a full video URL and no thumbnail.
    install_stub(
        &dir,
        "profile",
        r#"printf '%s' '{"id":"MS4wLjABAAAA","title":"raimqqq","_type":"playlist","entries":[{"id":"7674870647071296789","url":"https://www.tiktok.com/@raimqqq/video/7674870647071296789","title":"Watch out","duration":9},{"id":"7674870647071296790","url":"https://www.tiktok.com/@raimqqq/video/7674870647071296790","title":"Second","duration":null},{"id":"nourl","title":"skipped"}]}'"#,
    );
    let profile_url = url::Url::parse("https://www.tiktok.com/@raimqqq").unwrap();
    let listing = ytdlp::list_profile(&profile_url).await.expect("listing");

    assert_eq!(listing.uploader, "raimqqq");
    // The third entry has no URL, so there is nothing to queue for it.
    assert_eq!(listing.count, 2, "entries without a URL must be skipped");
    assert_eq!(listing.entries.len(), listing.count, "count must match entries");
    assert_eq!(
        listing.entries[0].url,
        "https://www.tiktok.com/@raimqqq/video/7674870647071296789"
    );
    assert_eq!(listing.entries[0].duration_seconds, Some(9.0));
    assert_eq!(listing.entries[1].duration_seconds, None);

    // ---- 6. an empty feed is "no media", not a listing of nothing ---------
    install_stub(&dir, "emptyfeed", r#"printf '%s' '{"title":"empty","entries":[]}'"#);
    match ytdlp::list_profile(&profile_url).await {
        Err(AppError::NoMediaFound) => {}
        other => panic!("expected NoMediaFound, got {other:?}"),
    }

    // No "missing engine" case here: `locate()` deliberately falls back to
    // /opt/homebrew/bin and friends, so it cannot be starved by clearing PATH
    // on a machine that genuinely has yt-dlp installed - and clearing PATH
    // would sabotage the sibling test running in parallel.

    let _ = std::fs::remove_dir_all(&dir);
}

/// Every flag we pass must actually exist in the installed yt-dlp.
///
/// This exists because it already went wrong: `--no-netrc` was passed for a
/// while and is not a yt-dlp option, so the engine exited with
/// "no such option" before fetching a single byte and every download failed.
/// A stub can't catch that - only the real binary can - so this test runs
/// against whatever is installed and skips when nothing is.
///
/// `--version` makes it a pure argument-parsing check: no network, no URL, and
/// it exits non-zero precisely when an option is unrecognised.
#[tokio::test]
async fn every_flag_we_pass_is_accepted_by_the_real_engine() {
    // Not `engine_path()`: the stub test above may have left an override set.
    let Some(engine) = which_yt_dlp() else {
        eprintln!("skipping: yt-dlp is not installed");
        return;
    };

    let out = tokio::process::Command::new(&engine)
        .args(ytdlp::HARDENED_FLAGS)
        .arg("--no-playlist")
        .arg("--yes-playlist")
        .arg("--flat-playlist")
        .arg("--newline")
        .arg("--no-warnings")
        .arg("--socket-timeout")
        .arg("20")
        .arg("--retries")
        .arg("3")
        .arg("--progress-template")
        .arg("MDPROGRESS %(progress.downloaded_bytes)s")
        .arg("-f")
        .arg("b[ext=mp4]/b[ext=mov]/b")
        .arg("--version")
        .output()
        .await
        .expect("spawn yt-dlp");

    assert!(
        out.status.success(),
        "yt-dlp rejected one of our flags:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// PATH lookup that ignores `MEDIA_DOWNLOADER_YTDLP`, plus the locations a GUI
/// app's PATH typically misses.
fn which_yt_dlp() -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(
        ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"]
            .iter()
            .map(PathBuf::from),
    );
    dirs.into_iter()
        .map(|d| d.join("yt-dlp"))
        .find(|p| p.is_file())
}
