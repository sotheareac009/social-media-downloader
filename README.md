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

TikTok's feed endpoint is flaky when hit anonymously and intermittently answers
`Unable to extract secondary user ID` for a profile that works moments later.
That is a TikTok-side rate limit, not a broken link; retry.

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
