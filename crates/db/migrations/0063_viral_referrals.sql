-- +goose Up
-- Viral Referral System for Nebo Launch
-- Target: 1M users in 30 days via unlimited 3M/1M token referral rewards

-- =============================================================================
-- REFERRAL CODES
-- Each user gets a unique referral code they can share
-- =============================================================================

CREATE TABLE IF NOT EXISTS referral_codes (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    code TEXT NOT NULL UNIQUE,  -- Short code like "ALAN42" or random "g7k2m9"
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_referral_codes_user_id ON referral_codes(user_id);
CREATE INDEX IF NOT EXISTS idx_referral_codes_code ON referral_codes(code);

-- =============================================================================
-- REFERRAL SIGNUPS
-- Tracks when someone signs up via a referral link
-- =============================================================================

CREATE TABLE IF NOT EXISTS referral_signups (
    id TEXT PRIMARY KEY,
    referrer_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,  -- Person who shared
    referee_user_id TEXT REFERENCES users(id) ON DELETE CASCADE,  -- Person who signed up (NULL until they create account)
    referee_email TEXT,  -- Email they used to sign up (before user_id exists)
    referral_code TEXT NOT NULL REFERENCES referral_codes(code),
    
    -- Token rewards
    referrer_tokens_awarded INTEGER NOT NULL DEFAULT 3000000,  -- 3M tokens
    referee_tokens_awarded INTEGER NOT NULL DEFAULT 1000000,   -- 1M tokens
    tokens_expire_at INTEGER NOT NULL,  -- 90 days from signup
    
    -- Status tracking
    status TEXT NOT NULL DEFAULT 'pending',  -- pending → signed_up → activated → expired
    activated_at INTEGER,  -- When referee ran first 10 queries or paid
    
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_referral_signups_referrer ON referral_signups(referrer_user_id);
CREATE INDEX IF NOT EXISTS idx_referral_signups_referee ON referral_signups(referee_user_id);
CREATE INDEX IF NOT EXISTS idx_referral_signups_email ON referral_signups(referee_email);
CREATE INDEX IF NOT EXISTS idx_referral_signups_status ON referral_signups(status);
CREATE INDEX IF NOT EXISTS idx_referral_signups_expires ON referral_signups(tokens_expire_at);

-- =============================================================================
-- TOKEN BALANCES
-- Tracks bonus token credits from referrals
-- Checked by Janus before charging for usage
-- =============================================================================

CREATE TABLE IF NOT EXISTS token_balances (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    
    -- Balance tracking
    balance INTEGER NOT NULL DEFAULT 0,  -- Current available bonus tokens
    total_earned INTEGER NOT NULL DEFAULT 0,  -- Lifetime total from referrals
    total_used INTEGER NOT NULL DEFAULT 0,  -- How many bonus tokens have been spent
    
    -- Expiry tracking (multiple batches with different expiry dates)
    expires_at INTEGER,  -- Next expiry date for this user's oldest token batch
    
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_token_balances_user_id ON token_balances(user_id);
CREATE INDEX IF NOT EXISTS idx_token_balances_expires ON token_balances(expires_at);

-- =============================================================================
-- TOKEN TRANSACTIONS
-- Ledger of all token awards and spending
-- =============================================================================

CREATE TABLE IF NOT EXISTS token_transactions (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    
    -- Transaction details
    amount INTEGER NOT NULL,  -- Positive for credits, negative for debits
    type TEXT NOT NULL,  -- 'referral_reward', 'referral_bonus', 'usage', 'expiry'
    
    -- Source tracking
    referral_signup_id TEXT REFERENCES referral_signups(id),  -- If from referral
    description TEXT,  -- Human-readable description
    
    -- Balance after this transaction
    balance_after INTEGER NOT NULL,
    
    expires_at INTEGER,  -- When these tokens expire (for credits only)
    
    created_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_token_transactions_user_id ON token_transactions(user_id);
CREATE INDEX IF NOT EXISTS idx_token_transactions_type ON token_transactions(type);
CREATE INDEX IF NOT EXISTS idx_token_transactions_created ON token_transactions(created_at);

-- =============================================================================
-- REFERRAL LEADERBOARD (Materialized for performance)
-- Updated by trigger or cron job
-- =============================================================================

CREATE TABLE IF NOT EXISTS referral_leaderboard (
    user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    user_name TEXT NOT NULL,
    
    -- Stats
    total_referrals INTEGER NOT NULL DEFAULT 0,  -- Total signups
    activated_referrals INTEGER NOT NULL DEFAULT 0,  -- Signups that activated
    total_tokens_earned INTEGER NOT NULL DEFAULT 0,  -- Total tokens from referrals
    
    -- Ranking
    rank INTEGER,
    
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_referral_leaderboard_rank ON referral_leaderboard(rank);
CREATE INDEX IF NOT EXISTS idx_referral_leaderboard_referrals ON referral_leaderboard(total_referrals DESC);

-- +goose Down
DROP TABLE IF EXISTS referral_leaderboard;
DROP TABLE IF EXISTS token_transactions;
DROP TABLE IF EXISTS token_balances;
DROP TABLE IF EXISTS referral_signups;
DROP TABLE IF EXISTS referral_codes;
