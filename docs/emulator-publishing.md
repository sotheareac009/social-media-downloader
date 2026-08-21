# Emulator publishing (LDPlayer + ADB)

Publish one video to many social accounts by driving the Android apps that are
**already signed in** inside your LDPlayer instances.

The app never asks for a Facebook, Instagram, TikTok or YouTube password. It
copies a file onto an emulator over ADB, makes Android's gallery see it, and
opens the app on its own composer. The session stays inside the Android app,
where you put it.

---

## 1. Architecture

```text
React / TypeScript UI
    src/pages/publisher/*, src/lib/ldplayer.ts, src/lib/publish.ts
        │  invoke() + Tauri events
        ▼
Tauri commands
    src-tauri/src/commands/ldplayer.rs
    src-tauri/src/commands/publish.rs
        ▼
Publish queue                     ← orchestration, bounded workers, SQLite
    src-tauri/src/publish/queue.rs
        │                    ╲
        │                     ╲  the ONE platform-aware step
        ▼                      ▼
LDPlayer manager           Platform connectors
    src-tauri/src/             src-tauri/src/publish/connector/*
    ldplayer/manager.rs
        ▼            ╲
    ldconsole.exe     ADB
    (instances)       (generic Android)
        ▼              ▼
      LDPlayer instances
        ▼
      Android social apps
```

### The layering rule

`ldplayer/*` knows about emulators and Android. It must never name a social
platform. `publish/connector/*` is the only place that may.

This is enforced, not merely documented — `ldplayer/manager.rs` carries a test
that greps its own source (comments stripped) for `facebook`, `instagram`,
`tiktok`, `youtube` and fails the build if one appears in code. When you add
the fifth platform, that guarantee is what makes it a new file instead of a
rewrite.

---

## 2. Folder structure

```text
src-tauri/src/
├── ldplayer/                   generic device layer — no platform knowledge
│   ├── mod.rs                  layering + security note
│   ├── adb.rs                  ADB client: devices, push, shell, MediaStore,
│   │                           share intents, screencap
│   ├── console.rs              ldconsole.exe: list2, launch, quit, serial
│   ├── manager.rs              the service: detect, list, start/stop, connect,
│   │                           transfer, scan, screenshot, log, events
│   └── settings.rs             device-settings.json (paths, dirs, limits)
│
├── publish/                    accounts, jobs, connectors
│   ├── mod.rs                  what this feature is and is not
│   ├── model.rs                Platform, Account, MediaItem, PublishJob
│   ├── store.rs                SQLite (publisher.sqlite3)
│   ├── queue.rs                the job system + worker pool
│   └── connector/
│       ├── mod.rs              PlatformConnector trait + the safety contract
│       └── share.rs            ShareConnector — the first, generic connector
│
├── commands/
│   ├── ldplayer.rs             15 device commands
│   └── publish.rs              13 account/queue commands
│
└── examples/
    └── emulator-check.rs       CLI diagnostic (see §9)

src/
├── lib/
│   ├── ldplayer.ts             typed bridge + device events
│   └── publish.ts              typed bridge + publish events
├── components/publish/
│   ├── PublishProvider.tsx     one shared fetch of devices/accounts/jobs
│   ├── PlatformMark.tsx
│   └── StatusDot.tsx
├── pages/publisher/
│   ├── DashboardPage.tsx       counts, emulators, accounts, recent activity
│   ├── AccountsPage.tsx        emulators + accounts on them
│   ├── PublishPage.tsx         video → caption → accounts → Publish
│   ├── JobList.tsx             the queue, rendered identically everywhere
│   └── SettingsPage.tsx        paths, limits, logging, log pane
└── styles/publish.css
```

---

## 3. Dependencies

**No new crates and no new npm packages were needed.** The feature is built
from what the project already depends on:

| Need | Already present |
|---|---|
| Spawning `adb` / `ldconsole` | `tokio` (`process` feature) |
| Job persistence | `rusqlite` (bundled) |
| Ids | `uuid` |
| Wire types | `serde`, `serde_json` |
| Connector trait | `async-trait` |
| File pickers | `tauri-plugin-dialog` |
| Screenshot rendering | Tauri asset protocol (already enabled) |

External tools, supplied by the user's machine rather than bundled:

- **LDPlayer** (Windows) — provides `ldconsole.exe` *and* a matching `adb.exe`.
- **ADB** — LDPlayer's bundled copy is preferred; a system `adb` on PATH works.

> Mixing ADB versions is not cosmetic. A newer `adb` on PATH kills the server
> LDPlayer started, and running instances go `offline` until restarted. That is
> why `Adb::discover` prefers LDPlayer's own binary over the one on PATH.

---

## 4. Tauri configuration

Nothing in `tauri.conf.json` had to change. The two things this feature needs
were already configured:

```jsonc
"security": {
  "assetProtocol": {
    "enable": true,
    "scope": ["$HOME/**"]     // lets the UI render emulator screenshots
  }
}
```

Screenshots are written to `<app-data-dir>/screenshots/`, which sits under
`$HOME` on both macOS and Windows.

The dialog plugin is already registered in `lib.rs`; the file pickers are
invoked from Rust, so no extra capability entry is required.

---

## 5. Database schema

`<app-data-dir>/publisher.sqlite3` — deliberately **separate** from
`accounts.sqlite3`, which belongs to the OAuth layer and enforces one row per
provider.

```sql
CREATE TABLE publish_accounts (
    id                   TEXT PRIMARY KEY,
    name                 TEXT NOT NULL,
    platform             TEXT NOT NULL,   -- facebook | instagram | tiktok | youtube
    ldplayer_instance_id TEXT NOT NULL,   -- 'ld:0' or 'adb:emulator-5554'
    package_name         TEXT NOT NULL,   -- may be a Lite/regional variant
    created_at           INTEGER NOT NULL,
    UNIQUE (ldplayer_instance_id, package_name)
);

CREATE TABLE publish_media (
    id               TEXT PRIMARY KEY,
    path             TEXT NOT NULL,       -- reference; the file is never copied
    file_name        TEXT NOT NULL,
    size_bytes       INTEGER NOT NULL,
    duration_seconds REAL,
    added_at         INTEGER NOT NULL
);

CREATE TABLE publish_jobs (
    id              TEXT PRIMARY KEY,
    media_id        TEXT NOT NULL REFERENCES publish_media(id)    ON DELETE CASCADE,
    account_id      TEXT NOT NULL REFERENCES publish_accounts(id) ON DELETE CASCADE,
    caption         TEXT NOT NULL,
    status          TEXT NOT NULL,
    progress        REAL NOT NULL DEFAULT 0,
    step            TEXT,
    error_code      TEXT,
    error           TEXT,
    screenshot_path TEXT,
    created_at      INTEGER NOT NULL,
    started_at      INTEGER,
    completed_at    INTEGER
);

-- Album posts: one job, several assets, in a defined order.
CREATE TABLE publish_job_media (
    job_id   TEXT NOT NULL REFERENCES publish_jobs(id)  ON DELETE CASCADE,
    media_id TEXT NOT NULL REFERENCES publish_media(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    PRIMARY KEY (job_id, position)
);

CREATE INDEX idx_jobs_status  ON publish_jobs(status);
CREATE INDEX idx_jobs_created ON publish_jobs(created_at DESC);
```

Schema changes go through `PublishStore::migrate`, which adds missing columns
one at a time behind a `PRAGMA table_info` check. New columns must be nullable
or carry a DEFAULT. This runs on every startup and is safe to re-run — it is
what lets you ship an update without breaking an existing user's job history.

**There is no column that can hold a credential**, and a guard test fails the
build if one is added whose name contains `token`, `secret`, `cookie`,
`password`, `credential` or `session`.

### Job statuses

```text
pending → uploading → publishing → published
                                 ↘ needs_attention
                                 ↘ failed
                     ↘ cancelled
```

`needs_attention` is not a failure. It means the app is open with the video
attached and it is your turn — which is also the correct answer when a platform
asks for a login confirmation or a security check. Painting that red would
train people to ignore the failures that matter.

On startup, any job left `pending`/`uploading`/`publishing` by a crash is failed
with code `interrupted` and the message "its outcome is unknown", because that
is the truth.

---

## 6. IPC / command design

### Device commands (`commands/ldplayer.rs`)

| Command | Purpose |
|---|---|
| `ldplayer_environment` | Is ADB/LDPlayer present, versions, paths |
| `ldplayer_redetect` | Re-run detection after installing LDPlayer |
| `ldplayer_get_settings` / `ldplayer_set_settings` | Persisted preferences |
| `ldplayer_list_devices` | Instances + any other ADB device |
| `ldplayer_start` / `ldplayer_stop` | Boot / shut down an instance |
| `ldplayer_connect` | Boot if needed **and wait for Android** |
| `ldplayer_connect_endpoint` | Attach by address (`5555`, `127.0.0.1:5555`) — escape hatch |
| `ldplayer_packages` | Installed packages on one device |
| `ldplayer_transfer_media` | Push + MediaStore index (testable alone) |
| `ldplayer_launch_app` / `ldplayer_stop_app` | Open / force-stop an app |
| `ldplayer_screenshot` | PNG to disk, returns path |
| `ldplayer_pick_media` / `ldplayer_browse_path` | Native pickers (video **and** photo) |

### Publishing commands (`commands/publish.rs`)

| Command | Purpose |
|---|---|
| `publish_platforms` | Supported platforms + package names |
| `publish_accounts` | Accounts joined with **live** device status |
| `publish_discover_accounts` | Social apps found installed on a device |
| `publish_add_account` / `publish_rename_account` / `publish_remove_account` | |
| `publish_submit` | Queue N assets to M accounts, as `album` or `single` |
| `publish_jobs` / `publish_summary` | Read the queue |
| `publish_retry` / `publish_cancel` / `publish_remove_job` / `publish_clear_finished` | |

### Events

```text
ldplayer://devices   full device list refreshed
ldplayer://device    one device changed state
ldplayer://log       a log line (verbose mode; errors always)

publish://created    job queued
publish://updated    status / progress / step changed
publish://finished   job reached a terminal state
```

Commands return the current view synchronously; events keep it fresh. That is
the same contract the download feature already uses, so the frontend pattern is
not new.

---

## 7. How a publish actually runs

`PublishQueue::run_inner`, in order:

1. **Wake the device.** `ensure_online` launches the instance if it is stopped
   and polls until `sys.boot_completed=1` *and* `init.svc.bootanim=stopped`.
   (Boot-completed alone flips well before the launcher is usable; starting an
   app during the boot animation reliably lands on a blank screen.)
2. **Check the app is installed** — *before* spending a minute copying a file
   for an app that isn't there.
3. **Copy and index** (`transfer_media`, §8).
4. **Hand off to the connector** — the only platform-aware step.
5. **Screenshot** the result (always on failure, every step in verbose mode).
6. **Optionally clean up** the pushed file — only after a confirmed publish,
   never after a hand-off, because you have not tapped Post yet.

Cancellation is checked *between* stages, never inside one: killing an
`adb push` halfway leaves a truncated file on the device.

Concurrency defaults to **2**. Emulators share one CPU and one disk; beyond
two or three, every job gets slower and devices start dropping offline.

---

## 8. Example: transferring a video from PC to LDPlayer

The complete path, from `ldplayer/manager.rs`:

```rust
pub async fn transfer_media(
    &self,
    app: Option<&AppHandle>,
    device_id: &str,
    local: &Path,
) -> Result<String> {
    let serial = self.serial_for_device(device_id).await?;   // "ld:0" → real serial
    let adb = self.adb()?;
    let settings = self.settings();

    let file_name = sanitize_file_name(local);               // sdcard is FAT-derived
    let remote = settings.remote_path_for(&file_name);       // /sdcard/Movies/…/clip.mp4

    // 1. push (creates the parent dir first; adb's own error is useless)
    adb.push(&serial, local, &remote).await?;

    // 2. verify the whole file arrived — a truncated push otherwise surfaces
    //    ten steps later as "the app can't read the video"
    let local_size = std::fs::metadata(local).map(|m| m.len()).unwrap_or(0);
    if let Some(remote_size) = adb.remote_size(&serial, &remote).await {
        if local_size > 0 && remote_size != local_size {
            return Err(AppError::MediaTransferFailed(
                format!("only {remote_size} of {local_size} bytes arrived")));
        }
    }

    // 3. make Android's gallery see it
    adb.scan_media(&serial, &remote).await.ok();

    // 4. verify it actually got indexed — a scan that silently no-ops is the
    //    single most common cause of an empty gallery picker
    if !adb.is_in_media_store(&serial, &remote).await {
        tokio::time::sleep(Duration::from_millis(1200)).await;   // one retry
        adb.scan_media(&serial, &remote).await.ok();
        if !adb.is_in_media_store(&serial, &remote).await {
            return Err(AppError::MediaScanFailed);
        }
    }

    Ok(remote)
}
```

The raw commands underneath:

```bash
adb -s 127.0.0.1:5555 shell mkdir -p '/sdcard/Movies/SocialPublisher'
adb -s 127.0.0.1:5555 push "C:\videos\my-video.mp4" /sdcard/Movies/SocialPublisher/my-video.mp4
adb -s 127.0.0.1:5555 shell stat -c %s '/sdcard/Movies/SocialPublisher/my-video.mp4'

# index it — both forms, because neither covers every Android LDPlayer ships
adb -s ... shell am broadcast -a android.intent.action.MEDIA_SCANNER_SCAN_FILE \
    -d file:///sdcard/Movies/SocialPublisher/my-video.mp4
adb -s ... shell content call --uri content://media/external/file \
    --method scan_file --arg '/sdcard/Movies/SocialPublisher/my-video.mp4'

# confirm MediaStore really has it
adb -s ... shell content query --uri content://media/external/video/media \
    --projection _id --where "_data='/sdcard/Movies/SocialPublisher/my-video.mp4'"
```

### Album posts, and what Android will not let us do

Selecting several files offers two modes:

- **`single`** — each asset becomes its own post. *N* files × *M* accounts = *N×M*
  jobs, each fully automated exactly like a one-file publish.
- **`album`** — one job per account carrying every asset in order. *M* jobs.

An album **cannot be pre-attached**, and this is a hard Android limit rather
than a shortcut. `ACTION_SEND_MULTIPLE` needs `EXTRA_STREAM` as a
`ParcelableArrayList<Uri>`, and `am start` has no flag that builds one: `--eu`
takes a single URI, and `--esa` passes Strings, which every receiving app
rejects with a `ClassCastException`. So an album job stages all files, indexes
them, opens the app, and tells the person which order to tick them in — rather
than firing an intent we know will fail.

Order is therefore real data, stored as `publish_job_media.position` and shown
in the hand-off message, because the gallery will not preserve it — selection
order in the app's own picker is what decides the carousel.

`publish_jobs.media_id` stays the first asset even though the join table also
holds it. That redundancy is deliberate: the column is NOT NULL, and rows
written before album posts existed have no join rows at all, so keeping it
authoritative for "the first asset" means old jobs still render.

### Videos and photos are filed separately

Android keeps them in **different MediaStore tables**
(`content://media/external/video/media` vs `.../images/media`), and every app's
picker filters by one or the other. A photo indexed as video is invisible to a
picker asking for photos, so `MediaCollection` (in [adb.rs]) decides three
things together: which device folder to push to (`Movies` vs `Pictures`), which
table to verify the index in, and the share MIME (`video/*` vs `image/*`).

Unknown extensions are treated as video — this feature is video-first, and the
wrong guess degrades to "the picker doesn't show it" rather than a failed
transfer.

### Why `/sdcard/Movies` and not `/sdcard/Download`

MediaStore indexes `Movies` as **video**, and every social app's picker reads
video from MediaStore. Files dropped in `Download` are frequently invisible to
those pickers — the most confusing failure in the whole feature ("the video is
right there, but the app can't see it").

### Why a `content://` URI, not `file://`

Handing `file://` to another app throws `FileUriExposedException` on Android 7+.
The connector converts the MediaStore `_id` into
`content://media/external/video/media/<id>` and fires:

```bash
adb -s ... shell am start -a android.intent.action.SEND -t 'video/*' \
    -p 'com.facebook.katana' --grant-read-uri-permission \
    --eu android.intent.extra.STREAM 'content://media/external/video/media/42' \
    --es android.intent.extra.TEXT 'Check out my new video!'
```

`-p <package>` (rather than `-n pkg/Activity`) restricts the intent to one app
and lets Android resolve the receiver — activity names change with every app
release, package names do not.

---

## 9. Setup, step by step

### Prerequisites (Windows)

1. Install **LDPlayer 9** (default path `C:\LDPlayer\LDPlayer9`).
2. Create your instances in the Multi-Instance Manager.
3. In each instance, install the social app and **sign in by hand, inside the
   emulator.** This app never does that for you.
4. In LDPlayer → Settings → Other, enable **ADB debugging** ("Open local
   connection" / *开启本地连接*).

### First run

1. Launch the app → **Publishing → Dashboard**.
2. If it says ADB isn't available, go to **Settings** and either point
   *LDPlayer folder* at your install directory, or *ADB executable* at an
   `adb.exe`, then press **Re-detect**.
3. **Emulator accounts** → your instances are listed automatically, running or
   not. Press **Start** on one, wait for *Connected*, then **Find apps**.
   (If an instance is missing because LDPlayer wasn't detected, use *Add a
   device by address* — instance 0 is port 5555, instance 1 is 5557, and so on.)
   Then Recognised social apps appear; press
   **Add** on each. Rename them by clicking the name.
4. **Publish** → choose one or more videos/photos, write a caption, pick
   **One album post** or **Separate posts** (shown only when you pick more than
   one file), tick accounts, **Publish**.
5. Watch the queue. Each job ends either *Published* or *Needs you* — the
   latter means the app is open on that instance with your video attached.

### Verifying without the GUI

```bash
cd src-tauri
cargo run --example emulator-check
cargo run --example emulator-check -- push ld:0 "C:\videos\my-video.mp4"
```

This prints ADB/LDPlayer paths, every instance and its state, and which
supported social apps are installed on each. The `push` form exercises the
whole PC → emulator → gallery path. Run it first whenever publishing "doesn't
work" on a new machine.

---

## 10. Windows development

```powershell
# one-time
winget install Rustlang.Rust.MSVC          # or rustup-init.exe
winget install Microsoft.VisualStudio.2022.BuildTools   # MSVC + Windows SDK
node --version                             # 18+

npm install
npm run tauri dev                          # dev build with hot reload
npm run build:windows                      # release .exe / installer
```

Notes specific to this feature on Windows:

- Every child process is spawned through `crate::process::command`, which sets
  `CREATE_NO_WINDOW`. Without it, each `adb` call flashes a console window —
  and a 12-account publish would flash dozens.
- `ldconsole` writes in the system code page, not UTF-8. Output is decoded
  lossily on purpose: a mangled character in an instance name is survivable,
  refusing to list instances is not.
- Instance → ADB serial is resolved by **asking LDPlayer**
  (`ldconsole adb --index N --command get-serialno`). The folklore formula
  `5555 + index*2` is used only as a fallback, and only after an explicit
  `adb connect` proves something is there. Getting this wrong doesn't fail
  loudly — it publishes to someone else's account.
- Registry lookup uses `reg.exe` rather than a registry crate: one read, one
  platform, present on every Windows.

### Developing on macOS / Linux

LDPlayer is Windows-only, and the app says so rather than reporting "not
installed". Everything else still works against any device `adb` can see
(Android Studio emulator, a phone on USB), which is enough to develop and test
the entire transfer, indexing and share-intent path.

---

## 11. Error handling and logging

### Errors

Every failure is a typed `AppError` variant with a stable machine code the UI
can branch on, and a message written for a person:

| Code | Message |
|---|---|
| `adb_missing` | ADB was not found. Install LDPlayer, or set the ADB path in Settings. |
| `adb_failed` | ADB failed: … (sanitised, first line, ≤300 chars) |
| `ldplayer_missing` | LDPlayer was not found on this computer… |
| `instance_offline` | The emulator `…` is not responding; start it in LDPlayer… |
| `not_an_ldplayer_instance` | `…` is not an LDPlayer instance, so this app can't start or stop it |
| `media_file_missing` | Could not find the video file `…` |
| `media_transfer_failed` | Copying the video to the emulator failed: … |
| `media_scan_failed` | The video was copied but Android's gallery did not index it… |
| `app_not_installed` | `…` is not installed on that emulator |
| `app_launch_failed` | Could not open `…` on the emulator |
| `job_not_retryable` | A job that is `published` can't be retried (double-posting is unrecoverable) |

ADB's output is sanitised before it reaches a job's error field: `adb:` and
`error:` prefixes stripped, first line only, truncated. ADB's habit of exiting
`0` while printing a failure is handled explicitly (`Output::failed_despite_zero`).

### Logging

- `LdPlayerManager::log` is the single entry point. Errors are **always**
  logged and emitted; everything else only when *Verbose logging* is on.
- Verbose mode also captures a screenshot at each publishing step. Screenshots
  are pruned to the newest 300 — otherwise the folder quietly reaches gigabytes.
- The Settings page has a live log pane (bounded to 400 lines) fed by
  `ldplayer://log`.
- Every job keeps its latest screenshot path, shown inline in the queue. It is
  the one artefact that answers "what is it stuck on?" without alt-tabbing to
  LDPlayer.

---

## 12. Security and safety

The app relies entirely on sessions you established yourself, inside the
emulator. It does **not**:

- collect social-media passwords — there is no field for one, anywhere;
- read cookies, tokens or session files off the device;
- automate a login screen, a two-factor prompt, or a captcha;
- work around a rate limit, a checkpoint or a security interstitial.

When an app asks for any of those, the connector returns `NeedsUser`, the job
stops as `needs_attention`, and you finish it in the emulator the same way you
would on your phone.

Three of these rules are enforced by tests rather than left as intentions:

- no schema column can hold a credential (`publish/store.rs`);
- no platform name may appear in the device layer (`ldplayer/manager.rs`);
- every platform maps to exactly one connector, and no package is claimed by
  two platforms (`publish/model.rs`, `publish/connector/mod.rs`).

---

## 13. Adding the next platform

The `ShareConnector` currently serves all four platforms. To give one a
purpose-built workflow:

1. Add `src-tauri/src/publish/connector/instagram.rs` implementing
   `PlatformConnector`.
2. Change the one arm in `connector::for_platform`.

That is the whole change. The queue, the device layer, the database and the UI
stay exactly as they are — which is the property the layering rule exists to
protect.

To add a **new** platform: a variant in `Platform`, its package names, a
connector, and the match arm. Four small edits, none of them in `ldplayer/`.
