//! Loopback OAuth redirect listener (RFC 8252 §7.3).
//!
//! Why loopback rather than a custom URL scheme: `http://127.0.0.1:{port}` is
//! the redirect style Google's *Desktop app* client type officially supports,
//! it needs no OS-level scheme registration, and any other local app cannot
//! claim it the way it can hijack a custom scheme.
//!
//! The listener binds an ephemeral port on the loopback interface only, serves
//! exactly one request, replies with a small self-contained HTML page, and
//! shuts down. It is never reachable from outside the machine.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::auth::CallbackData;
use crate::errors::{AppError, Result};

/// How long the user has to complete the login in their browser.
pub const FLOW_TIMEOUT: Duration = Duration::from_secs(300);

/// Cap on the request head we are willing to read. A browser GET of our
/// redirect is well under 8 KiB; anything larger is not a redirect we want.
const MAX_REQUEST_BYTES: usize = 8 * 1024;

pub struct CallbackListener {
    listener: TcpListener,
    redirect_uri: String,
}

impl CallbackListener {
    /// Bind an ephemeral loopback port. The OS picks the port, so two flows
    /// can never collide and no fixed port needs reserving.
    pub async fn bind() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| AppError::CallbackListener)?;
        let port = listener
            .local_addr()
            .map_err(|_| AppError::CallbackListener)?
            .port();
        Ok(Self {
            listener,
            redirect_uri: format!("http://127.0.0.1:{port}/callback"),
        })
    }

    /// The exact redirect URI to send in the authorization request and to echo
    /// back during the token exchange.
    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// Serve requests until one is a well-formed `/callback`, then return it.
    ///
    /// Stray requests (favicon probes, port scanners, a browser prefetch) are
    /// answered with 404 and do not end the flow - otherwise a random probe
    /// could cancel a login that is still in progress.
    pub async fn wait_for_callback(self) -> Result<CallbackData> {
        let deadline = tokio::time::Instant::now() + FLOW_TIMEOUT;

        loop {
            let accept = tokio::time::timeout_at(deadline, self.listener.accept()).await;
            let (mut stream, _peer) = match accept {
                Err(_) => return Err(AppError::TimedOut),
                Ok(Err(_)) => continue,
                Ok(Ok(v)) => v,
            };

            let Some(target) = read_request_target(&mut stream).await else {
                let _ = respond(&mut stream, 400, &page_error("Malformed request")).await;
                continue;
            };

            let (path, query) = match target.split_once('?') {
                Some((p, q)) => (p, q),
                None => (target.as_str(), ""),
            };

            if path != "/callback" {
                let _ = respond(&mut stream, 404, &page_error("Not found")).await;
                continue;
            }

            let data = CallbackData::from_query(query);

            // The browser tab is the user's only feedback surface here, so it
            // gets a real page - but never one that echoes back any parameter,
            // since `error_description` is provider-controlled text.
            let body = if data.error.is_some() {
                // A provider that sends an `error_type` alongside `access_denied`
                // is reporting an app-configuration problem, not a user refusal;
                // telling the user they cancelled would be wrong.
                let genuinely_cancelled =
                    data.error.as_deref() == Some("access_denied") && data.error_type.is_none();
                page_result(if genuinely_cancelled {
                    Outcome::Cancelled
                } else {
                    Outcome::Failed
                })
            } else if data.code.is_some() {
                page_result(Outcome::Success)
            } else {
                let _ = respond(&mut stream, 400, &page_error("Missing parameters")).await;
                continue;
            };

            let _ = respond(&mut stream, 200, &body).await;
            let _ = stream.shutdown().await;
            return Ok(data);
        }
    }
}

/// Read the request line and return its target (`/callback?code=...`).
///
/// SECURITY: the target contains the authorization code. It is parsed here and
/// never logged, never printed, and never written anywhere.
async fn read_request_target(stream: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];

    loop {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);

        if buf.len() > MAX_REQUEST_BYTES {
            return None;
        }
        // The request line is complete at the first CRLF; a GET has no body we
        // care about, so there is no reason to read the rest of the headers.
        if let Some(pos) = find_crlf(&buf) {
            let line = std::str::from_utf8(&buf[..pos]).ok()?;
            let mut parts = line.split(' ');
            let method = parts.next()?;
            let target = parts.next()?;
            if method != "GET" {
                return None;
            }
            return Some(target.to_string());
        }
    }
    None
}

fn find_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\r\n")
}

async fn respond(stream: &mut tokio::net::TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        _ => "Not Found",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {len}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n",
        len = body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.flush().await
}

/// Fully self-contained page - no external requests, so it renders identically
/// offline and leaks no referrer to any third party.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Outcome {
    Success,
    /// The user declined at the provider's consent screen.
    Cancelled,
    /// The provider refused for some other reason - usually app configuration.
    Failed,
}

fn page_result(outcome: Outcome) -> String {
    let (glyph, title, sub, accent) = match outcome {
        Outcome::Success => (
            "&#10003;",
            "You're connected",
            "Authorization complete. You can close this tab and return to Media Downloader.",
            "#34d399",
        ),
        Outcome::Cancelled => (
            "&#10005;",
            "Authorization cancelled",
            "Nothing was connected. You can close this tab and try again from the app.",
            "#f87171",
        ),
        Outcome::Failed => (
            "&#10005;",
            "Authorization failed",
            "The platform refused the request. Close this tab and check Media Downloader for details.",
            "#f87171",
        ),
    };
    shell(glyph, title, sub, accent)
}

fn page_error(title: &str) -> String {
    shell("&#10005;", title, "This page is not part of the sign-in flow.", "#f87171")
}

fn shell(glyph: &str, title: &str, sub: &str, accent: &str) -> String {
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="referrer" content="no-referrer">
<title>{title}</title>
<style>
*{{box-sizing:border-box}}
body{{margin:0;min-height:100vh;display:grid;place-items:center;
background:#0b0d12;color:#e8eaf0;
font:16px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif}}
.card{{max-width:26rem;padding:3rem 2.5rem;text-align:center;
background:#12151d;border:1px solid #232838;border-radius:20px;
box-shadow:0 24px 60px rgba(0,0,0,.45)}}
.mark{{width:56px;height:56px;margin:0 auto 1.5rem;display:grid;place-items:center;
border-radius:50%;font-size:26px;color:{accent};
background:color-mix(in srgb,{accent} 14%,transparent);
border:1px solid color-mix(in srgb,{accent} 35%,transparent)}}
h1{{margin:0 0 .5rem;font-size:1.375rem;letter-spacing:-.02em}}
p{{margin:0;color:#8f97ab;font-size:.9375rem}}
</style></head><body>
<div class="card"><div class="mark">{glyph}</div><h1>{title}</h1><p>{sub}</p></div>
</body></html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_callback_query() {
        let d = CallbackData::from_query("code=abc123&state=xyz&scope=openid+profile");
        assert_eq!(d.code.as_deref(), Some("abc123"));
        assert_eq!(d.state.as_deref(), Some("xyz"));
        assert!(d.error.is_none());
    }

    #[test]
    fn parses_denial() {
        let d = CallbackData::from_query("error=access_denied&error_description=User+said+no&state=xyz");
        assert_eq!(d.error.as_deref(), Some("access_denied"));
        assert_eq!(d.error_description.as_deref(), Some("User said no"));
        assert!(d.code.is_none());
    }

    #[test]
    fn success_page_never_echoes_parameters() {
        let page = page_result(Outcome::Success);
        assert!(!page.contains("code="));
        assert!(!page.contains("state="));
    }

    /// A provider-configuration refusal must not be dressed up as the user
    /// having changed their mind.
    #[test]
    fn a_provider_refusal_page_does_not_claim_the_user_cancelled() {
        let page = page_result(Outcome::Failed);
        assert!(page.contains("Authorization failed"));
        assert!(!page.to_lowercase().contains("cancelled"));
    }

    #[test]
    fn error_type_is_parsed_from_the_callback() {
        let d = CallbackData::from_query(
            "error=access_denied&error_type=non_sandbox_target&state=xyz",
        );
        assert_eq!(d.error.as_deref(), Some("access_denied"));
        assert_eq!(d.error_type.as_deref(), Some("non_sandbox_target"));
    }

    #[tokio::test]
    async fn binds_a_loopback_redirect_uri() {
        let l = CallbackListener::bind().await.unwrap();
        let uri = l.redirect_uri().to_string();
        assert!(uri.starts_with("http://127.0.0.1:"), "{uri}");
        assert!(uri.ends_with("/callback"), "{uri}");
    }
}
