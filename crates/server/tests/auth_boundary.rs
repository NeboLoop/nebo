//! The protected routes must be protected.
//!
//! `jwt_auth` reads the signing secret out of a request extension. If that
//! extension is not in the request by the time the middleware runs, the
//! middleware has nothing to verify against — and HS256 verifies happily
//! against an empty key, so a token anyone can mint would open `/user/me`.
//! This test drives the real server, booted the way the app boots it, and
//! checks the boundary from outside.
//!
//! Run:
//!   cargo test -p nebo-server --test auth_boundary

use base64::Engine;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::{Value, json};

mod common;
use common::TestServer;

const EMAIL: &str = "owner@example.com";
const PASSWORD: &str = "correct-horse-battery";

/// The user id the server itself put in the token it minted, read out of the
/// payload without verifying it — the way anyone holding one token, or simply
/// knowing the id, would read it.
fn subject_of(token: &str) -> String {
    let payload = token.split('.').nth(1).expect("jwt payload segment");
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .expect("decode jwt payload");
    let claims: Value = serde_json::from_slice(&bytes).expect("parse jwt payload");
    claims["userId"].as_str().expect("userId claim").to_string()
}

/// A token for a real user, signed with the empty string. That is the whole
/// attack: no secret, no guessing, a signature over nothing.
fn token_signed_with_nothing(user_id: &str) -> String {
    let now = chrono::Utc::now().timestamp();
    let claims = json!({
        "userId": user_id,
        "email": EMAIL,
        "iat": now,
        "exp": now + 3600,
    });
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(b""),
    )
    .expect("sign forged token")
}

/// Every route behind `user::protected_routes()`, each with a body its handler
/// accepts. DELETE is last: it removes the account the rest are checked
/// against.
fn protected_calls() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        ("GET", "/user/me", Value::Null),
        ("PUT", "/user/me", json!({ "name": "Renamed" })),
        (
            "POST",
            "/user/me/change-password",
            json!({ "currentPassword": PASSWORD, "newPassword": "a-new-one-entirely" }),
        ),
        ("DELETE", "/user/me", Value::Null),
    ]
}

async fn call(
    server: &TestServer,
    method: &str,
    path: &str,
    body: &Value,
    token: &str,
) -> reqwest::StatusCode {
    let url = server.url(path);
    let builder = match method {
        "GET" => server.client.get(url),
        "PUT" => server.client.put(url),
        "POST" => server.client.post(url),
        "DELETE" => server.client.delete(url),
        other => panic!("unsupported method {other}"),
    }
    .bearer_auth(token);
    let builder = if body.is_null() {
        builder
    } else {
        builder.json(body)
    };
    builder.send().await.unwrap().status()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_protected_routes_are_protected() {
    let server = TestServer::boot().await;

    // A real account, and the real token the server minted for it.
    let registered = server
        .post_json(
            "/auth/register",
            &json!({ "email": EMAIL, "password": PASSWORD, "name": "Owner" }),
        )
        .await;
    assert!(
        registered.status().is_success(),
        "register failed: {}",
        registered.status()
    );
    let body: Value = registered.json().await.unwrap();
    let real_token = body["token"]
        .as_str()
        .expect("token in register response")
        .to_string();
    let forged = token_signed_with_nothing(&subject_of(&real_token));

    // Nothing the forged token touches may open.
    for (method, path, payload) in protected_calls() {
        let status = call(&server, method, path, &payload, &forged).await;
        assert_eq!(
            status,
            reqwest::StatusCode::UNAUTHORIZED,
            "{method} {path} accepted a token signed with the empty string (got {status})",
        );
    }

    // The server's own token must still be let through every one of them.
    for (method, path, payload) in protected_calls() {
        let status = call(&server, method, path, &payload, &real_token).await;
        assert!(
            status.is_success(),
            "{method} {path} refused the server's own correctly signed token (got {status})",
        );
    }

    // The login door's rate limiter is wired the same way and was inert for
    // the same reason: the middleware ran before its `RateLimiter` extension
    // was in the request, and a missing limiter used to mean "no limit".
    // Registering above already spent one of the window's ten.
    let mut saw_429 = false;
    for _ in 0..12 {
        let status = server
            .post_json(
                "/auth/login",
                &json!({ "email": "nobody@example.com", "password": "wrong" }),
            )
            .await
            .status();
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            saw_429 = true;
            break;
        }
    }
    assert!(
        saw_429,
        "thirteen auth attempts from one address were never rate limited",
    );
}
