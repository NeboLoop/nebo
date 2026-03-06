# Bug Report: Browser Extension Cannot Read Page Content

## Problem Description
The Nebo browser extension can successfully navigate to URLs, but fails when attempting to read page content using `web(action: "read_page")`. The extension shows as connected in the status, but returns a serialization error when trying to execute JavaScript on the page.

## Error Message
```
Failed to read page: Error in invocation of scripting.executeScript(scripting.ScriptInjection injection, optional function callback): Error at parameter 'injection': Error at property 'args': Error at index 3: Value is unserializable.
```

## Symptoms
- `web(action: "navigate")` works correctly
- Browser shows as connected (`web(action: "status")` returns connected: true)
- `web(action: "read_page")` fails with unserializable error
- `web(action: "screenshot")` fails with "Extension disconnected"
- Navigation succeeds but content extraction fails

## Expected Behavior
The extension should be able to:
1. Navigate to any URL
2. Read the page accessibility tree with element refs
3. Extract interactive elements
4. Take screenshots of the page

## Actual Behavior
1. Navigation works ✓
2. Page reading fails ✗
3. Screenshot fails ✗
4. JavaScript execution fails ✗

## Technical Details
The error occurs in `scripting.executeScript()` when trying to inject the page reading function. The "Value is unserializable" error at index 3 suggests that one of the arguments being passed to the script is not serializable - possibly:
- A circular reference in the injection object
- An unsupported data type in the args array
- A function or object that can't be serialized to the extension context

## Reproduction Steps
1. Call `web(action: "navigate", url: "https://example.com")` - succeeds
2. Call `web(action: "read_page")` - fails with serialization error
3. Call `web(action: "read_page", filter: "interactive")` - fails with same error
4. Call `web(action: "screenshot")` - fails with "Extension disconnected"

## Environment
- Browser: Brave (also affects Chrome)
- Extension: Nebo browser extension
- Platform: macOS (aarch64)
- Date: March 5, 2026

## Impact
This is a **critical blocker** - the browser automation feature is non-functional for any task that requires reading page content. This includes:
- Reading emails
- Searching and extracting search results
- Form filling and interaction
- Data extraction from websites
- Any workflow requiring page content access

## Required Fix
The extension needs to:
1. Fix the serialization issue in `scripting.executeScript()` calls
2. Ensure all arguments passed to injected scripts are serializable
3. Implement proper error handling for failed script injections
4. Add diagnostic logging to identify which argument is failing serialization

## Workarounds (Temporary)
- Use `web(action: "fetch")` for simple HTTP requests (won't work for JavaScript-heavy sites)
- Manual navigation by user
- Use alternative APIs or tools for specific tasks

## Priority
**CRITICAL** - This blocks all browser automation functionality

---
*Reported by: Nebo AI Assistant*
*Date: March 5, 2026*
