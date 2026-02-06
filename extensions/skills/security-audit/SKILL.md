---
name: security-audit
description: Security-focused code analysis using OWASP guidelines
version: "1.0.0"
priority: 20
triggers:
  - security
  - audit
  - vulnerability
  - owasp
  - pen test
  - penetration test
  - security review
  - secure code
tools:
  - read
  - grep
  - glob
metadata:
  nebo:
    emoji: "🔐"
---

# Security Audit

When performing a security audit:

## 1. OWASP Top 10 Checklist

- A01:2021 Broken Access Control
- A02:2021 Cryptographic Failures
- A03:2021 Injection (SQL, Command, LDAP, XSS)
- A04:2021 Insecure Design
- A05:2021 Security Misconfiguration
- A06:2021 Vulnerable Components
- A07:2021 Authentication Failures
- A08:2021 Software/Data Integrity Failures
- A09:2021 Security Logging Failures
- A10:2021 Server-Side Request Forgery (SSRF)

## 2. Common Security Issues

Look for:
- Hardcoded secrets, API keys, passwords, tokens
- Unvalidated/unsanitized user input
- Missing authentication or authorization checks
- Insecure cryptography (weak algorithms, hardcoded keys)
- Sensitive data exposure in logs or errors
- Path traversal vulnerabilities
- Unsafe deserialization
- Missing security headers
- CORS misconfigurations

## 3. Severity Levels

- **CRITICAL**: Immediately exploitable, high impact
- **HIGH**: Serious vulnerability, requires specific conditions
- **MEDIUM**: Should be fixed, moderate risk
- **LOW**: Best practice improvement, minimal risk
- **INFO**: Informational finding

## 4. Report Format

For each finding include:
- Location (file:line)
- Description of the vulnerability
- Potential impact
- Recommended fix with code example

## Example

**User:** "Check this authentication code for security issues"

**Assistant:** I'll perform a security audit focusing on authentication vulnerabilities.

**CRITICAL:**
- `auth/login.go:45` - SQL injection via string concatenation
  ```go
  // Vulnerable
  query := "SELECT * FROM users WHERE email = '" + email + "'"
  // Fix: Use parameterized queries
  query := "SELECT * FROM users WHERE email = ?"
  ```

**HIGH:**
- `config/secrets.go:12` - Hardcoded API key
  ```go
  // Vulnerable
  apiKey := "sk-live-abc123"
  // Fix: Use environment variables
  apiKey := os.Getenv("API_KEY")
  ```

**MEDIUM:**
- `auth/session.go:78` - Missing secure flag on session cookie
  ```go
  // Fix: Add Secure and HttpOnly flags
  http.SetCookie(w, &http.Cookie{
      Name:     "session",
      Value:    token,
      Secure:   true,
      HttpOnly: true,
      SameSite: http.SameSiteStrictMode,
  })
  ```
