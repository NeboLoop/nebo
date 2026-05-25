# Browser Extension Issues with Slack API Pages

## Issue Summary
The Nebo browser extension fails to properly access and interact with Slack API pages (api.slack.com), consistently returning "Cannot access a chrome:// URL" errors.

## Symptoms
- `read_page` returns 0 lines or fails with chrome:// URL error
- `get_page_text` fails with extraction error
- `evaluate` JavaScript execution fails
- Navigation appears to succeed but page state is inaccessible
- Browser window closes when extension stops using it

## Affected URLs
- https://api.slack.com/apps/A0B4X8963MM/oauth
- https://api.slack.com/apps/A0B4X8963MM

## Error Messages
```
click failed: Cannot access a chrome:// URL. Recovery: Try read_page to see current page state and adjust your approach.
evaluate failed: Cannot attach debugger to chrome:// pages. Navigate to a regular web page first.
get_page_text failed: Failed to extract page text: Cannot access a chrome:// URL.
```

## Workarounds Attempted
1. Force navigation - still returns chrome:// state
2. Reload via JavaScript evaluation - fails
3. Different depth/filter parameters - no improvement
4. Direct navigation to oauth endpoint - same issue

## Impact
Cannot automate Slack app configuration tasks such as:
- Viewing/adding OAuth scopes
- Retrieving bot tokens
- Managing app settings
- Configuring webhook endpoints

## Recommendation
Investigate why Slack API pages are being treated as chrome:// URLs. This may be a security restriction, routing issue, or incompatibility with Slack's authentication flow. Consider:
- Checking if Slack uses special protocols that block extension access
- Testing with other API documentation sites for comparison
- Implementing fallback mechanism for pages the extension cannot access

## Date
May 21, 2026
