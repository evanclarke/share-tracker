//! The request guard: two checks on *where a request came from*, applied in
//! front of every route by `app::router`, whatever `[auth]` says.
//!
//! **Cross-site writes are refused.** A browser sends some cross-origin
//! requests without any CORS preflight — a plain HTML `<form>` can `POST`
//! `text/plain`, `multipart/form-data` or `application/x-www-form-urlencoded`
//! to any URL. Every JSON write here refuses those content types (`415`), but a
//! handler that reads no body at all (`/corporate_actions/{id}/demerge`,
//! `/ess_statements/{id}/vest`, `POST /jobs/{name}`, …), the feed imports that
//! take a bare `String` (`/rba_fx_rates/import` would store an attacker's FX
//! rates, and the import never overwrites a stored one), and the multipart
//! upload do not. With `[auth]` configured the `SameSite=Lax` session cookie
//! already withholds the credential from such a request; without `[auth]` —
//! the default — there is no credential to withhold, so any page the user
//! visited could write to `http://127.0.0.1:3000`. So an unsafe method
//! (anything but `GET`/`HEAD`/`OPTIONS`) is refused `403` when the browser
//! says it is cross-origin: `Sec-Fetch-Site` other than `same-origin`/`none`,
//! or — for a browser too old to send that header — an `Origin` that does not
//! name the request's own `Host`. A client that sends neither header (curl, a
//! deployment script, Claude Code) is not a browser being driven by another
//! site, and is let through.
//!
//! **Without `[auth]`, unknown `Host` names are refused (DNS rebinding).** A
//! page on `evil.example` can re-point its own name at `127.0.0.1` after
//! loading, and its scripts are then *same-origin* with this server under that
//! name — reading every response, the cross-site check above satisfied. The
//! one thing such a request cannot fake is its `Host` header, which still says
//! `evil.example`. So with no `[auth]`, a request is served only when its
//! `Host` is `localhost`, an IP address (a literal address cannot be
//! rebound), or a name listed in the config file's `allowed_hosts`. With
//! `[auth]` configured the check is skipped: the rebound origin holds no
//! session cookie (cookies belong to the name, not the address), so it reaches
//! nothing but the login page, and a reverse-proxy deployment need not list
//! its public name twice. A request with no `Host` at all is not a browser's
//! and is served.

use axum::{
    extract::{Request, State},
    http::{HeaderMap, Method, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use super::http::ApiError;

/// The `Sec-Fetch-Site` header, which axum's `header` module does not name.
const SEC_FETCH_SITE: &str = "sec-fetch-site";

/// What the guard checks; built once in `app::router`.
#[derive(Clone, Debug)]
pub struct RequestGuard {
    /// `None` when `[auth]` is configured (the `Host` check is skipped — see
    /// the module docs); otherwise the configured `allowed_hosts`, already
    /// normalised by [`normalise_host`].
    allowed_hosts: Option<Vec<String>>,
}

impl RequestGuard {
    /// `auth_configured` decides whether the `Host` check runs at all;
    /// `allowed_hosts` are the config file's extra names (beyond `localhost`
    /// and IP addresses, which are always allowed).
    pub fn new(auth_configured: bool, allowed_hosts: &[String]) -> Self {
        RequestGuard {
            allowed_hosts: (!auth_configured)
                .then(|| allowed_hosts.iter().map(|h| normalise_host(h)).collect()),
        }
    }

    /// The reason to refuse a request with these parts, or `None` to serve it.
    fn refusal(&self, method: &Method, headers: &HeaderMap) -> Option<String> {
        let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
        if let (Some(allowed), Some(host)) = (&self.allowed_hosts, host)
            && !host_allowed(host, allowed)
        {
            let name = normalise_host(host);
            return Some(format!(
                "this server does not answer to the name '{name}': with no [auth] configured it \
                 serves only localhost, IP addresses, and the names listed in the config file's \
                 allowed_hosts (a guard against DNS rebinding) — add \"{name}\" to allowed_hosts \
                 to reach it by that name"
            ));
        }
        if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
            return None;
        }
        cross_site_refusal(headers, host)
    }
}

/// The `Host` check: `localhost`, any IP address, or a configured name.
fn host_allowed(host: &str, allowed: &[String]) -> bool {
    let name = normalise_host(host);
    name == "localhost" || name.parse::<std::net::IpAddr>().is_ok() || allowed.contains(&name)
}

/// A `Host` header value (or a configured name) reduced to the bare, lowercase
/// name: the port and an IPv6 literal's brackets dropped, as is a trailing
/// root dot (`localhost.` is `localhost`).
pub(crate) fn normalise_host(raw: &str) -> String {
    let raw = raw.trim();
    let name = if let Some(rest) = raw.strip_prefix('[') {
        // `[::1]:3000` — the address is everything up to the bracket.
        rest.split(']').next().unwrap_or(rest)
    } else {
        match raw.rsplit_once(':') {
            Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => name,
            _ => raw,
        }
    };
    name.trim_end_matches('.').to_ascii_lowercase()
}

/// The cross-origin check for an unsafe method (see the module docs).
fn cross_site_refusal(headers: &HeaderMap, host: Option<&str>) -> Option<String> {
    let refuse = |from: &str| {
        Some(format!(
            "refused a cross-site request ({from}): a write must come from this application's \
             own pages or from a client that is not a browser"
        ))
    };
    if let Some(site) = headers.get(SEC_FETCH_SITE) {
        return match site.to_str().unwrap_or("") {
            "same-origin" | "none" => None,
            other => refuse(&format!("Sec-Fetch-Site: {other}")),
        };
    }
    let origin = headers.get(header::ORIGIN)?.to_str().unwrap_or("null");
    let authority = origin.split_once("://").map(|(_, rest)| rest);
    match (authority, host) {
        (Some(authority), Some(host)) if authority.eq_ignore_ascii_case(host.trim()) => None,
        _ => refuse(&format!("Origin: {origin}")),
    }
}

/// The middleware itself, wired in with `axum::middleware::from_fn_with_state`
/// by `app::router` outside every route, the login page included.
pub async fn guard(State(guard): State<RequestGuard>, req: Request, next: Next) -> Response {
    match guard.refusal(req.method(), req.headers()) {
        None => next.run(req).await,
        Some(reason) => {
            tracing::warn!(method = %req.method(), path = %req.uri().path(), "{reason}");
            ApiError::forbidden(reason).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ApiClient, test_pool};
    use axum::http::StatusCode;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
        }
        map
    }

    fn open() -> RequestGuard {
        RequestGuard::new(false, &["Bigbrain.lan".to_string()])
    }

    #[test]
    fn a_host_header_is_reduced_to_its_bare_lowercase_name() {
        assert_eq!(normalise_host("LocalHost:3000"), "localhost");
        assert_eq!(normalise_host("[::1]:3000"), "::1");
        assert_eq!(normalise_host("[::1]"), "::1");
        assert_eq!(normalise_host("127.0.0.1"), "127.0.0.1");
        assert_eq!(normalise_host("bigbrain.lan.:3000"), "bigbrain.lan");
    }

    /// Without `[auth]`: localhost, any IP literal and the configured names are
    /// served; any other name is the DNS-rebinding case and is refused — on a
    /// read too, since reading the portfolio is what rebinding is for.
    #[test]
    fn without_auth_only_localhost_ip_literals_and_listed_names_are_served() {
        let guard = open();
        for host in [
            "localhost:3000",
            "127.0.0.1:3000",
            "[::1]:3000",
            "192.168.1.20:3000",
            "bigbrain.lan:3000",
            "BIGBRAIN.LAN",
        ] {
            assert_eq!(
                guard.refusal(&Method::GET, &headers(&[("host", host)])),
                None,
                "{host}"
            );
        }
        let refused = guard
            .refusal(&Method::GET, &headers(&[("host", "evil.example:3000")]))
            .expect("an unlisted name is refused");
        assert!(refused.contains("'evil.example'"), "{refused}");
        assert!(refused.contains("allowed_hosts"), "{refused}");
        // No Host header at all is not a browser's request.
        assert_eq!(guard.refusal(&Method::GET, &HeaderMap::new()), None);
    }

    /// With `[auth]` the rebound origin has no session cookie, so the `Host`
    /// check is skipped — a proxied deployment's public name needs no listing.
    #[test]
    fn with_auth_any_host_is_served() {
        let guard = RequestGuard::new(true, &[]);
        assert_eq!(
            guard.refusal(&Method::GET, &headers(&[("host", "tracker.example.com")])),
            None
        );
    }

    #[test]
    fn a_write_the_browser_marks_cross_site_is_refused_whatever_auth_says() {
        for guard in [open(), RequestGuard::new(true, &[])] {
            for site in ["cross-site", "same-site"] {
                let h = headers(&[("host", "localhost:3000"), ("sec-fetch-site", site)]);
                for method in [Method::POST, Method::PUT, Method::DELETE, Method::PATCH] {
                    let refused = guard.refusal(&method, &h).expect("refused");
                    assert!(refused.contains(site), "{refused}");
                }
                // A cross-site *read* is a navigation or a link; it is served.
                assert_eq!(guard.refusal(&Method::GET, &h), None);
            }
            for site in ["same-origin", "none"] {
                let h = headers(&[("host", "localhost:3000"), ("sec-fetch-site", site)]);
                assert_eq!(guard.refusal(&Method::POST, &h), None, "{site}");
            }
        }
    }

    /// The fallback for a browser that sends `Origin` but not `Sec-Fetch-Site`.
    #[test]
    fn without_sec_fetch_site_the_origin_must_name_the_host() {
        let guard = open();
        let same = headers(&[
            ("host", "localhost:3000"),
            ("origin", "http://localhost:3000"),
        ]);
        assert_eq!(guard.refusal(&Method::POST, &same), None);
        for origin in ["http://evil.example", "http://localhost:8080", "null"] {
            let h = headers(&[("host", "localhost:3000"), ("origin", origin)]);
            assert!(guard.refusal(&Method::POST, &h).is_some(), "{origin}");
        }
        // Neither header: curl, a script, Claude Code — not a browser.
        let h = headers(&[("host", "localhost:3000")]);
        assert_eq!(guard.refusal(&Method::POST, &h), None);
    }

    /// End to end through the real application: a bodyless operation `POST` a
    /// cross-site form could send is refused `403` with the reason before its
    /// handler runs, and an unlisted `Host` cannot read the portfolio.
    #[tokio::test]
    async fn the_application_refuses_cross_site_writes_and_unknown_hosts() {
        let pool = test_pool().await;
        let resp = ApiClient::full(&pool)
            .with_header("Sec-Fetch-Site", "cross-site")
            .with_header("Content-Type", "text/plain")
            .post_empty("/jobs/backup")
            .await;
        assert_eq!(resp.status, StatusCode::FORBIDDEN);
        assert!(resp.text().contains("cross-site"), "{}", resp.text());

        let resp = ApiClient::full(&pool)
            .with_header("Host", "evil.example")
            .get("/listings")
            .await;
        assert_eq!(resp.status, StatusCode::FORBIDDEN);
        assert!(resp.text().contains("allowed_hosts"), "{}", resp.text());

        let resp = ApiClient::full(&pool)
            .with_header("Host", "127.0.0.1:3000")
            .with_header("Sec-Fetch-Site", "same-origin")
            .get("/listings")
            .await;
        assert_eq!(resp.status, StatusCode::OK);
    }
}
