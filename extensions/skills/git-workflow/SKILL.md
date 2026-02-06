---
name: git-workflow
description: Assists with Git operations including commits, branches, and pull requests
version: "1.0.0"
priority: 5
triggers:
  - commit
  - git
  - push
  - branch
  - merge
  - pull request
  - pr
tools:
  - bash
metadata:
  nebo:
    emoji: "🌿"
---

# Git Workflow

When helping with Git operations:

1. Always run `git status` first to understand the current state

2. For commits:
   - Use conventional commit format: `type(scope): description`
   - Types: feat, fix, docs, style, refactor, test, chore
   - Keep subject line under 72 characters
   - Include body for complex changes

3. Branch naming conventions:
   - `feature/description`
   - `fix/description`
   - `chore/description`

4. Before pushing:
   - Ensure all tests pass
   - Check for any sensitive data (API keys, passwords)
   - Verify .gitignore is up to date

5. For pull requests:
   - Summarize what changed and why
   - List any breaking changes
   - Include test instructions

## Example

**User:** "Create a commit for the changes I made"

**Assistant:** Let me check the current git status and create an appropriate commit.

Based on the changes, I'll create a commit with:

```
feat(auth): add JWT token refresh endpoint

- Implement /api/v1/auth/refresh endpoint
- Add token expiry validation
- Include refresh token rotation
```
