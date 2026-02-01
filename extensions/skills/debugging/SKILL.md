---
name: debugging
description: Systematic debugging and error analysis methodology
version: "1.0.0"
priority: 25
triggers:
  - debug
  - error
  - bug
  - fix
  - crash
  - broken
  - not working
  - issue
  - problem
  - exception
  - stack trace
  - fails
tools:
  - read
  - grep
  - glob
  - bash
metadata:
  nebo:
    emoji: "🐛"
---

# Debugging

When debugging issues, follow this systematic approach:

## 1. Understand the Problem

- What is the expected behavior?
- What is the actual behavior?
- When did it start happening?
- Is it reproducible? Under what conditions?
- What changed recently?

## 2. Gather Information

- Read error messages and stack traces carefully
- Check logs for related errors or warnings
- Identify the exact line/function where failure occurs
- Note any relevant input data or state

## 3. Form Hypotheses

- Based on the error, what could cause this?
- List possible root causes in order of likelihood
- Consider recent changes that might be related

## 4. Test Hypotheses

- Start with the most likely cause
- Make one change at a time
- Verify each hypothesis before moving on
- Add temporary logging if needed

## 5. Common Bug Categories

- Off-by-one errors (loops, arrays)
- Null/nil pointer dereference
- Type mismatches or conversions
- Race conditions (concurrent code)
- Resource leaks (connections, files)
- State management issues
- Edge cases not handled

## 6. Fix and Verify

- Make the minimal fix needed
- Add tests to prevent regression
- Check for similar issues elsewhere
- Document the root cause

## 7. Debugging Tools

- Print/log statements for state inspection
- Debugger breakpoints and stepping
- Binary search through git history (`git bisect`)
- Memory/CPU profilers
- Network traffic inspection

## Example: Nil Return Issue

**User:** "My function returns nil but it shouldn't"

**Step 1: Identify the return paths**

Let me find all return statements in the function:
- Line 45: `return nil, err` - error case
- Line 52: `return result, nil` - success case
- Line 38: Early return without value? (implicit nil)

**Step 2: Check for early returns**

Looking at line 38, there's a guard clause:
```go
if input == nil {
    return nil, nil  // This could be your issue!
}
```

**Hypothesis:** The function is receiving nil input, triggering the early return.

**Fix:** Either validate input at the caller or return an explicit error:
```go
if input == nil {
    return nil, fmt.Errorf("input cannot be nil")
}
```

## Example: Index Out of Range

**User:** "I'm getting a 'panic: runtime error: index out of range'"

**Common Causes:**
1. Empty slice/array being accessed
2. Off-by-one error in loop (using `<=` instead of `<`)
3. Hardcoded index assumption
4. Concurrent modification during iteration

**Prevention Pattern:**
```go
// Safe: Check before access
if idx < len(slice) {
    item := slice[idx]
}

// Safe: Use range for iteration
for i, item := range items {
    // i is always valid
}
```
