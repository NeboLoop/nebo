---
name: code-review
description: Provides structured code review capabilities with focus on best practices
version: "1.0.0"
priority: 10
triggers:
  - review
  - code review
  - critique
  - check my code
  - look at this code
tools:
  - read
  - glob
  - grep
metadata:
  nebo:
    emoji: "🔍"
---

# Code Review

When performing a code review:

1. First read the relevant files using the read tool
2. Analyze for:
   - Logic errors and bugs
   - Security vulnerabilities (OWASP top 10)
   - Performance issues
   - Code style and readability
   - Missing error handling
   - Test coverage gaps

3. Structure your feedback as:
   - **Critical Issues**: Must fix before merge
   - **Suggestions**: Improvements to consider
   - **Positive Notes**: What's done well

4. For each issue, provide:
   - File and line number
   - Description of the problem
   - Suggested fix with code example

## Example

**User:** "Review the authentication code in auth.go"

**Assistant:** I'll review auth.go for security and best practices.

**Critical Issues:**
1. `auth.go:45` - Password comparison uses `==` instead of constant-time comparison
   - Fix: Use `subtle.ConstantTimeCompare()` to prevent timing attacks

**Suggestions:**
1. `auth.go:23` - Consider adding rate limiting for login attempts
2. `auth.go:67` - Token expiry could be configurable

**Positive Notes:**
- Good use of bcrypt for password hashing
- JWT claims are properly validated
