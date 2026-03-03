use std::sync::Arc;

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::json;

use config::Config;
use db::Store;
use types::NeboError;

/// Auth response with access and refresh tokens.
pub struct AuthResponse {
    pub token: String,
    pub refresh_token: String,
    pub expires_at: i64,
}

/// Handles local authentication with SQLite.
pub struct AuthService {
    store: Arc<Store>,
    config: Config,
}

impl AuthService {
    pub fn new(store: Arc<Store>, config: Config) -> Self {
        Self { store, config }
    }

    /// Register creates a new user account.
    pub fn register(
        &self,
        email: &str,
        password: &str,
        name: &str,
    ) -> Result<AuthResponse, NeboError> {
        // Check if email already exists
        if self.store.check_email_exists(email)? {
            return Err(NeboError::EmailExists);
        }

        // Hash password
        let hash = bcrypt::hash(password, bcrypt::DEFAULT_COST)
            .map_err(|e| NeboError::Internal(format!("failed to hash password: {e}")))?;

        let user_id = generate_id();

        // Create user
        self.store.create_user(&user_id, email, &hash, name)?;

        // Create default preferences
        self.store.create_user_preferences(&user_id)?;

        // Generate tokens
        self.generate_tokens(&user_id, email)
    }

    /// Login authenticates a user and returns tokens.
    pub fn login(&self, email: &str, password: &str) -> Result<AuthResponse, NeboError> {
        let user = self
            .store
            .get_user_by_email(email)?
            .ok_or(NeboError::InvalidCredentials)?;

        // Verify password
        let valid = bcrypt::verify(password, &user.password_hash)
            .map_err(|_| NeboError::InvalidCredentials)?;
        if !valid {
            return Err(NeboError::InvalidCredentials);
        }

        self.generate_tokens(&user.id, &user.email)
    }

    /// Generate new tokens from a refresh token.
    pub fn refresh_token(&self, refresh_token: &str) -> Result<AuthResponse, NeboError> {
        let token_hash = hash_token(refresh_token);

        let stored = self
            .store
            .get_refresh_token_by_hash(&token_hash)?
            .ok_or(NeboError::InvalidToken)?;

        let user = self
            .store
            .get_user_by_id(&stored.user_id)?
            .ok_or(NeboError::UserNotFound)?;

        // Delete old token
        let _ = self.store.delete_refresh_token(&token_hash);

        self.generate_tokens(&user.id, &user.email)
    }

    /// Get a user by ID.
    pub fn get_user_by_id(&self, user_id: &str) -> Result<Option<db::models::User>, NeboError> {
        self.store.get_user_by_id(user_id)
    }

    /// Get a user by email.
    pub fn get_user_by_email(
        &self,
        email: &str,
    ) -> Result<Option<db::models::User>, NeboError> {
        self.store.get_user_by_email(email)
    }

    /// Change a user's password.
    pub fn change_password(
        &self,
        user_id: &str,
        current_password: &str,
        new_password: &str,
    ) -> Result<(), NeboError> {
        let user = self
            .store
            .get_user_by_id(user_id)?
            .ok_or(NeboError::UserNotFound)?;

        let valid = bcrypt::verify(current_password, &user.password_hash)
            .map_err(|_| NeboError::InvalidCredentials)?;
        if !valid {
            return Err(NeboError::InvalidCredentials);
        }

        let new_hash = bcrypt::hash(new_password, bcrypt::DEFAULT_COST)
            .map_err(|e| NeboError::Internal(format!("failed to hash password: {e}")))?;

        self.store.update_user_password(user_id, &new_hash)
    }

    /// Delete a user account.
    pub fn delete_user(&self, user_id: &str) -> Result<(), NeboError> {
        self.store.delete_user(user_id)
    }

    /// Create a password reset token.
    pub fn create_password_reset_token(&self, email: &str) -> Result<Option<String>, NeboError> {
        let user = match self.store.get_user_by_email(email)? {
            Some(u) => u,
            None => return Ok(None), // Don't reveal if email exists
        };

        let token = generate_token();
        let expires = chrono::Utc::now().timestamp() + 3600; // 1 hour

        self.store
            .set_password_reset_token(&user.id, &token, expires)?;

        Ok(Some(token))
    }

    /// Reset a user's password using a token.
    pub fn reset_password(&self, token: &str, new_password: &str) -> Result<(), NeboError> {
        let user = self
            .store
            .get_user_by_password_reset_token(token)?
            .ok_or(NeboError::InvalidToken)?;

        let hash = bcrypt::hash(new_password, bcrypt::DEFAULT_COST)
            .map_err(|e| NeboError::Internal(format!("failed to hash password: {e}")))?;

        self.store.update_user_password(&user.id, &hash)
    }

    /// Generate tokens for an existing user (admin login bypass).
    pub fn generate_tokens_for_user(
        &self,
        user_id: &str,
        email: &str,
    ) -> Result<AuthResponse, NeboError> {
        self.generate_tokens(user_id, email)
    }

    fn generate_tokens(&self, user_id: &str, email: &str) -> Result<AuthResponse, NeboError> {
        let now = chrono::Utc::now().timestamp();
        let access_expiry = now + self.config.auth.access_expire;
        let refresh_expiry = now + self.config.auth.refresh_token_expire;

        // Create access token (JWT)
        let claims = json!({
            "userId": user_id,
            "email": email,
            "iat": now,
            "exp": access_expiry,
        });

        let access_token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(self.config.auth.access_secret.as_bytes()),
        )
        .map_err(|e| NeboError::Internal(format!("failed to sign token: {e}")))?;

        // Create refresh token
        let refresh_token = generate_token();
        let token_hash = hash_token(&refresh_token);
        let token_id = generate_id();

        self.store
            .create_refresh_token(&token_id, user_id, &token_hash, refresh_expiry)?;

        Ok(AuthResponse {
            token: access_token,
            refresh_token,
            expires_at: access_expiry,
        })
    }
}

/// Generate a random 32-byte hex ID.
fn generate_id() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill(&mut bytes);
    hex::encode(bytes)
}

/// Generate a random 64-byte hex token.
fn generate_token() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill(&mut bytes);
    hex::encode(bytes)
}

/// Hash a token for storage (matches Go implementation).
fn hash_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}
