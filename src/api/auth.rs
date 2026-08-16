//! Basic token auth middleware.
//!
//! The dnshub API is designed to sit behind Traefik/Authelia for
//! authentication (PRD section 4.9). However, a simple bearer-token
//! check is provided for standalone deployments.
//!
//! When `auth_token` is `None`, all requests pass through (the default,
//! suitable for behind-Traefik deployments). When set, requests must
//! include an `Authorization: Bearer <token>` header matching the
//! configured token.

use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;

/// Check whether the request's Authorization header matches the expected
/// bearer token. Returns `Ok(())` if the request should be allowed, or
/// `Err(StatusCode::UNAUTHORIZED)` if it should be rejected.
fn check_token(expected_token: &Option<String>, req: &Request) -> Result<(), StatusCode> {
    let Some(token) = expected_token else {
        return Ok(());
    };
    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    match auth_header {
        Some(h) if h.starts_with("Bearer ") => {
            let provided = &h["Bearer ".len()..];
            if provided == token {
                Ok(())
            } else {
                Err(StatusCode::UNAUTHORIZED)
            }
        }
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

/// Auth middleware: checks the `Authorization: Bearer <token>` header.
///
/// If `expected_token` is `None`, the middleware is a no-op (all requests
/// pass through). This is the default when the API is behind Traefik/
/// Authelia.
pub async fn require_token(
    expected_token: Option<String>,
    req: Request,
    next: Next,
) -> Response {
    match check_token(&expected_token, &req) {
        Ok(()) => next.run(req).await,
        Err(status) => Response::builder()
            .status(status)
            .body(axum::body::Body::from("unauthorized"))
            .unwrap(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;

    fn make_request(auth: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().uri("/ok");
        if let Some(a) = auth {
            builder = builder.header("authorization", a);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[test]
    fn no_token_allows_all() {
        let req = make_request(None);
        assert!(check_token(&None, &req).is_ok());
    }

    #[test]
    fn valid_token_allows() {
        let token = Some("secret".to_string());
        let req = make_request(Some("Bearer secret"));
        assert!(check_token(&token, &req).is_ok());
    }

    #[test]
    fn missing_header_rejected() {
        let token = Some("secret".to_string());
        let req = make_request(None);
        assert_eq!(check_token(&token, &req), Err(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn wrong_token_rejected() {
        let token = Some("secret".to_string());
        let req = make_request(Some("Bearer wrong"));
        assert_eq!(check_token(&token, &req), Err(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn non_bearer_header_rejected() {
        let token = Some("secret".to_string());
        let req = make_request(Some("Basic secret"));
        assert_eq!(check_token(&token, &req), Err(StatusCode::UNAUTHORIZED));
    }
}
