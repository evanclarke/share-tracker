//! The browser-hardening headers on every response, layered in `app::router`
//! outside the request guard (so a refusal carries them too) and inside only
//! the panic net.
//!
//! The threat is clickjacking: with `[auth]` off (the default) a hostile page
//! could frame the app and trick a click on a delete or job button — clicks
//! inside the frame are same-origin, so `request_guard`'s `Sec-Fetch-Site`
//! check passes them. `X-Frame-Options: DENY` plus the CSP's
//! `frame-ancestors 'none'` refuse the framing itself (the header for older
//! browsers, the directive for current ones).
//!
//! The rest of the policy costs nothing because the frontend loads nothing
//! third-party: every script, stylesheet and frame is same-origin
//! (`default-src 'self'`). The one inline script — `web::THEME_BOOT_SCRIPT`,
//! which must run before the first paint — is allowed by its SHA-256 hash,
//! computed here from the very const both pages substitute, so editing it
//! cannot leave the policy blocking it. The views set styles only through the
//! CSSOM (`el.style.x = …`), which `style-src` does not govern; no markup
//! carries a `style=` attribute.
//!
//! A handler that must answer differently sets its own headers and the layer
//! leaves them alone: the attachment content route is framed by the app's own
//! document pane, so it answers [`FRAME_SAME_ORIGIN`] /
//! [`FRAME_ANCESTORS_SELF`] instead.
use crate::web;
use std::sync::LazyLock;

use axum::http::{HeaderValue, header};
use axum::response::Response;
use sha2::{Digest, Sha256};

/// `X-Frame-Options` for a response the app frames itself.
pub const FRAME_SAME_ORIGIN: &str = "SAMEORIGIN";

/// `Content-Security-Policy` for a response the app frames itself: framing by
/// this origin only, and nothing else — the attachment bytes are a PDF, image
/// or text file, and a `default-src` on a PDF can stop Chrome's viewer.
pub const FRAME_ANCESTORS_SELF: &str = "frame-ancestors 'self'";

/// The policy every other response carries.
static POLICY: LazyLock<HeaderValue> =
    LazyLock::new(|| HeaderValue::try_from(policy()).expect("the policy is ASCII"));

fn policy() -> String {
    format!(
        "default-src 'self'; script-src 'self' 'sha256-{}'; object-src 'none'; \
         base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
        base64(&Sha256::digest(theme_boot_body().as_bytes()))
    )
}

/// The text between `THEME_BOOT_SCRIPT`'s `<script>` tags — what a browser
/// hashes to match a `'sha256-…'` source.
fn theme_boot_body() -> &'static str {
    let script = web::THEME_BOOT_SCRIPT;
    let (_, rest) = script
        .split_once("<script>")
        .expect("the theme boot script opens with <script>");
    let (body, _) = rest
        .rsplit_once("</script>")
        .expect("the theme boot script closes with </script>");
    body
}

/// Standard padded base64 (RFC 4648 §4), the encoding a CSP hash source takes.
/// Written out rather than a dependency: this is its one use.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (u32::from(b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 0x3f) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The layer itself (`axum::middleware::map_response`): adds each header the
/// handler did not set.
pub async fn apply(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers
        .entry(header::X_FRAME_OPTIONS)
        .or_insert(HeaderValue::from_static("DENY"));
    headers
        .entry(header::CONTENT_SECURITY_POLICY)
        .or_insert_with(|| POLICY.clone());
    headers
        .entry(header::X_CONTENT_TYPE_OPTIONS)
        .or_insert(HeaderValue::from_static("nosniff"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web;

    /// RFC 4648 §10's test vectors, covering every padding length.
    #[test]
    fn base64_matches_the_rfc_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected, "{input:?}");
        }
    }

    /// The hash is over the script's text alone — the tags excluded, the
    /// surrounding whitespace inside them kept, as the browser hashes it.
    #[test]
    fn the_hash_covers_the_whole_inline_script_body() {
        let body = theme_boot_body();
        assert!(body.contains("localStorage.getItem('share-tracker.theme')"));
        assert!(!body.contains("<script") && !body.contains("</script"));
        assert_eq!(
            format!("<script>{body}</script>"),
            web::THEME_BOOT_SCRIPT.trim_start()
        );
        // SHA-256 is 32 bytes: 44 base64 characters, one `=` of padding.
        let hash = base64(&Sha256::digest(body.as_bytes()));
        assert_eq!(hash.len(), 44);
        assert!(policy().contains(&format!("'sha256-{hash}'")));
    }
}
