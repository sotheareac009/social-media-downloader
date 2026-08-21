//! Application-wide error type.
//!
//! SECURITY: no variant here is allowed to carry a token, an authorization
//! code, a `code_verifier`, or a cookie. Errors are logged and are surfaced to
//! the frontend, so anything embedded in them must be safe to disclose.

use serde::Serialize;

pub type Result<T> = std::result::Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("unknown provider: {0}")]
    UnknownProvider(String),

    #[error("provider `{0}` is not configured; set its client id before connecting")]
    ProviderNotConfigured(String),

    #[error("an authorization flow is already in progress")]
    FlowAlreadyRunning,

    #[error("no authorization flow is in progress")]
    NoFlowRunning,

    #[error("the authorization state did not match; the callback was rejected")]
    StateMismatch,

    #[error("the authorization was cancelled")]
    Cancelled,

    #[error("the authorization timed out")]
    TimedOut,

    #[error("the provider denied the request: {0}")]
    ProviderDenied(String),

    /// The provider rejected the request because of how the *app* is set up on
    /// their side, not because of anything the user did. The payload is
    /// operator-facing guidance written by the provider module, so it is shown
    /// verbatim; it must never contain a token, code or provider message.
    #[error("{0}")]
    ProviderConfiguration(String),

    /// Redacted transport failure. Never carries a response body, because a
    /// token endpoint body can contain credentials.
    #[error("network request to the provider failed")]
    Network,

    #[error("the provider returned an unexpected response")]
    MalformedProviderResponse,

    #[error("no stored credential for provider `{0}`")]
    CredentialNotFound(String),

    #[error("secure credential storage is unavailable: {0}")]
    Keychain(String),

    #[error("local database error: {0}")]
    Database(String),

    #[error("could not start the local callback listener")]
    CallbackListener,

    #[error("could not open the system browser")]
    BrowserLaunch,

    // ------------------------------------------------------------ download

    /// The URL is not a link this build knows how to handle.
    #[error("that link isn't a supported Facebook or TikTok video URL")]
    UnsupportedUrl,

    /// Facebook ephemeral Stories - no extractor supports them.
    #[error("Facebook Stories can't be downloaded — no downloader supports them. Regular videos, reels and posts work.")]
    FacebookStoriesUnsupported,

    /// An Instagram profile or tab, which yt-dlp cannot currently list.
    /// Separated from `UnsupportedUrl` because the answer is "not yet, and
    /// here is the workaround" rather than "wrong kind of link".
    #[error("Instagram profiles can't be listed yet — paste individual reel links instead")]
    InstagramProfileUnsupported,

    /// yt-dlp is not installed or not on PATH.
    #[error("the download engine (yt-dlp) was not found on this system")]
    EngineMissing,

    /// gallery-dl is not installed. Only needed for Instagram profile
    /// listing, so this is separate from `EngineMissing` - downloads work
    /// fine without it.
    #[error("listing Instagram profiles needs gallery-dl, which isn't installed")]
    ListerMissing,

    /// yt-dlp ran but failed. Carries only its final, already-sanitized line.
    #[error("the download engine failed: {0}")]
    EngineFailed(String),

    /// The post exists but is not publicly readable without signing in.
    #[error("this post is private or requires a login, so it cannot be downloaded")]
    MediaNotPublic,

    /// The post is public but carries no downloadable video stream.
    #[error("no downloadable video was found at that link")]
    NoMediaFound,

    /// The platform refused this request but the post is fine - anti-bot
    /// throttling, which is what TikTok does when asked for many videos in a
    /// row. Distinct from `NoMediaFound` because it is worth retrying and
    /// because telling someone their video doesn't exist would be a lie.
    #[error("the platform is rate-limiting us; this usually succeeds on a retry")]
    TemporarilyUnavailable,

    /// The media CDN refused the request outright - YouTube's anti-bot layer
    /// answering a client it doesn't like. Distinct from throttling because
    /// waiting does not help; only asking as a different client does.
    #[error("the platform refused this request; trying a different client")]
    ClientRefused,

    #[error("no download job with that id")]
    JobNotFound,

    #[error("could not write to the download folder: {0}")]
    DownloadPath(String),

    // --- licensing ---
    // Distinct variants rather than one "bad licence": the UI has to say
    // something different for each, and "expired" in particular must not read
    // as "you typed it wrong".
    #[error("this build has no licence key configured")]
    LicenseNotConfigured,

    #[error("that doesn't look like a licence key")]
    LicenseMalformed,

    #[error("that licence key isn't valid")]
    LicenseInvalid,

    #[error("that licence key has expired")]
    LicenseExpired,

    #[error("that licence key needs a newer version of the app")]
    LicenseUnsupported,


    // ----------------------------------------------- emulator publishing
    //
    // These describe the *device*, never the social account: there is no
    // credential in this path to leak, because the login stays inside the
    // Android app.

    #[error("ADB was not found. Install LDPlayer, or set the ADB path in Settings.")]
    AdbMissing,

    #[error("ADB failed: {0}")]
    AdbFailed(String),

    #[error("LDPlayer was not found on this computer. Set its folder in Settings if it is installed somewhere unusual.")]
    LdPlayerMissing,

    #[error("LDPlayer reported a problem: {0}")]
    LdPlayerFailed(String),

    /// A hand-typed connect address that isn't `host:port`.
    #[error("`{0}` isn't a device address — expected something like 127.0.0.1:5555")]
    InvalidDeviceAddress(String),

    #[error("no LDPlayer instance with id `{0}`")]
    InstanceNotFound(String),

    #[error("the emulator `{0}` is not responding; start it in LDPlayer and try again")]
    InstanceOffline(String),

    /// Asked to start or stop something this app does not control - a phone,
    /// or another vendor's emulator.
    #[error("`{0}` is not an LDPlayer instance, so this app can't start or stop it")]
    NotAnLdplayerInstance(String),

    #[error("could not find the video file `{0}`")]
    MediaFileMissing(String),

    #[error("copying the video to the emulator failed: {0}")]
    MediaTransferFailed(String),

    /// The file is on the device but Android's gallery cannot see it, which
    /// means every app's media picker will come up empty.
    #[error("the video was copied but Android's gallery did not index it, so the app won't see it")]
    MediaScanFailed,

    #[error("`{0}` is not installed on that emulator")]
    AppNotInstalled(String),

    #[error("could not open `{0}` on the emulator")]
    AppLaunchFailed(String),

    #[error("no account with id `{0}`")]
    AccountNotFound(String),

    #[error("{0}")]
    PackagePlatformMismatch(String),

    #[error("no publishing job with that id")]
    PublishJobNotFound(String),

    #[error("a job that is `{0}` can't be retried")]
    JobNotRetryable(String),

    #[error("choose at least one account to publish to")]
    NoAccountsSelected,

    #[error("choose at least one video or photo to publish")]
    NoMediaSelected,

    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    /// Stable machine-readable code the UI can branch on.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownProvider(_) => "unknown_provider",
            Self::ProviderNotConfigured(_) => "provider_not_configured",
            Self::FlowAlreadyRunning => "flow_already_running",
            Self::NoFlowRunning => "no_flow_running",
            Self::StateMismatch => "state_mismatch",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::ProviderDenied(_) => "provider_denied",
            Self::ProviderConfiguration(_) => "provider_configuration",
            Self::Network => "network",
            Self::MalformedProviderResponse => "malformed_provider_response",
            Self::CredentialNotFound(_) => "credential_not_found",
            Self::Keychain(_) => "keychain",
            Self::Database(_) => "database",
            Self::CallbackListener => "callback_listener",
            Self::BrowserLaunch => "browser_launch",
            Self::UnsupportedUrl => "unsupported_url",
            Self::FacebookStoriesUnsupported => "facebook_stories_unsupported",
            Self::InstagramProfileUnsupported => "instagram_profile_unsupported",
            Self::EngineMissing => "engine_missing",
            Self::ListerMissing => "lister_missing",
            Self::EngineFailed(_) => "engine_failed",
            Self::MediaNotPublic => "media_not_public",
            Self::NoMediaFound => "no_media_found",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
            Self::ClientRefused => "client_refused",
            Self::JobNotFound => "job_not_found",
            Self::DownloadPath(_) => "download_path",
            Self::LicenseNotConfigured => "license_not_configured",
            Self::LicenseMalformed => "license_malformed",
            Self::LicenseInvalid => "license_invalid",
            Self::LicenseExpired => "license_expired",
            Self::LicenseUnsupported => "license_unsupported",
            Self::AdbMissing => "adb_missing",
            Self::AdbFailed(_) => "adb_failed",
            Self::LdPlayerMissing => "ldplayer_missing",
            Self::LdPlayerFailed(_) => "ldplayer_failed",
            Self::InvalidDeviceAddress(_) => "invalid_device_address",
            Self::InstanceNotFound(_) => "instance_not_found",
            Self::InstanceOffline(_) => "instance_offline",
            Self::NotAnLdplayerInstance(_) => "not_an_ldplayer_instance",
            Self::MediaFileMissing(_) => "media_file_missing",
            Self::MediaTransferFailed(_) => "media_transfer_failed",
            Self::MediaScanFailed => "media_scan_failed",
            Self::AppNotInstalled(_) => "app_not_installed",
            Self::AppLaunchFailed(_) => "app_launch_failed",
            Self::AccountNotFound(_) => "account_not_found",
            Self::PackagePlatformMismatch(_) => "package_platform_mismatch",
            Self::PublishJobNotFound(_) => "publish_job_not_found",
            Self::JobNotRetryable(_) => "job_not_retryable",
            Self::NoAccountsSelected => "no_accounts_selected",
            Self::NoMediaSelected => "no_media_selected",
            Self::Internal(_) => "internal",
        }
    }
}

/// Wire shape sent to the frontend.
#[derive(Debug, Serialize)]
pub struct ErrorPayload {
    pub code: String,
    pub message: String,
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        ErrorPayload {
            code: self.code().to_string(),
            message: self.to_string(),
        }
        .serialize(s)
    }
}

// `reqwest` errors can embed the request URL (which for a token exchange is
// fine) but their bodies must never leak, so we collapse them to a unit variant.
impl From<reqwest::Error> for AppError {
    fn from(_: reqwest::Error) -> Self {
        AppError::Network
    }
}

impl From<keyring::Error> for AppError {
    fn from(e: keyring::Error) -> Self {
        match e {
            keyring::Error::NoEntry => AppError::CredentialNotFound("<unknown>".into()),
            other => AppError::Keychain(other.to_string()),
        }
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::Database(e.to_string())
    }
}
