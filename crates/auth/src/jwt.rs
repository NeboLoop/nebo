use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

use types::NeboError;

/// Raw JWT claims as a HashMap (matches Go's jwt.MapClaims).
pub type Claims = std::collections::HashMap<String, serde_json::Value>;

/// Structured JWT claims.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JWTClaims {
    /// Subject (user ID) — checks both "sub" and "userId" keys.
    pub sub: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub iss: String,
    #[serde(default)]
    pub exp: i64,
    #[serde(default)]
    pub iat: i64,
}

/// Agent WebSocket token claims.
#[derive(Debug, Serialize, Deserialize)]
struct AgentWSClaims {
    #[serde(rename = "type")]
    token_type: String,
    iat: i64,
    exp: i64,
}

/// Validate a JWT token and return raw claims.
pub fn validate_jwt(token_string: &str, secret: &str) -> Result<Claims, NeboError> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    // We handle claims extraction manually
    validation.required_spec_claims.clear();

    let token_data = decode::<Claims>(
        token_string,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|_| NeboError::InvalidToken)?;

    Ok(token_data.claims)
}

/// Validate a JWT token and return structured claims.
/// Checks both "sub" and "userId" claim keys.
pub fn validate_jwt_claims(token_string: &str, secret: &str) -> Result<JWTClaims, NeboError> {
    let map = validate_jwt(token_string, secret)?;

    // Extract subject — check both "sub" and "userId" keys
    let sub = map
        .get("sub")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            map.get("userId")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
        })
        .ok_or(NeboError::InvalidToken)?
        .to_string();

    let email = map
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let name = map
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let iss = map
        .get("iss")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let exp = map.get("exp").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64;
    let iat = map.get("iat").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64;

    Ok(JWTClaims {
        sub,
        email,
        name,
        iss,
        exp,
        iat,
    })
}

/// Mint a short-lived HS256 JWT for agent WebSocket authentication.
pub fn generate_agent_ws_token(secret: &str, ttl_seconds: i64) -> Result<String, NeboError> {
    let now = chrono::Utc::now().timestamp();
    let claims = AgentWSClaims {
        token_type: "agent_ws".into(),
        iat: now,
        exp: now + ttl_seconds,
    };
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| NeboError::Internal(format!("failed to sign agent WS token: {e}")))
}

/// Validate a JWT intended for agent WebSocket authentication.
/// Verifies signature, expiration, and that the "type" claim is "agent_ws".
pub fn validate_agent_ws_token(token_string: &str, secret: &str) -> Result<(), NeboError> {
    let map = validate_jwt(token_string, secret)?;
    let token_type = map.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if token_type != "agent_ws" {
        return Err(NeboError::InvalidToken);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_and_validate_agent_ws_token() {
        let secret = "test-secret-key-for-testing";
        let token = generate_agent_ws_token(secret, 3600).unwrap();
        assert!(validate_agent_ws_token(&token, secret).is_ok());
        assert!(validate_agent_ws_token(&token, "wrong-secret").is_err());
    }

    #[test]
    fn test_validate_jwt_claims() {
        use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};

        let secret = "test-secret";
        let now = chrono::Utc::now().timestamp();
        let mut claims = Claims::new();
        claims.insert("userId".into(), serde_json::json!("user-123"));
        claims.insert("email".into(), serde_json::json!("test@example.com"));
        claims.insert("exp".into(), serde_json::json!(now + 3600));
        claims.insert("iat".into(), serde_json::json!(now));

        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        )
        .unwrap();

        let parsed = validate_jwt_claims(&token, secret).unwrap();
        assert_eq!(parsed.sub, "user-123");
        assert_eq!(parsed.email, "test@example.com");
    }

    /// A session the owner already holds must survive a library upgrade. These
    /// four HS256 tokens were minted OUTSIDE this crate against the secret
    /// "test-secret" and are checked in as literals, so the test fails the day
    /// the wire format we accept changes — which is the day every signed-in
    /// person is logged out. Do not regenerate them to make the test pass.
    #[test]
    fn tokens_minted_before_the_upgrade_still_validate() {
        let secret = "test-secret";

        // userId + far-future exp — the shape the login path mints.
        let with_exp = "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.eyJ1c2VySWQiOiJ1c2VyLTEyMyIsImVtYWlsIjoidGVzdEBleGFtcGxlLmNvbSIsImlhdCI6MTc1NjY4NDgwMCwiZXhwIjo0MTAyNDQ0ODAwfQ.ettYcpSvffmlBeq7sg-YL3ExG0GGlha-s7wz0C64vEI";
        let parsed = validate_jwt_claims(with_exp, secret).unwrap();
        assert_eq!(parsed.sub, "user-123");
        assert_eq!(parsed.email, "test@example.com");
        assert_eq!(parsed.exp, 4102444800);

        // `sub` instead of `userId`, and NO exp at all — accepted because
        // required_spec_claims is cleared. jsonwebtoken 10.3.0 fixed a type
        // confusion on exp/nbf when they are not required; this is the guard.
        let no_exp = "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ1c2VyLTEyMyIsImVtYWlsIjoidGVzdEBleGFtcGxlLmNvbSIsImlhdCI6MTc1NjY4NDgwMH0.dE-D3FMUERjIOkcRnyKJQYAusgF5FyT1Mmp-hW3WUeU";
        assert_eq!(validate_jwt_claims(no_exp, secret).unwrap().sub, "user-123");

        // Expired in 2001 — still refused.
        let expired = "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.eyJ1c2VySWQiOiJ1c2VyLTEyMyIsImlhdCI6MTAwMDAwMDAwMCwiZXhwIjoxMDAwMDAzNjAwfQ.ew85LG2koFqxJKJ-Ant5KFrCiwENEhL6k0BRDPnCuhw";
        assert!(validate_jwt_claims(expired, secret).is_err());

        // An agent WebSocket token minted before the upgrade still opens /agent/ws.
        let agent_ws = "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9.eyJ0eXBlIjoiYWdlbnRfd3MiLCJpYXQiOjE3NTY2ODQ4MDAsImV4cCI6NDEwMjQ0NDgwMH0.3elr1HdDL9MnDYKSUzHZ6sqBMc3MBI5PF4LY6bnQ9WA";
        assert!(validate_agent_ws_token(agent_ws, secret).is_ok());
        assert!(validate_agent_ws_token(agent_ws, "wrong-secret").is_err());
    }
}
