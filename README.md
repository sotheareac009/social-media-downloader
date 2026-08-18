# Media Downloader

A Tauri 2 + Rust + React + TypeScript desktop app.

Two independent capabilities, deliberately not wired together:

| | Needs an account? | What it reaches |
|---|---|---|
| **Downloads** | No | Public Facebook and TikTok videos and reels |
| **Accounts** | Yes | Your profile name and avatar — nothing else |

**Signing in does not unlock private media, and downloading does not require
signing in.** The scopes requested at login are profile-only, and the download
engine is deliberately run with no session at all (see below). A post is
downloadable exactly when it is already public.

---

## Downloading public videos

Paste a link on the Downloads page. Supported hosts:

- `youtube.com/watch?v=…`, `youtu.be/…`, `youtube.com/shorts/…`
- `youtube.com/@channel`, `youtube.com/playlist?list=…` (whole feeds)

- `facebook.com/watch`, `facebook.com/reel/…`, `fb.watch/…`, `m.facebook.com/…`
- `tiktok.com/@user/video/…`, `vm.tiktok.com/…`, `vt.tiktok.com/…`
- `tiktok.com/@user` — a whole profile (see below)

Anything else is refused. The host allowlist lives in
`src-tauri/src/download/url.rs` and is matched exactly, not by substring, so
`tiktok.com.example.test` does not pass.

### Whole TikTok profiles

Paste `https://www.tiktok.com/@name` and the app lists the creator's feed with
`--flat-playlist`, which reads the listing without resolving each post — 133
videos enumerate in seconds rather than minutes.

The count is shown for confirmation *before* anything is queued: one pasted line
can mean a hundred files, and deciding that silently is how you fill a disk.
Press **Download all N** to queue them; two run at a time and each can be
cancelled individually.

Facebook has no equivalent. yt-dlp has no page-listing extractor for it —
`facebook.com/<page>/videos` returns "Unsupported URL" — so every Facebook link
is a single post.

### TikTok rate limiting

TikTok throttles anonymous requests, and does it *silently* — it answers with an
anti-bot page that yt-dlp reports as `Unable to extract universal data for
rehydration`. The video is fine; the request was refused. Profile listing has
the same problem, surfacing as `Unable to extract secondary user ID`.

The app handles this rather than passing it on:

- Throttling is a distinct error from "no video found", so a rate-limited post
  is never reported as missing.
- Failed jobs retry up to 4 times, backing off 5s / 15s / 30s.
- Engine starts are spaced 1.2s apart so a large queue doesn't burst.

Those numbers are measured, not guessed. Replaying a slice of a real profile:

| policy | result |
|---|---|
| no retry, no stagger | 5/8 |
| 3 attempts, 3s/9s, 700ms stagger | 7/8 |
| 4 attempts, 5s/15s/30s, 1.2s stagger | 12/12 |

### Per-link quality, read from the video

Paste a single video link and the app probes it, then shows a panel with the
title, thumbnail and **the quality tiers that video actually has** — 8K appears
only when the video really offers it. Pick one and download; the choice applies
to that job only, leaving the global default alone.

Tiers come from yt-dlp's `format_note`, **not** from pixel height. An ultrawide
8K stream is 7680x3200, so its height is 3200 — calling it "3200p" would be
both wrong and unrecognisable, while the platform labels it `4320p`. Height is
only the fallback when a format carries no note. Frame-rate variants ("1080p60"
and "1080p") collapse into one entry, and audio-only formats are excluded.

Probing is debounced and only runs for a single link — inspecting ten pasted
URLs while someone is still typing would fire ten network requests. Multi-link
pastes queue immediately at the global quality, as before.

### Choosing quality

The Downloads page has a **Quality** picker: Best available, 2160p, 1440p,
1080p, 720p, 480p or 360p. The choice is saved next to the download folder and
survives restarts.

Every selector ends in a bare `b` fallback, so a video with nothing at the
requested height still downloads at whatever it does have, rather than failing
with "requested format is not available".

Measured against a real YouTube video offering up to 1080p:

| selection | with FFmpeg | without |
|---|---|---|
| Best   | 1080p | 360p |
| ≤1080p | 1080p | 360p |
| ≤720p  | 720p  | 360p |
| ≤480p  | 480p  | 360p |

That right-hand column is the point of the next section.

### YouTube quality needs FFmpeg

Above 360p, YouTube serves video and audio as **separate streams**. Without a
merger the best single file available is 360p; measured on one video, 360p
progressive versus 1080p merged. Facebook and TikTok serve progressive files and
are unaffected.

So FFmpeg is optional but strongly recommended:

```bash
brew install ffmpeg                 # macOS
winget install Gyan.FFmpeg          # Windows
sudo apt install ffmpeg             # Linux
```

The app detects it and switches format selection automatically — with FFmpeg it
asks for `bv*+ba` and remuxes to mp4; without, it asks only for single files, so
a download can never succeed for ten minutes and then fail at a merge step. The
Downloads page says which mode you're in. Override detection with
`MEDIA_DOWNLOADER_FFMPEG=/full/path/to/ffmpeg`.

### YouTube anti-bot refusals

YouTube intermittently answers its own media URLs with `HTTP 403` for anonymous
clients. It is genuinely intermittent — the same video 403s and then downloads
minutes later.

The app handles it with a player-client fallback chain rather than giving up.
**The order is quality-critical**, measured by downloading and probing the
result:

| client | result |
|---|---|
| default | 1080p |
| `tv_embedded` | 1080p |
| `web_embedded` | 1080p |
| `android_vr` | 1080p |
| `mweb` | **360p only** |
| `web`, `ios` | no usable format |

`mweb` is tried last precisely because it serves only format 18. An earlier
version of this chain put it second, so every 403 — which is common — silently
downgraded the download to 360p no matter what quality was selected. Any client
added here must be checked for the same trap: one that "works" while quietly
capping quality is worse than one that fails outright.

A real chain walk on one video: default → 403, `tv_embedded` → 403,
`web_embedded` → 1080p. Refusals shift per video and over time, which is why
the chain has depth.

A 403 is classified separately from rate limiting, because waiting doesn't help
— only asking as a different client does — so it retries immediately instead of
backing off.

### One prerequisite: yt-dlp

Neither platform has an API that returns a media file — both serve short-lived
signed CDN URLs embedded in page state that changes without notice. yt-dlp
tracks that churn full-time, so it does the extraction:

```bash
brew install yt-dlp                 # macOS
winget install yt-dlp.yt-dlp        # Windows
pipx install yt-dlp                 # Linux
```

It is **not bundled** — a pinned copy would silently break every time a
platform changed its markup. The Downloads page shows whether it was found.

If it is installed but not detected, that is almost always PATH: a desktop app
launched from Finder or the Start menu does not inherit your shell PATH. Set an
explicit path in `.env` and restart:

```dotenv
MEDIA_DOWNLOADER_YTDLP=/opt/homebrew/bin/yt-dlp
```

### Why "public only" is enforced, not just promised

Every engine invocation passes `--ignore-config`, `--no-cookies` and
`--no-cookies-from-browser`. So the engine cannot read your browser session and
cannot load a `yt-dlp.conf` that would re-add one. A `.netrc` is never consulted
either, because yt-dlp only reads one when `--netrc` is passed and we never
pass it. `src-tauri/src/download/` has no access to the keychain
and takes no token argument — there is no code path that could pass one.

A private post therefore fails with a clear "this isn't public" message whether
or not you are signed in.

Format selection is `b[ext=mp4]/b[ext=mov]/b` — a single progressive file. This
build ships no FFmpeg, so a format needing a video+audio merge would fail at the
last step after a full download; picking a single stream avoids that.

### Where files are saved

`~/Downloads/Media Downloader` by default. Two downloads run at a time.

Users change this themselves in the app — **Change…** next to "Saving to" opens
the OS folder picker. The choice is remembered across restarts in
`downloader-settings.json` in the app data directory, and **Reset** returns to
the default.

The picker is opened from Rust, not JavaScript, so the webview is never granted
filesystem capabilities: it receives a path string and no ability to read or
write anything itself. If a saved folder later disappears — an unplugged
external drive, a renamed directory — the app falls back to the default instead
of failing every download with a path error.

---

## Running it

```bash
npm install
cp .env.example .env   # already done for you
npm run tauri dev
```

Fill in `.env` and restart. With everything blank the app still runs — providers
show **Needs setup** and Connect is disabled.

### Configuration

`.env` is discovered at startup from the first of these that exists:

1. `$MEDIA_DOWNLOADER_ENV_FILE` (explicit path)
2. `./.env` and `../.env` relative to the working directory
3. `src-tauri/.env` and the project root, in dev builds

Per key, the priority is **real environment variable → `.env` → compile-time
value**. A shell variable always wins, so a one-off override still works:

```bash
GOOGLE_CLIENT_ID="other-id" npm run tauri dev
```

For a release build, bake the values in at compile time instead of shipping a
`.env` inside the bundle:

```bash
GOOGLE_CLIENT_ID="..." npm run tauri build
```

To check what the app will actually see — which `.env` it found and which
providers are ready — without launching the UI:

```bash
cd src-tauri && cargo run --example config-check
```

It prints only "set"/"not set", never a value, so its output is safe to share.

> `.env` holds *application* configuration — the client ID/secret a platform
> issued to this app. It never holds a *user's* token. User access and refresh
> tokens go to the OS keychain and are never written to any file.

## Getting a Google client ID

Google's *Desktop app* client type is the one that officially supports the
loopback + PKCE flow this app uses.

1. <https://console.cloud.google.com/> → create or pick a project
2. **APIs & Services → OAuth consent screen** → External → add yourself as a
   test user
3. **Credentials → Create credentials → OAuth client ID → Desktop app**
4. Copy the client ID

No redirect URI needs registering: Google accepts any `http://127.0.0.1:{port}`
loopback for Desktop clients, which is why the app can bind an ephemeral port
per flow.

Set it in `.env`:

```dotenv
GOOGLE_CLIENT_ID=1234-abc.apps.googleusercontent.com
```

If your client configuration also requires the secret at the token endpoint, set
`GOOGLE_CLIENT_SECRET` too. Per RFC 8252 §8.5 that value is *not* a real secret
in an installed app — PKCE is what secures this flow.

## Instagram

"Instagram API with Instagram Login" (Business Login). Requires an Instagram
**Business or Creator** account — this API replaced the Basic Display API,
which Meta shut down in December 2024, and a personal account cannot complete
the flow.

```dotenv
INSTAGRAM_CLIENT_ID=
INSTAGRAM_CLIENT_SECRET=
INSTAGRAM_REDIRECT_URI=https://you.example/ig/callback
```

Instagram deviates from ordinary OAuth in four ways, each handled in
[`instagram.rs`](src-tauri/src/auth/providers/instagram.rs):

| | Instagram | Ordinary OAuth |
|---|---|---|
| Token response | `{"data":[{...}]}` — nested array | flat object |
| Authorization code | arrives with `#_` appended | used as-is |
| Token acquisition | **two** exchanges (1h → 60d) | one |
| Renewal | token refreshes *itself* | `refresh_token` |

The last two are the ones that bite. Skipping the long-lived exchange leaves a
credential that dies within the hour, so `handle_callback` always performs both
steps before returning. And because Instagram issues no refresh token,
`AuthProvider::can_refresh` exists — the standard `refresh_token.is_some()`
check would report every Instagram account as permanently needing re-auth.

PKCE is deliberately not sent: Meta does not document `code_challenge` support
on this endpoint, and the exchange is protected by the client secret and the
registered https redirect instead. A test asserts it stays absent.

## TikTok

Login Kit v2. Scaffolded and disabled until configured, for the same reason as
Facebook — no native client type, redirect URIs must be absolute `https` and
registered under **App Dashboard → Login Kit → Redirect URI**, and the token
endpoint requires a client secret.

```dotenv
TIKTOK_CLIENT_KEY=
TIKTOK_CLIENT_SECRET=
TIKTOK_REDIRECT_URI=https://you.example/tiktok/callback
```

Three TikTok-specific traps the provider handles, each one a place a generic
OAuth client breaks:

| | TikTok | Everyone else |
|---|---|---|
| Client identifier | `client_key` | `client_id` |
| Scope delimiter | comma | space |
| Failure signal | **HTTP 200** + `error` object | non-2xx status |

That last one is the dangerous one: checking `status.is_success()` alone would
treat a denied request as a valid token response. `check_api_error` inspects
every body before it is trusted, and handles both envelope shapes TikTok uses
(`{"error":{"code":"..."}}` from the Open API, `{"error":"invalid_grant"}` from
the OAuth endpoints).

Scope is `user.info.basic` only. Login Kit authenticates an identity — it grants
no access to private or downloadable media.

## Facebook

Scaffolded against the same trait, disabled until configured, because Facebook
has no native client type:

- loopback `127.0.0.1` redirects are rejected — you must supply a redirect URI
  on a domain whitelisted in your app's *Valid OAuth Redirect URIs*
- the token endpoint requires a real `client_secret`, which cannot be kept
  secret inside a desktop binary

All three keys are required — the provider stays disabled until every one is
set:

```dotenv
FACEBOOK_CLIENT_ID=1234567890
FACEBOOK_CLIENT_SECRET=abcdef...
FACEBOOK_REDIRECT_URI=https://you.example/oauth/callback
```

The redirect URI must be an https host you have added under **App Dashboard →
Facebook Login → Settings → Valid OAuth Redirect URIs**, which in practice means
hosting a small redirect page you control.

Facebook login grants **no** access to private media, and an OAuth token is not
a browser cookie. Do not build on either assumption.

---

## Architecture

```
Connect  →  AuthManager
              ├─ binds ephemeral loopback listener  (auth/callback.rs)
              ├─ provider builds authorize URL      (auth/providers/*)
              ├─ system browser opens it            (tauri-plugin-opener)
              ├─ callback arrives
              ├─ STATE VALIDATED HERE               (before provider code runs)
              ├─ provider exchanges code + PKCE
              ├─ credential  → OS keychain          (auth/storage.rs)
              └─ account     → SQLite               (db.rs)
                                 ↓
                         AccountView → React
```

`AuthManager` holds only `Arc<dyn AuthProvider>`. It has no OAuth URL, no scope
list and no `match` on a platform. The registry lives in
`auth/providers/mod.rs::build_registry`, so adding a platform means writing one
module and adding one line there — `manager.rs` is not touched.

TikTok was added this way as the proof: one new file, one registry line, one
enum variant. No change to `AuthManager`.

Instagram needed one genuine extension — `AuthProvider::can_refresh`, with a
default matching standard OAuth — because it renews a token without a refresh
token. That is the abstraction doing its job: a provider quirk became one
overridable method rather than a special case inside the manager.

### Where secrets are, and are not

| | Access token | Refresh token | Account name / avatar |
|---|---|---|---|
| OS keychain | ✅ | ✅ | — |
| SQLite | ❌ | ❌ | ✅ |
| React / localStorage | ❌ | ❌ | ✅ |
| Logs | ❌ | ❌ | — |

`Credential` implements `Debug` by hand to print `<redacted>`, and no Tauri
command accepts or returns it. If a later feature needs an authenticated
request, it calls `AuthManager::access_token` from Rust and makes the request in
Rust.

### Commands

| Command | Returns |
|---|---|
| `auth_get_providers()` | `ProviderDescriptor[]` |
| `auth_get_accounts()` | `AccountView[]` |
| `auth_get_account(provider)` | `AccountView` |
| `auth_connect(provider)` | `AccountView` |
| `auth_disconnect(provider)` | `AccountView` |

### Events

`auth://started` · `auth://success` · `auth://failed` · `auth://disconnected`

---

## Tests

```bash
cd src-tauri && cargo test
```

47 tests cover: `.env` discovery and the environment-beats-file precedence,
Instagram's `#_` code suffix and refresh-without-a-refresh-token, TikTok's
HTTP-200-with-an-error handling and its `client_key`/comma-scope quirks, PKCE
challenge derivation, state randomness and rejection of
every non-exact match, redaction of tokens in `Debug`, the authorize URL being
HTTPS and scope-minimal, the callback page never echoing parameters, and a
guard test asserting the `accounts` table has no column capable of holding a
secret.

---

## Running in Docker

**Read this first: Media Downloader draws a native desktop window.** A container
has no display, so "one command and it runs" is only literally true on a Linux
host, where the X11 socket can be shared in. On macOS and Windows you would need
an X server (XQuartz / VcXsrv), and the result is worse than just running
`npm run tauri dev`.

What the image *is* genuinely good for, on any host:

- **A reproducible build** — Linux binaries and bundles without installing Rust,
  Node, or the GTK/WebKit development headers.
- **Bundled dependencies** — yt-dlp and FFmpeg are baked in, so the download
  engine and 1080p+ merging work with nothing to install.

### Build the app (works everywhere)

```bash
docker compose run --rm build
```

Artefacts land in `./dist-docker` — the `media-downloader` binary plus whatever
bundles Tauri produced (`.deb`, `.AppImage`).

### Run the app (Linux host)

```bash
xhost +local:docker          # let the container talk to your X server
docker compose up app
```

Downloads are written to `./downloads` on the host, so they survive the
container. `./.env` is mounted read-only if present — it is never copied into
the image, so no client ID is ever baked into a layer.

### Two honest limitations

**Signing in does not work in a bare container.** Credentials go to the OS
secure store, which on Linux means the Secret Service. The image installs
`gnome-keyring` and `dbus-x11`, but nothing starts a keyring daemon, so the
Accounts page will fail to save. **Downloading is unaffected** — it uses no
credentials at all by design, which is the whole point of the split described
above.

**yt-dlp is fetched at image build time, not pinned.** It has to track platform
changes continuously; a version pinned into an image would rot. Rebuild the
image to update it:

```bash
docker compose build --no-cache app
```
