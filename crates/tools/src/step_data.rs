//! A workflow step's data, whole, a page at a time.
//!
//! When the data a step is handed (its inputs, its parents' results, its
//! loop item, the `{{...}}` it references) is too large for its model, the
//! step is given a summary of each large piece in its place ([`described`])
//! and reads the piece itself with `read_step_data`: a page at a time, each
//! page sized to the context its model has left, never all at once. A tool
//! result too large to show inline in a workflow step is saved and read the
//! same way (`saved/<file>`, `result_shape::Reader::StepData`). The data
//! itself stays whole in the run: step outputs in its activity results,
//! inputs on its run row.

use std::sync::Arc;

use db::Store;
use serde_json::{Map, Value, json};

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// The tool's name.
pub const TOOL: &str = "read_step_data";

/// The largest page, whatever context is left.
pub const PAGE_MAX_CHARS: usize = 40_000;

/// The smallest page: one that still shows a few items.
pub const PAGE_MIN_CHARS: usize = 2_000;

/// A tool result saved for this step: `saved/<file>`.
pub const SAVED: &str = "saved/";

/// The characters one page may hold: a quarter of the context the step's
/// model has left (`room` tokens, about four characters each), between
/// [`PAGE_MIN_CHARS`] and [`PAGE_MAX_CHARS`]. With no model in the call
/// (`None`), the largest page.
pub fn page_chars(room: Option<usize>) -> usize {
    room.map_or(PAGE_MAX_CHARS, |tokens| tokens.clamp(PAGE_MIN_CHARS, PAGE_MAX_CHARS))
}

/// Where a piece of a run's data lives: a data path (`nodes.<step>…`,
/// `inputs.<name>…`, `saved/<file>…`) and the loop iteration the step runs
/// in (`""` outside a loop; a step's output is looked up there first, then
/// in the iterations around it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Source {
    pub path: String,
    pub at: String,
}

impl Source {
    pub fn new(path: impl Into<String>, at: impl Into<String>) -> Self {
        Self { path: path.into(), at: at.into() }
    }

    /// The call that reads it: `read_step_data {"source": …, "at": …}`.
    pub fn call(&self) -> String {
        let mut args = Map::new();
        args.insert("source".into(), json!(self.path));
        if !self.at.is_empty() {
            args.insert("at".into(), json!(self.at));
        }
        format!("{TOOL} {}", Value::Object(args))
    }
}

/// A step's output as data: its JSON, or its text when it is not JSON.
pub fn parse(text: &str) -> Value {
    serde_json::from_str(text.trim()).unwrap_or_else(|_| Value::String(text.to_string()))
}

/// The characters a value takes in a prompt: a text's own, anything else
/// its JSON.
pub fn chars(value: &Value) -> usize {
    match value {
        Value::String(s) => s.len(),
        other => other.to_string().len(),
    }
}

/// A count with thousands separators: 5,240.
fn n(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// At most `max` bytes of `s`, on a character boundary.
fn head(s: &str, max: usize) -> &str {
    &s[..types::strutil::floor_char_boundary(s, max.min(s.len()))]
}

/// What a value is, in one line: its kind, its size and, for a list of
/// objects, the fields they carry.
fn shape(value: &Value) -> String {
    match value {
        Value::Array(items) => {
            let mut line = format!("A list of {} items ({} characters)", n(items.len()), n(chars(value)));
            let objects = items.iter().take(50).filter(|i| i.is_object()).count();
            if objects > 0 && objects == items.len().min(50) {
                let mut fields: Vec<&str> = Vec::new();
                for item in items.iter().take(50) {
                    for key in item.as_object().into_iter().flat_map(|m| m.keys()) {
                        if !fields.contains(&key.as_str()) {
                            fields.push(key);
                        }
                    }
                }
                let more = if fields.len() > 40 { ", …" } else { "" };
                fields.truncate(40);
                line.push_str(&format!("; each an object with fields: {}{more}", fields.join(", ")));
            } else if items.iter().take(50).all(Value::is_string) && !items.is_empty() {
                line.push_str("; each a text");
            } else if items.iter().take(50).all(Value::is_number) && !items.is_empty() {
                line.push_str("; each a number");
            }
            line + "."
        }
        Value::Object(map) => format!("An object with {} keys ({} characters).", n(map.len()), n(chars(value))),
        Value::String(s) => format!("Text, {} characters, {} lines.", n(s.len()), n(s.lines().count())),
        other => format!("The value {other}."),
    }
}

/// A short description of a value inside another: `list of 5,240 items`,
/// `text, 1,200 characters`, or a small value itself.
fn brief(value: &Value) -> String {
    match value {
        Value::Array(items) => format!("list of {} items", n(items.len())),
        Value::Object(map) => format!("object with {} keys", n(map.len())),
        Value::String(s) if s.len() > 80 => format!("text, {} characters", n(s.len())),
        other => other.to_string(),
    }
}

/// A deterministic summary of a value in at most about `budget`
/// characters: the producer's own `summary` when it has one, what the value
/// is (kind, size, fields or keys), then as many of its first items, keys or
/// characters as fit.
pub fn summary(value: &Value, budget: usize) -> String {
    let mut out = String::new();
    if let Some(own) = value.get("summary").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
        out.push_str(&format!("Its own summary: {}\n", head(own.trim(), budget / 3)));
    }
    out.push_str(&shape(value));
    let room = |out: &String| budget.saturating_sub(out.len());
    match value {
        Value::Array(items) => {
            let mut lines = String::new();
            for (i, item) in items.iter().enumerate() {
                let text = item.to_string();
                let line = if text.len() + 16 <= room(&out).saturating_sub(lines.len()) {
                    format!("\n[{i}] {text}")
                } else if i == 0 {
                    format!("\n[0] {}", brief(item))
                } else {
                    break;
                };
                lines.push_str(&line);
            }
            if !lines.is_empty() {
                out.push_str("\nFirst items:");
                out.push_str(&lines);
            }
        }
        Value::Object(map) => {
            // Each key gets an even share of what is left: a value within it
            // whole, a longer text its beginning, anything else in brief.
            out.push_str("\nKeys:");
            let mut left = map.len();
            for (key, v) in map {
                let share = room(&out) / left.max(1);
                left -= 1;
                let line = match v {
                    _ if v.to_string().len() + key.len() + 6 <= share => format!("\n- {key}: {v}"),
                    Value::String(s) if share > key.len() + 60 => {
                        format!("\n- {key}: {}… ({} characters)", head(s, share - key.len() - 40), n(s.len()))
                    }
                    _ => format!("\n- {key}: {}", brief(v)),
                };
                if line.len() > room(&out) {
                    out.push_str("\n- …");
                    break;
                }
                out.push_str(&line);
            }
        }
        Value::String(s) => {
            let begins = head(s, room(&out).saturating_sub(20));
            if !begins.is_empty() {
                out.push_str("\nIt begins:\n");
                out.push_str(begins);
                if begins.len() < s.len() {
                    out.push('…');
                }
            }
        }
        _ => {}
    }
    out
}

/// What a step is handed in place of a value too large to include whole:
/// its [`summary`] and the call that reads it a page at a time.
pub fn described(value: &Value, source: &Source, budget: usize) -> String {
    format!(
        "{}\nToo large to include whole here: read it a page at a time with {}. Each page ends with the \
         `from` for the next; `where` keeps the list items whose fields equal the values you give.",
        summary(value, budget),
        source.call()
    )
}

/// The workflow run a step's session belongs to.
fn run_of(session_key: &str) -> Option<&str> {
    crate::origin::workflow_run_id(session_key).or_else(|| {
        session_key
            .strip_prefix("workflow:")?
            .split(':')
            .next()
            .filter(|r| !r.is_empty())
    })
}

/// `value` at `segments`: object keys and list positions; a text that holds
/// JSON is read as that JSON.
fn walk(mut value: Value, segments: &[&str], path: &str) -> Result<Value, String> {
    for seg in segments {
        if let Value::String(s) = &value
            && let Ok(parsed @ (Value::Object(_) | Value::Array(_))) = serde_json::from_str::<Value>(s.trim())
        {
            value = parsed;
        }
        value = match value {
            Value::Object(mut map) => map.remove(*seg),
            Value::Array(mut items) => seg.parse::<usize>().ok().filter(|i| *i < items.len()).map(|i| items.swap_remove(i)),
            _ => None,
        }
        .ok_or_else(|| format!("{path}: there is no '{seg}' there"))?;
    }
    Ok(value)
}

/// The value a field path names in an item.
fn field<'v>(item: &'v Value, path: &str) -> Option<&'v Value> {
    path.split('.').try_fold(item, |v, seg| match v {
        Value::Object(map) => map.get(seg),
        Value::Array(items) => items.get(seg.parse::<usize>().ok()?),
        _ => None,
    })
}

/// Whether a field equals what `where` asks for: text without regard to
/// case, anything else exactly (a number given as text matches the number).
fn equal(got: &Value, want: &Value) -> bool {
    match (got, want) {
        (Value::String(a), Value::String(b)) => a.eq_ignore_ascii_case(b),
        (Value::Number(a), Value::String(b)) => a.to_string() == *b,
        _ => got == want,
    }
}

/// One page of `value`, at most `budget` characters, starting at `from`
/// (a list position, a key number, or a character offset), saying where it
/// is and ending with the cursor for the next page.
pub fn page(value: &Value, source: &Source, from: usize, limit: usize, filter: Option<&Map<String, Value>>, budget: usize) -> Result<String, String> {
    let label = &source.path;
    let filter = filter.filter(|f| !f.is_empty());
    let child = |key: &str| Source::new(format!("{}.{key}", source.path), source.at.clone());
    let line_for = |key: &str, item: &Value, prefix: String| {
        let text = item.to_string();
        if text.len() > budget / 2 {
            format!("{prefix}{} Read it with {}", shape(item), child(key).call())
        } else {
            format!("{prefix}{text}")
        }
    };
    match value {
        Value::Array(items) => {
            let total = items.len();
            let keep = |item: &Value| filter.is_none_or(|f| f.iter().all(|(k, want)| field(item, k).is_some_and(|got| equal(got, want))));
            let matching = if filter.is_some() { items.iter().filter(|i| keep(i)).count() } else { total };
            let before = if filter.is_some() { items.iter().take(from).filter(|i| keep(i)).count() } else { from.min(total) };
            let mut lines = Vec::new();
            let mut used = 0;
            let mut at = from;
            while at < total && lines.len() < limit {
                if !keep(&items[at]) {
                    at += 1;
                    continue;
                }
                let line = line_for(&at.to_string(), &items[at], format!("[{at}] "));
                if used + line.len() + 1 > budget && !lines.is_empty() {
                    break;
                }
                used += line.len() + 1;
                lines.push(line);
                at += 1;
            }
            let shown = lines.len();
            let mut out = match filter {
                None if shown == 0 => return Ok(format!("{label}: a list of {} items; nothing from {from}.", n(total))),
                None => format!("{label}: a list of {} items. Items {}–{} of {}:\n", n(total), n(from), n(at - 1), n(total)),
                Some(f) if shown == 0 => {
                    return Ok(format!("{label}: a list of {} items; {} match {}, none from {from}.", n(total), n(matching), Value::Object(f.clone())));
                }
                Some(f) => format!(
                    "{label}: a list of {} items; {} match {}. Matching items {}–{} of {} (positions up to {}):\n",
                    n(total),
                    n(matching),
                    Value::Object(f.clone()),
                    n(before + 1),
                    n(before + shown),
                    n(matching),
                    n(at - 1)
                ),
            };
            out.push_str(&lines.join("\n"));
            if before + shown < matching {
                out.push_str(&format!("\nNext page: from=\"{at}\""));
            } else {
                out.push_str("\nEnd of the list.");
            }
            Ok(out)
        }
        Value::Object(map) => {
            if filter.is_some() {
                return Err(format!(
                    "{label} is an object, and `where` filters a list. Read one of its fields: {}",
                    child("<key>").call()
                ));
            }
            let total = map.len();
            let mut lines = Vec::new();
            let mut used = 0;
            let mut at = from;
            for (key, v) in map.iter().skip(from) {
                if lines.len() >= limit {
                    break;
                }
                let line = line_for(key, v, format!("{}: ", json!(key)));
                if used + line.len() + 1 > budget && !lines.is_empty() {
                    break;
                }
                used += line.len() + 1;
                lines.push(line);
                at += 1;
            }
            if lines.is_empty() {
                return Ok(format!("{label}: an object with {} keys; nothing from {from}.", n(total)));
            }
            let mut out = format!("{label}: an object with {} keys. Keys {}–{} of {}:\n", n(total), n(from), n(at - 1), n(total));
            out.push_str(&lines.join("\n"));
            out.push_str(&if at < total { format!("\nNext page: from=\"{at}\"") } else { "\nEnd of the object.".to_string() });
            Ok(out)
        }
        Value::String(s) => {
            if filter.is_some() {
                return Err(format!("{label} is text, and `where` filters a list."));
            }
            let total = s.chars().count();
            let text: String = s.chars().skip(from).take(budget).collect();
            if text.is_empty() {
                return Ok(format!("{label}: text of {} characters; nothing from {from}.", n(total)));
            }
            // Cut at a line end when one falls in the page's second half.
            let mut taken = text.chars().count();
            let mut body = text.as_str();
            if from + taken < total
                && let Some(nl) = text.rfind('\n').filter(|&nl| nl > text.len() / 2)
            {
                body = &text[..=nl];
                taken = body.chars().count();
            }
            let end = from + taken;
            let mut out = format!("{label}: text of {} characters. Characters {}–{} of {}:\n{body}", n(total), n(from), n(end - 1), n(total));
            out.push_str(&if end < total { format!("\nNext page: from=\"{end}\"") } else { "\nEnd of the text.".to_string() });
            Ok(out)
        }
        other => Ok(format!("{label}: {other}")),
    }
}

/// `read_step_data`: a workflow step's data, a page at a time.
pub struct ReadStepDataTool {
    store: Arc<Store>,
}

impl ReadStepDataTool {
    pub fn new(store: Arc<Store>) -> Self {
        Self { store }
    }

    /// The value `source` names in this step's run (or among the results
    /// saved for this step).
    fn load(&self, ctx: &ToolContext, source: &Source) -> Result<Value, String> {
        let path = source.path.trim();
        if let Some(rest) = path.strip_prefix(SAVED) {
            let (stem, inner) = rest.split_once('.').unwrap_or((rest, ""));
            if stem.is_empty() || !stem.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Err(format!("{path}: saved/ takes the file a saved result names, like saved/3f2c…"));
            }
            let file = crate::result_shape::results_dir(&ctx.session_id).join(format!("{stem}.txt"));
            let text = std::fs::read_to_string(&file).map_err(|_| format!("{path}: nothing was saved under that name for this step"))?;
            let segments: Vec<&str> = inner.split('.').filter(|s| !s.is_empty()).collect();
            return walk(parse(&text), &segments, path);
        }
        let Some(run) = run_of(&ctx.session_key) else {
            return Err(format!("{TOOL} reads a workflow step's data; this is not a workflow step."));
        };
        let segments: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
        match segments.as_slice() {
            ["nodes", id, rest @ ..] => {
                let contents = self.store.completed_activity_contents(run).map_err(|e| e.to_string())?;
                // The iteration the step runs in, then the ones around it.
                let mut at = source.at.as_str();
                let content = loop {
                    if let Some(c) = contents.get(&(id.to_string(), at.to_string())) {
                        break c;
                    }
                    if at.is_empty() {
                        return Err(format!("{path}: step '{id}' has no finished output in this run"));
                    }
                    at = at.rsplit_once('.').map_or("", |(outer, _)| outer);
                };
                walk(parse(content), rest, path)
            }
            ["inputs", rest @ ..] => {
                let inputs = self
                    .store
                    .get_workflow_run(run)
                    .map_err(|e| e.to_string())?
                    .and_then(|r| r.inputs)
                    .map(|i| parse(&i))
                    .unwrap_or(Value::Null);
                walk(inputs, rest, path)
            }
            _ => Err(format!("{path}: a source starts with nodes.<step>, inputs.<name> or saved/<file>")),
        }
    }
}

impl DynTool for ReadStepDataTool {
    fn name(&self) -> &str {
        TOOL
    }

    fn description(&self) -> String {
        "Reads a workflow step's data a page at a time: an input, an earlier step's result, a loop item, \
         or a tool result too large to show, whichever this step was told is too large to include whole.\n\
         - Each page fits what is left of your context, says where it is (items 200–399 of 5,240) and ends \
           with the `from` for the next page. Asking for everything returns the first page and that cursor.\n\
         - A path reads one part: `nodes.fetch.records.12`, `nodes.fetch.summary`.\n\
         - `where` keeps the list items whose fields equal the values given, e.g. {\"status\": \"open\"}."
            .to_string()
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "source": { "type": "string", "description": "What to read, as the step was told: nodes.<step>[.<field>…], inputs.<name>[…], or saved/<file>[…]." },
                "at": { "type": "string", "description": "The loop iteration, when the step was told one (\"2\", \"2.0\")." },
                "from": { "type": "string", "description": "Where the page starts: the cursor the last page ended with. Default the beginning." },
                "limit": { "type": "integer", "description": "Most list items or keys on the page; a page never runs past its size either way." },
                "where": { "type": "object", "description": "Only list items whose fields equal these values; a field may be a path (owner.name)." }
            },
            "required": ["source"]
        })
    }

    fn search_hint(&self) -> &str {
        "read a workflow step's large data in pages"
    }

    fn read_only(&self, _input: &Value) -> bool {
        true
    }

    /// It pages itself: a page is never saved and previewed.
    fn max_result_chars(&self, _input: &Value) -> Option<usize> {
        None
    }

    fn describe(&self, _input: &Value) -> String {
        "reading the step's data".to_string()
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let source = Source::new(
                input["source"].as_str().unwrap_or("").trim(),
                input["at"].as_str().unwrap_or("").trim(),
            );
            let from = match &input["from"] {
                Value::Null => 0,
                Value::Number(v) => v.as_u64().unwrap_or(0) as usize,
                Value::String(s) if s.trim().is_empty() => 0,
                Value::String(s) => match s.trim().parse() {
                    Ok(v) => v,
                    Err(_) => return ToolResult::error(format!("from is the cursor a page ended with, a number; got {s:?}")),
                },
                other => return ToolResult::error(format!("from is the cursor a page ended with, a number; got {other}")),
            };
            let limit = input["limit"].as_u64().map_or(usize::MAX, |l| l.max(1) as usize);
            let value = match self.load(ctx, &source) {
                Ok(v) => v,
                Err(e) => return ToolResult::error(e),
            };
            match page(&value, &source, from, limit, input["where"].as_object(), page_chars(ctx.context_room)) {
                Ok(text) => ToolResult::ok(text),
                Err(e) => ToolResult::error(e),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(count: usize) -> Value {
        Value::Array(
            (0..count)
                .map(|i| json!({"id": i, "status": if i % 3 == 0 { "open" } else { "closed" }, "note": "x".repeat(300)}))
                .collect(),
        )
    }

    #[test]
    fn a_json_list_is_summarized_by_kind_size_fields_and_first_items() {
        let list = json!([{"id": 1, "name": "Ava"}, {"id": 2, "name": "Bo", "tag": "vip"}, {"id": 3, "name": "Cy"}]);
        let s = summary(&list, 1_500);
        assert!(s.starts_with(&format!("A list of 3 items ({} characters); each an object with fields: id, name, tag.", list.to_string().len())), "{s}");
        assert!(s.contains("First items:\n[0] {\"id\":1,\"name\":\"Ava\"}\n[1] {\"id\":2,\"name\":\"Bo\",\"tag\":\"vip\"}"), "{s}");

        let big = rows(5_240);
        let s = summary(&big, 1_500);
        assert!(s.starts_with("A list of 5,240 items ("), "{s}");
        assert!(s.contains("fields: id, note, status."), "{s}");
        assert!(s.contains("[0] {") && !s.contains("[5239]"), "{s}");
        assert!(s.len() <= 1_500, "{}", s.len());

        let own = json!({"summary": "5240 rows fetched", "records": big});
        let s = summary(&own, 1_500);
        assert!(s.starts_with("Its own summary: 5240 rows fetched\nAn object with 2 keys"), "{s}");
        assert!(s.contains("- records: list of 5,240 items"), "{s}");
    }

    /// A 2 MB list read page by page: every item once, in order, and no page
    /// over the page bound.
    #[test]
    fn a_two_megabyte_list_reads_page_by_page_within_the_bound() {
        let list = rows(6_000);
        assert!(list.to_string().len() >= 2_000_000, "{}", list.to_string().len());
        let source = Source::new("nodes.fetch", "");
        let budget = page_chars(Some(30_000));
        assert_eq!(budget, 30_000);
        let mut from = 0;
        let mut seen = 0;
        let mut pages = 0;
        loop {
            let text = page(&list, &source, from, usize::MAX, None, budget).unwrap();
            pages += 1;
            assert!(text.len() <= budget + 300, "page {pages} is {} chars", text.len());
            seen += text.lines().filter(|l| l.starts_with('[')).count();
            match text.rsplit_once("Next page: from=\"") {
                Some((_, rest)) => from = rest.trim_end_matches('"').parse().unwrap(),
                None => break,
            }
        }
        assert_eq!(seen, 6_000);
        assert!(pages > 60, "{pages}");
    }

    /// Asking for everything returns the first page and the cursor.
    #[test]
    fn everything_is_the_first_page_and_a_cursor() {
        let list = rows(6_000);
        let text = page(&list, &Source::new("inputs.rows", ""), 0, usize::MAX, None, PAGE_MAX_CHARS).unwrap();
        assert!(text.starts_with("inputs.rows: a list of 6,000 items. Items 0–"), "{}", &text[..100]);
        assert!(text.contains("\n[0] {"));
        let (_, cursor) = text.rsplit_once("Next page: from=\"").expect("a cursor");
        let next: usize = cursor.trim_end_matches('"').parse().unwrap();
        assert!(next > 0 && next < 6_000, "{next}");
        assert!(text.len() <= PAGE_MAX_CHARS + 300);
    }

    #[test]
    fn where_filters_and_the_page_says_where_it_is() {
        let list = rows(30);
        let filter = json!({"status": "OPEN"});
        let text = page(&list, &Source::new("nodes.fetch", "2"), 0, 4, filter.as_object(), PAGE_MAX_CHARS).unwrap();
        assert!(text.contains("10 match {\"status\":\"OPEN\"}. Matching items 1–4 of 10"), "{text}");
        assert!(text.contains("[0] ") && text.contains("[9] ") && !text.contains("[1] "), "{text}");
        assert!(text.ends_with("Next page: from=\"10\""), "{text}");
        let text = page(&list, &Source::new("nodes.fetch", "2"), 10, 100, filter.as_object(), PAGE_MAX_CHARS).unwrap();
        assert!(text.contains("Matching items 5–10 of 10") && text.ends_with("End of the list."), "{text}");
    }

    #[test]
    fn text_and_an_item_too_large_for_a_page_are_paged_too() {
        let text = Value::String("line\n".repeat(10_000));
        let out = page(&text, &Source::new("nodes.read", ""), 0, usize::MAX, None, PAGE_MIN_CHARS).unwrap();
        assert!(out.starts_with("nodes.read: text of 50,000 characters. Characters 0–1,999 of 50,000:"), "{}", &out[..80]);
        assert!(out.ends_with("Next page: from=\"2000\""), "{}", &out[out.len() - 40..]);

        let nested = json!([{"blob": "y".repeat(10_000)}, 2]);
        let out = page(&nested, &Source::new("nodes.read", "1"), 0, usize::MAX, None, PAGE_MIN_CHARS).unwrap();
        assert!(out.contains("[0] An object with 1 keys") && out.contains("Read it with read_step_data {\"at\":\"1\",\"source\":\"nodes.read.0\"}"), "{out}");
        assert!(out.len() < PAGE_MIN_CHARS, "{}", out.len());
    }

    #[test]
    fn a_page_is_a_quarter_of_the_room_left_within_bounds() {
        assert_eq!(page_chars(None), PAGE_MAX_CHARS);
        assert_eq!(page_chars(Some(10)), PAGE_MIN_CHARS);
        assert_eq!(page_chars(Some(12_000)), 12_000);
        assert_eq!(page_chars(Some(900_000)), PAGE_MAX_CHARS);
    }

    #[tokio::test]
    async fn the_tool_reads_a_runs_outputs_and_inputs_and_only_its_own_run() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("s.db").to_string_lossy()).unwrap());
        let inputs = json!({"rows": rows(500)}).to_string();
        store.create_workflow_run("run-1", "wf", "manual", None, Some(&inputs), None, None).unwrap();
        store.create_activity_result("run-1", "fetch", "", "completed", 0, 1, None, 0, Some(0)).unwrap();
        store.set_activity_result_content("run-1", "fetch", "", &json!({"records": rows(40)}).to_string()).unwrap();
        let tool = ReadStepDataTool::new(store);
        let ctx = ToolContext { session_key: "agent:ava:workflow:run-1:digest:2".into(), context_room: Some(8_000), ..Default::default() };

        let out = tool.execute_dyn(&ctx, json!({"source": "nodes.fetch.records", "at": "2", "where": {"status": "open"}, "limit": 2})).await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.starts_with("nodes.fetch.records: a list of 40 items; 14 match"), "{}", out.content);

        let out = tool.execute_dyn(&ctx, json!({"source": "inputs.rows", "from": "490"})).await;
        assert!(out.content.contains("Items 490–499 of 500") && out.content.ends_with("End of the list."), "{}", out.content);

        let out = tool.execute_dyn(&ctx, json!({"source": "inputs.rows.7.id"})).await;
        assert_eq!(out.content, "inputs.rows.7.id: 7");

        let other = ToolContext { session_key: "agent:ava:workflow:run-2:digest:".into(), ..Default::default() };
        let out = tool.execute_dyn(&other, json!({"source": "nodes.fetch"})).await;
        assert!(out.is_error && out.content.contains("no finished output in this run"), "{}", out.content);

        let chat = ToolContext { session_key: "agent:ava:main".into(), ..Default::default() };
        assert!(tool.execute_dyn(&chat, json!({"source": "nodes.fetch"})).await.is_error);
    }
}
