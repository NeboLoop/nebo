# A2UI update_data_model overwrites entire data model on pathed updates

## Location
`crates/server/src/a2ui.rs` — `update_data_model()`, line ~647

## Bug
When `update_data_model` is called with a `path` (e.g. `/contacts/active`), the in-memory state and DB persistence store the partial `value` as the **entire** data model, discarding all other paths.

```rust
// Line 647 — replaces entire model instead of merging at path
if let Some(state) = surfaces.get_mut(surface_id) {
    state.data_model = Some(value.clone());
}
```

The WebSocket broadcast IS correct — it uses `UpdateDataModelBuilder` with the path, so the frontend receives a properly scoped update. But:

1. On page refresh / surface reload, the DB-persisted model is wrong (only contains the last pathed update's value, not the full model)
2. Sequential pathed updates clobber each other — updating `/contacts/active` then `/contacts/dormant` leaves only dormant data in the model

## Expected behavior
Pathed updates should merge into the existing data model at the specified JSON Pointer path, not replace the root.

## Secondary issue: string double-serialization
The `value` parameter in the A2UI tool schema is typed as `"type": "string"`. When the agent passes a JSON array/object as the value, it arrives as a `serde_json::Value::String(...)` wrapping the JSON text, rather than the deserialized structure. This results in the data model containing a JSON string literal instead of the actual data.

The tool should attempt `serde_json::from_str()` on string values to unwrap embedded JSON before storing.

## Impact
- Agent workspace views that use `data_bindings` with pathed updates (like the outreach-coach contacts view) render empty because the data model structure doesn't match what components expect
- The GenericBinder subscribes to paths like `/contacts/active` but finds either nothing (wrong root) or a string (double-serialized)

## Reproduction
1. Create a surface with a data model like `{"contacts": {"active": [], "in_window": [], "dormant": []}}`
2. Call `update_data_model` with `path: "/contacts/active"` and `value: [{"name": "Test"}]`
3. Check DB: `SELECT data_model FROM a2ui_surfaces WHERE ...` — the entire model is now just `[{"name": "Test"}]`, the `/contacts` wrapper and sibling paths are gone

## Fix
1. Load existing data model, parse path as JSON Pointer, merge value at that location, store merged result
2. Add `serde_json::from_str()` fallback on string values to handle JSON passed through the string-typed MCP parameter
