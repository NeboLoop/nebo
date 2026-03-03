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
    let exp = map
        .get("exp")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as i64;
    let iat = map
        .get("iat")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as i64;

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
pub fn generate_agent_ws_token(
    secret: &str,
    ttl_seconds: i64,
) -> Result<String, NeboError> {
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
    let token_type = map
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
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
        claims.insert("email".into(), serde_json::json!("test@test.com"));
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
        assert_eq!(parsed.email, "test@test.com");
    }
}
