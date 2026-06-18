use axum::{
    body::Body,
    extract::State,
    http::{
        HeaderMap, HeaderValue, Request,
        header::{COOKIE, SET_COOKIE},
    },
    middleware::Next,
    response::Response,
};
use deployment::Deployment;
use uuid::Uuid;

use crate::DeploymentImpl;

const AUTH_SESSION_COOKIE_NAME: &str = "vk_auth_session";
const AUTH_SESSION_COOKIE_MAX_AGE_SECS: u64 = 365 * 24 * 60 * 60;

pub async fn bind_auth_session(
    State(deployment): State<DeploymentImpl>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let (auth_session_id, should_set_cookie) = match parse_auth_session_cookie(request.headers()) {
        Some(value) => (value, false),
        None => (Uuid::new_v4().to_string(), true),
    };
    let secure_cookie = should_use_secure_cookie(request.headers());

    deployment
        .auth_context()
        .run_with_session(auth_session_id.clone(), async move {
            let mut response = next.run(request).await;

            if should_set_cookie
                && let Ok(cookie_header) =
                    build_auth_session_cookie_header(&auth_session_id, secure_cookie)
            {
                response.headers_mut().append(SET_COOKIE, cookie_header);
            }

            response
        })
        .await
}

fn parse_auth_session_cookie(headers: &HeaderMap) -> Option<String> {
    let cookies = headers.get(COOKIE)?.to_str().ok()?;
    for fragment in cookies.split(';') {
        let mut parts = fragment.trim().splitn(2, '=');
        let Some(name) = parts.next().map(str::trim) else {
            continue;
        };
        if name != AUTH_SESSION_COOKIE_NAME {
            continue;
        }

        let value = parts.next().map(str::trim).unwrap_or_default();
        if let Ok(parsed) = Uuid::parse_str(value) {
            return Some(parsed.to_string());
        }
    }
    None
}

fn should_use_secure_cookie(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .is_some_and(|proto| proto.eq_ignore_ascii_case("https"))
}

fn build_auth_session_cookie_header(
    auth_session_id: &str,
    secure: bool,
) -> Result<HeaderValue, axum::http::header::InvalidHeaderValue> {
    let mut cookie = format!(
        "{AUTH_SESSION_COOKIE_NAME}={auth_session_id}; Path=/; HttpOnly; SameSite=Lax; Max-Age={AUTH_SESSION_COOKIE_MAX_AGE_SECS}",
    );
    if secure {
        cookie.push_str("; Secure");
    }
    HeaderValue::from_str(&cookie)
}
