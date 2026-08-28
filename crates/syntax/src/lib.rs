//! nebo-syntax — in-process tree-sitter runtime with compiled-in grammars.
//!
//! The code-intelligence spine (PRD_CODING_HARNESS Pillar 2): zero external
//! processes, works on every customer machine, offline. Consumed by the `code`
//! STRAP tool (outline/symbols/parse_check/query/context) and by the os file
//! tool's edit-verification chain and outline-first reads.
//!
//! Tier-1 grammars only: rust, typescript(+tsx), javascript, python, go,
//! json, yaml, toml, bash, html, css, markdown.

use std::path::Path;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, Tree};

/// Hard cap on collected syntax errors — a minified or mangled file can hold
/// thousands of ERROR nodes; past this point more entries add noise, not signal.
const MAX_ERRORS: usize = 50;

/// Hard cap on collected query matches (memory safety; presentation caps are
/// the caller's job — see the `code` tool's "N more omitted" rendering).
const MAX_QUERY_MATCHES: usize = 2000;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("parser failed to produce a tree")]
    Parse,
    #[error("invalid tree-sitter query: {0}")]
    Query(String),
}

/// A tier-1 language with a compiled-in grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Go,
    Json,
    Yaml,
    Toml,
    Bash,
    Html,
    Css,
    Markdown,
}

impl Lang {
    /// Detect the language from a file path's extension. `None` means no
    /// compiled-in grammar — callers must treat that as absence, not error.
    pub fn from_path(path: &Path) -> Option<Lang> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "rs" => Some(Lang::Rust),
            "ts" | "mts" | "cts" => Some(Lang::TypeScript),
            "tsx" => Some(Lang::Tsx),
            "js" | "mjs" | "cjs" | "jsx" => Some(Lang::JavaScript),
            "py" => Some(Lang::Python),
            "go" => Some(Lang::Go),
            "json" => Some(Lang::Json),
            "yaml" | "yml" => Some(Lang::Yaml),
            "toml" => Some(Lang::Toml),
            "sh" | "bash" => Some(Lang::Bash),
            "html" | "htm" => Some(Lang::Html),
            "css" => Some(Lang::Css),
            "md" | "markdown" => Some(Lang::Markdown),
            _ => None,
        }
    }

    /// Human-facing language name, as it appears in tool results
    /// (e.g. "syntax OK (rust)").
    pub fn name(&self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::TypeScript => "typescript",
            Lang::Tsx => "tsx",
            Lang::JavaScript => "javascript",
            Lang::Python => "python",
            Lang::Go => "go",
            Lang::Json => "json",
            Lang::Yaml => "yaml",
            Lang::Toml => "toml",
            Lang::Bash => "bash",
            Lang::Html => "html",
            Lang::Css => "css",
            Lang::Markdown => "markdown",
        }
    }

    fn language(&self) -> Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Lang::Python => tree_sitter_python::LANGUAGE.into(),
            Lang::Go => tree_sitter_go::LANGUAGE.into(),
            Lang::Json => tree_sitter_json::LANGUAGE.into(),
            Lang::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Lang::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Lang::Bash => tree_sitter_bash::LANGUAGE.into(),
            Lang::Html => tree_sitter_html::LANGUAGE.into(),
            Lang::Css => tree_sitter_css::LANGUAGE.into(),
            Lang::Markdown => tree_sitter_md::LANGUAGE.into(),
        }
    }

    /// Descent cap for data/markup formats, where deep nesting is noise for an
    /// outline (a JSON outline is its top-level keys, not every leaf). `None`
    /// means unlimited (code languages nest meaningfully: impl → method).
    fn max_outline_depth(&self) -> Option<usize> {
        match self {
            Lang::Json | Lang::Toml => Some(2),
            Lang::Html => Some(3),
            Lang::Yaml => Some(4),
            _ => None,
        }
    }
}

/// One outline symbol. Lines are 1-based and inclusive; nesting mirrors the
/// source (methods inside an impl/class appear as `children`).
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: &'static str,
    pub start_line: usize,
    pub end_line: usize,
    pub children: Vec<Symbol>,
}

/// One syntax error from `parse_check`. Line/col are 1-based.
#[derive(Debug, Clone)]
pub struct SyntaxError {
    pub line: usize,
    pub col: usize,
    /// e.g. `unexpected ')'` or `missing '}'` — factual, tree-sitter-derived.
    pub message: String,
    /// The trimmed source line the error sits on, capped short.
    pub excerpt: String,
}

/// One capture from a structural `query`. Line is 1-based.
#[derive(Debug, Clone)]
pub struct QueryHit {
    /// The capture name from the query (without `@`).
    pub capture: String,
    /// The captured node's grammar kind.
    pub kind: String,
    pub line: usize,
    /// First line of the captured node's text, capped short.
    pub text: String,
}

fn parse(source: &str, lang: Lang) -> Result<Tree, Error> {
    let mut parser = Parser::new();
    parser.set_language(&lang.language()).map_err(|_| Error::Parse)?;
    parser.parse(source, None).ok_or(Error::Parse)
}

/// Slice a node's source text without panicking on odd byte ranges.
fn node_text<'a>(node: Node, source: &'a str) -> &'a str {
    source.get(node.byte_range()).unwrap_or("")
}

/// First line of `s`, trimmed, capped at `max` chars.
fn one_line(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    if line.chars().count() > max {
        let cut: String = line.chars().take(max).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

fn field_text(node: Node, field: &str, source: &str) -> Option<String> {
    node.child_by_field_name(field)
        .map(|n| one_line(node_text(n, source), 120))
}

fn child_text_by_kind(node: Node, kind: &str, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    let found = node.named_children(&mut cursor).find(|c| c.kind() == kind);
    found.map(|n| one_line(node_text(n, source), 120))
}

fn first_named_child_text(node: Node, source: &str) -> Option<String> {
    node.named_child(0).map(|n| one_line(node_text(n, source), 120))
}

/// Classify a node as an outline symbol: `Some((kind, name))` or `None`.
/// ONE table for all tier-1 languages — the walk engine is shared.
fn symbol_for(lang: Lang, node: Node, source: &str) -> Option<(&'static str, String)> {
    match lang {
        Lang::Rust => match node.kind() {
            "function_item" => Some(("fn", field_text(node, "name", source)?)),
            "struct_item" => Some(("struct", field_text(node, "name", source)?)),
            "enum_item" => Some(("enum", field_text(node, "name", source)?)),
            "union_item" => Some(("union", field_text(node, "name", source)?)),
            "trait_item" => Some(("trait", field_text(node, "name", source)?)),
            "mod_item" => Some(("mod", field_text(node, "name", source)?)),
            "macro_definition" => Some(("macro", field_text(node, "name", source)?)),
            "const_item" => Some(("const", field_text(node, "name", source)?)),
            "static_item" => Some(("static", field_text(node, "name", source)?)),
            "type_item" => Some(("type", field_text(node, "name", source)?)),
            "impl_item" => {
                let ty = field_text(node, "type", source)?;
                let name = match field_text(node, "trait", source) {
                    Some(tr) => format!("{tr} for {ty}"),
                    None => ty,
                };
                Some(("impl", name))
            }
            _ => None,
        },
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => match node.kind() {
            "function_declaration" | "generator_function_declaration" => {
                Some(("fn", field_text(node, "name", source)?))
            }
            "class_declaration" | "abstract_class_declaration" => {
                Some(("class", field_text(node, "name", source)?))
            }
            "method_definition" => Some(("method", field_text(node, "name", source)?)),
            "interface_declaration" => Some(("interface", field_text(node, "name", source)?)),
            "enum_declaration" => Some(("enum", field_text(node, "name", source)?)),
            "type_alias_declaration" => Some(("type", field_text(node, "name", source)?)),
            "module" | "internal_module" => Some(("namespace", field_text(node, "name", source)?)),
            // `const foo = () => {}` / `const foo = function () {}` — a function
            // in every way that matters for an outline.
            "variable_declarator" => {
                let value = node.child_by_field_name("value")?;
                if matches!(value.kind(), "arrow_function" | "function_expression") {
                    Some(("fn", field_text(node, "name", source)?))
                } else {
                    None
                }
            }
            _ => None,
        },
        Lang::Python => match node.kind() {
            "function_definition" => Some(("fn", field_text(node, "name", source)?)),
            "class_definition" => Some(("class", field_text(node, "name", source)?)),
            _ => None,
        },
        Lang::Go => match node.kind() {
            "function_declaration" => Some(("fn", field_text(node, "name", source)?)),
            "method_declaration" => Some(("method", field_text(node, "name", source)?)),
            "type_spec" => Some(("type", field_text(node, "name", source)?)),
            _ => None,
        },
        Lang::Json => match node.kind() {
            "pair" => {
                let key = field_text(node, "key", source)?;
                Some(("key", key.trim_matches('"').to_string()))
            }
            _ => None,
        },
        Lang::Yaml => match node.kind() {
            "block_mapping_pair" => Some(("key", field_text(node, "key", source)?)),
            _ => None,
        },
        Lang::Toml => match node.kind() {
            "table" => Some(("table", first_named_child_text(node, source)?)),
            "table_array_element" => Some(("table", first_named_child_text(node, source)?)),
            _ => None,
        },
        Lang::Bash => match node.kind() {
            "function_definition" => Some(("fn", field_text(node, "name", source)?)),
            _ => None,
        },
        Lang::Html => match node.kind() {
            "element" => {
                let tag = node
                    .child(0)
                    .filter(|c| matches!(c.kind(), "start_tag" | "self_closing_tag"))?;
                let mut name = child_text_by_kind(tag, "tag_name", source)?;
                if let Some(id) = html_id_attribute(tag, source) {
                    name.push('#');
                    name.push_str(&id);
                }
                Some(("element", name))
            }
            _ => None,
        },
        Lang::Css => match node.kind() {
            "rule_set" => Some(("rule", first_named_child_text(node, source)?)),
            "media_statement" => Some(("@media", one_line(node_text(node, source), 80))),
            "keyframes_statement" => {
                Some(("@keyframes", child_text_by_kind(node, "keyframes_name", source)?))
            }
            _ => None,
        },
        Lang::Markdown => match node.kind() {
            // tree-sitter-md wraps each heading + its content in a `section`,
            // so sections nest exactly like the document's heading structure.
            "section" => {
                let mut cursor = node.walk();
                let heading = node
                    .named_children(&mut cursor)
                    .find(|c| matches!(c.kind(), "atx_heading" | "setext_heading"))?;
                Some((heading_kind(heading), heading_name(heading, source)))
            }
            _ => None,
        },
    }
}

/// The `id="…"` attribute value of an HTML start tag, if present.
fn html_id_attribute(start_tag: Node, source: &str) -> Option<String> {
    let mut cursor = start_tag.walk();
    for attr in start_tag.named_children(&mut cursor) {
        if attr.kind() != "attribute" {
            continue;
        }
        let is_id = attr
            .named_child(0)
            .is_some_and(|n| n.kind() == "attribute_name" && node_text(n, source) == "id");
        if is_id {
            let mut ac = attr.walk();
            let value = attr
                .named_children(&mut ac)
                .find(|n| matches!(n.kind(), "attribute_value" | "quoted_attribute_value"));
            return value.map(|v| one_line(node_text(v, source).trim_matches('"'), 60));
        }
    }
    None
}

/// Heading level of a markdown heading node, as an outline kind ("h1".."h6").
fn heading_kind(heading: Node) -> &'static str {
    let mut cursor = heading.walk();
    for child in heading.children(&mut cursor) {
        match child.kind() {
            "atx_h1_marker" | "setext_h1_underline" => return "h1",
            "atx_h2_marker" | "setext_h2_underline" => return "h2",
            "atx_h3_marker" => return "h3",
            "atx_h4_marker" => return "h4",
            "atx_h5_marker" => return "h5",
            "atx_h6_marker" => return "h6",
            _ => {}
        }
    }
    "h1"
}

fn heading_name(heading: Node, source: &str) -> String {
    if let Some(inline) = heading.named_child(0).filter(|n| n.kind() == "inline") {
        return one_line(node_text(inline, source), 120);
    }
    one_line(node_text(heading, source).trim_start_matches('#'), 120)
}

/// Extract the symbol tree of `source`: functions, classes, impls, methods,
/// sections — per-language kinds, one shared walk. Lines are 1-based.
pub fn outline(source: &str, lang: Lang) -> Result<Vec<Symbol>, Error> {
    let tree = parse(source, lang)?;
    let max_depth = lang.max_outline_depth();

    struct Open {
        end_byte: usize,
        symbol: Symbol,
    }
    let mut roots: Vec<Symbol> = Vec::new();
    let mut open: Vec<Open> = Vec::new();

    // Close every open symbol that ends at or before `pos`, attaching it to
    // its parent (the next symbol down the stack) or to the root list.
    fn close_through(open: &mut Vec<Open>, roots: &mut Vec<Symbol>, pos: usize) {
        while open.last().is_some_and(|o| o.end_byte <= pos) {
            let done = open.pop().expect("checked non-empty");
            match open.last_mut() {
                Some(parent) => parent.symbol.children.push(done.symbol),
                None => roots.push(done.symbol),
            }
        }
    }

    // Iterative pre-order walk (an explicit cursor, not recursion — minified
    // sources nest deeply enough to overflow a recursive walker's stack).
    let mut cursor = tree.walk();
    let mut depth = 0usize;
    'walk: loop {
        let node = cursor.node();
        close_through(&mut open, &mut roots, node.start_byte());
        if let Some((kind, name)) = symbol_for(lang, node, source) {
            open.push(Open {
                end_byte: node.end_byte(),
                symbol: Symbol {
                    name,
                    kind,
                    start_line: node.start_position().row + 1,
                    end_line: node.end_position().row + 1,
                    children: Vec::new(),
                },
            });
        }
        let descend = max_depth.is_none_or(|m| depth < m);
        if descend && cursor.goto_first_child() {
            depth += 1;
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                continue 'walk;
            }
            if !cursor.goto_parent() {
                break 'walk;
            }
            depth -= 1;
        }
    }
    close_through(&mut open, &mut roots, usize::MAX);
    Ok(roots)
}

/// Report tree-sitter ERROR/MISSING nodes in `source` as positioned syntax
/// errors. An empty result means the grammar accepted the whole input.
/// Capped at [`MAX_ERRORS`] entries.
pub fn parse_check(source: &str, lang: Lang) -> Result<Vec<SyntaxError>, Error> {
    let tree = parse(source, lang)?;
    if !tree.root_node().has_error() {
        return Ok(Vec::new());
    }
    let lines: Vec<&str> = source.lines().collect();
    let mut errors: Vec<SyntaxError> = Vec::new();

    let mut cursor = tree.walk();
    'walk: loop {
        let node = cursor.node();
        if errors.len() >= MAX_ERRORS {
            break;
        }
        let mut descend = false;
        if node.is_missing() {
            errors.push(syntax_error(node, &lines, format!("missing '{}'", node.kind())));
        } else if node.is_error() {
            let snippet = one_line(node_text(node, source), 20);
            let message = if snippet.is_empty() {
                "invalid syntax".to_string()
            } else {
                format!("unexpected '{snippet}'")
            };
            errors.push(syntax_error(node, &lines, message));
            // Don't descend into an ERROR node — its children re-report the
            // same broken region as noise.
        } else {
            descend = node.has_error();
        }
        if descend && cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                continue 'walk;
            }
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    Ok(errors)
}

fn syntax_error(node: Node, lines: &[&str], message: String) -> SyntaxError {
    let pos = node.start_position();
    SyntaxError {
        line: pos.row + 1,
        col: pos.column + 1,
        message,
        excerpt: lines.get(pos.row).map(|l| one_line(l, 60)).unwrap_or_default(),
    }
}

/// Run a tree-sitter query (structural grep) over `source`, returning every
/// capture. Capped at [`MAX_QUERY_MATCHES`] hits for memory safety; display
/// caps with stated omission are the caller's job.
pub fn query(source: &str, lang: Lang, ts_query: &str) -> Result<Vec<QueryHit>, Error> {
    let tree = parse(source, lang)?;
    let q = Query::new(&lang.language(), ts_query).map_err(|e| Error::Query(e.to_string()))?;
    let capture_names = q.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&q, tree.root_node(), source.as_bytes());
    let mut hits: Vec<QueryHit> = Vec::new();
    'outer: while let Some(m) = matches.next() {
        for capture in m.captures {
            if hits.len() >= MAX_QUERY_MATCHES {
                break 'outer;
            }
            hits.push(QueryHit {
                capture: capture_names[capture.index as usize].to_string(),
                kind: capture.node.kind().to_string(),
                line: capture.node.start_position().row + 1,
                text: one_line(node_text(capture.node, source), 80),
            });
        }
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_names(symbols: &[Symbol]) -> Vec<(String, String)> {
        let mut out = Vec::new();
        fn push(symbols: &[Symbol], out: &mut Vec<(String, String)>) {
            for s in symbols {
                out.push((s.kind.to_string(), s.name.clone()));
                push(&s.children, out);
            }
        }
        push(symbols, &mut out);
        out
    }

    // ── Language detection ──────────────────────────────────────────

    /// Detection maps every tier-1 extension to its grammar and returns
    /// `None` — not a guess — for anything else.
    #[test]
    fn detects_tier1_extensions_and_nothing_else() {
        let cases = [
            ("a.rs", Lang::Rust),
            ("a.ts", Lang::TypeScript),
            ("a.tsx", Lang::Tsx),
            ("a.js", Lang::JavaScript),
            ("a.jsx", Lang::JavaScript),
            ("a.py", Lang::Python),
            ("a.go", Lang::Go),
            ("a.json", Lang::Json),
            ("a.yaml", Lang::Yaml),
            ("a.yml", Lang::Yaml),
            ("a.toml", Lang::Toml),
            ("a.sh", Lang::Bash),
            ("a.html", Lang::Html),
            ("a.css", Lang::Css),
            ("a.md", Lang::Markdown),
        ];
        for (path, want) in cases {
            assert_eq!(Lang::from_path(Path::new(path)), Some(want), "{path}");
        }
        for path in ["a.txt", "a.exe", "a", "a.zsh", "Makefile"] {
            assert_eq!(Lang::from_path(Path::new(path)), None, "{path}");
        }
    }

    // ── Outline per language ────────────────────────────────────────

    /// Rust outline nests methods under their impl and reports 1-based
    /// line ranges.
    #[test]
    fn outline_rust_nests_impl_methods() {
        let src = "struct Foo;\n\nimpl Foo {\n    fn bar(&self) {}\n    fn baz(&self) {}\n}\n\nfn free() {}\n";
        let symbols = outline(src, Lang::Rust).unwrap();
        let flat = flat_names(&symbols);
        assert!(flat.contains(&("struct".into(), "Foo".into())), "{flat:?}");
        assert!(flat.contains(&("fn".into(), "free".into())), "{flat:?}");
        let imp = symbols.iter().find(|s| s.kind == "impl").expect("impl symbol");
        assert_eq!(imp.name, "Foo");
        assert_eq!(imp.start_line, 3);
        assert_eq!(imp.end_line, 6);
        let methods: Vec<&str> = imp.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(methods, vec!["bar", "baz"], "methods nest under the impl");
    }

    /// TypeScript outline covers classes, methods, interfaces, and
    /// arrow-function consts.
    #[test]
    fn outline_typescript() {
        let src = "interface Shape { area(): number }\nclass Circle {\n  radius = 1;\n  area(): number { return 3 * this.radius; }\n}\nconst make = (r: number) => new Circle();\nfunction plain() {}\n";
        let flat = flat_names(&outline(src, Lang::TypeScript).unwrap());
        assert!(flat.contains(&("interface".into(), "Shape".into())), "{flat:?}");
        assert!(flat.contains(&("class".into(), "Circle".into())), "{flat:?}");
        assert!(flat.contains(&("method".into(), "area".into())), "{flat:?}");
        assert!(flat.contains(&("fn".into(), "make".into())), "{flat:?}");
        assert!(flat.contains(&("fn".into(), "plain".into())), "{flat:?}");
    }

    /// TSX parses JSX syntax that plain TypeScript rejects.
    #[test]
    fn outline_tsx_component() {
        let src = "export function App() {\n  return <div className=\"x\">hi</div>;\n}\n";
        let flat = flat_names(&outline(src, Lang::Tsx).unwrap());
        assert!(flat.contains(&("fn".into(), "App".into())), "{flat:?}");
        assert!(parse_check(src, Lang::Tsx).unwrap().is_empty(), "JSX is valid tsx");
    }

    /// JavaScript outline includes function declarations and arrow consts.
    #[test]
    fn outline_javascript() {
        let src = "function top() {}\nconst arrow = () => 1;\nclass K { m() {} }\n";
        let flat = flat_names(&outline(src, Lang::JavaScript).unwrap());
        assert!(flat.contains(&("fn".into(), "top".into())), "{flat:?}");
        assert!(flat.contains(&("fn".into(), "arrow".into())), "{flat:?}");
        assert!(flat.contains(&("method".into(), "m".into())), "{flat:?}");
    }

    /// Python outline nests methods under their class.
    #[test]
    fn outline_python_nests_class_methods() {
        let src = "class A:\n    def m(self):\n        pass\n\ndef free():\n    pass\n";
        let symbols = outline(src, Lang::Python).unwrap();
        let class = symbols.iter().find(|s| s.kind == "class").expect("class");
        assert_eq!(class.name, "A");
        assert_eq!(class.children.len(), 1);
        assert_eq!(class.children[0].name, "m");
        assert!(flat_names(&symbols).contains(&("fn".into(), "free".into())));
    }

    /// Go outline covers functions, methods, and type declarations.
    #[test]
    fn outline_go() {
        let src = "package p\n\ntype T struct{}\n\nfunc (t T) M() {}\n\nfunc F() {}\n";
        let flat = flat_names(&outline(src, Lang::Go).unwrap());
        assert!(flat.contains(&("type".into(), "T".into())), "{flat:?}");
        assert!(flat.contains(&("method".into(), "M".into())), "{flat:?}");
        assert!(flat.contains(&("fn".into(), "F".into())), "{flat:?}");
    }

    /// JSON outline is the top-level keys — leaf pairs deep in the tree
    /// must NOT flood it.
    #[test]
    fn outline_json_top_level_keys_only() {
        let src = "{\"name\": \"x\", \"deps\": {\"a\": {\"b\": 1}}}";
        let symbols = outline(src, Lang::Json).unwrap();
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["name", "deps"], "top-level keys only");
        assert!(symbols.iter().all(|s| s.children.is_empty()), "no deep pairs");
    }

    /// YAML outline is the top-level keys.
    #[test]
    fn outline_yaml_top_level_keys() {
        let src = "server:\n  host: x\n  port: 1\nlogging:\n  level: info\n";
        let symbols = outline(src, Lang::Yaml).unwrap();
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["server", "logging"]);
    }

    /// TOML outline is the table headers.
    #[test]
    fn outline_toml_tables() {
        let src = "top = 1\n\n[package]\nname = \"x\"\n\n[dependencies]\nserde = \"1\"\n";
        let symbols = outline(src, Lang::Toml).unwrap();
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"package"), "{names:?}");
        assert!(names.contains(&"dependencies"), "{names:?}");
    }

    /// Bash outline lists function definitions.
    #[test]
    fn outline_bash_functions() {
        let src = "#!/bin/bash\nbuild() {\n  echo hi\n}\nfunction deploy {\n  echo go\n}\n";
        let flat = flat_names(&outline(src, Lang::Bash).unwrap());
        assert!(flat.contains(&("fn".into(), "build".into())), "{flat:?}");
        assert!(flat.contains(&("fn".into(), "deploy".into())), "{flat:?}");
    }

    /// HTML outline lists shallow elements (tag, plus #id when present) and
    /// does not descend into deep noise.
    #[test]
    fn outline_html_shallow_elements() {
        let src = "<html><head><title>t</title></head><body><div id=\"app\"><p><span><b>x</b></span></p></div></body></html>";
        let flat = flat_names(&outline(src, Lang::Html).unwrap());
        let names: Vec<&str> = flat.iter().map(|(_, n)| n.as_str()).collect();
        assert!(names.contains(&"html"), "{names:?}");
        assert!(names.contains(&"body"), "{names:?}");
        assert!(names.contains(&"div#app"), "{names:?}");
        assert!(!names.contains(&"b"), "deep elements stay out: {names:?}");
    }

    /// CSS outline lists rule selectors and at-rules.
    #[test]
    fn outline_css_rules() {
        let src = ".card { color: red; }\n@media (max-width: 600px) {\n  .card { color: blue; }\n}\n@keyframes spin { from { top: 0; } }\n";
        let flat = flat_names(&outline(src, Lang::Css).unwrap());
        assert!(flat.contains(&("rule".into(), ".card".into())), "{flat:?}");
        assert!(flat.iter().any(|(k, _)| k == "@media"), "{flat:?}");
        assert!(flat.contains(&("@keyframes".into(), "spin".into())), "{flat:?}");
    }

    /// Markdown outline mirrors the heading hierarchy: an h2 section nests
    /// under its h1 section.
    #[test]
    fn outline_markdown_heading_tree() {
        let src = "# Title\n\nintro\n\n## Setup\n\nsteps\n\n## Usage\n\n# Appendix\n";
        let symbols = outline(src, Lang::Markdown).unwrap();
        let tops: Vec<(&str, &str)> =
            symbols.iter().map(|s| (s.kind, s.name.as_str())).collect();
        assert_eq!(tops, vec![("h1", "Title"), ("h1", "Appendix")], "{tops:?}");
        let title = &symbols[0];
        let subs: Vec<&str> = title.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(subs, vec!["Setup", "Usage"], "h2 sections nest under the h1");
    }

    // ── parse_check per language ────────────────────────────────────

    /// Valid source in every tier-1 language yields zero syntax errors —
    /// the edit-verification chain must never cry wolf on clean code.
    #[test]
    fn parse_check_clean_sources_report_no_errors() {
        let cases: Vec<(Lang, &str)> = vec![
            (Lang::Rust, "fn main() { println!(\"hi\"); }\n"),
            (Lang::TypeScript, "const x: number = 1;\n"),
            (Lang::Tsx, "const el = <div>hi</div>;\n"),
            (Lang::JavaScript, "const x = () => 1;\n"),
            (Lang::Python, "def f():\n    return 1\n"),
            (Lang::Go, "package p\n\nfunc F() {}\n"),
            (Lang::Json, "{\"a\": [1, 2]}"),
            (Lang::Yaml, "a: 1\nb:\n  - x\n"),
            (Lang::Toml, "[t]\na = 1\n"),
            (Lang::Bash, "for f in *; do echo \"$f\"; done\n"),
            (Lang::Html, "<html><body><p>hi</p></body></html>"),
            (Lang::Css, "a { color: red; }\n"),
            (Lang::Markdown, "# h\n\ntext\n"),
        ];
        for (lang, src) in cases {
            let errors = parse_check(src, lang).unwrap();
            assert!(errors.is_empty(), "{}: {errors:?}", lang.name());
        }
    }

    /// Broken source is reported with a 1-based line and a message — for every
    /// grammar strict enough to reject it. (markdown/html/yaml accept almost
    /// anything by design and are covered by the clean-source test above.)
    #[test]
    fn parse_check_broken_sources_report_positioned_errors() {
        let cases: Vec<(Lang, &str)> = vec![
            (Lang::Rust, "fn main() {\n    let x = ;\n}\n"),
            (Lang::TypeScript, "function f( {\n"),
            (Lang::JavaScript, "const x = ;\n"),
            (Lang::Python, "def f(:\n    pass\n"),
            (Lang::Go, "package p\n\nfunc F( {\n"),
            (Lang::Json, "{\"a\": }"),
            (Lang::Toml, "[table\nkey = 1\n"),
            (Lang::Bash, "if true; then\necho hi\n"),
            (Lang::Css, "a { color: red;\n"),
        ];
        for (lang, src) in cases {
            let errors = parse_check(src, lang).unwrap();
            assert!(!errors.is_empty(), "{} accepted broken source", lang.name());
            let e = &errors[0];
            assert!(e.line >= 1 && e.line <= src.lines().count() + 1,
                "{}: line {} out of range", lang.name(), e.line);
            assert!(!e.message.is_empty(), "{}: empty message", lang.name());
        }
    }

    /// A grammar-inserted MISSING token is reported as `missing '<token>'`
    /// (unclosed JSON object — the json grammar recovers by inserting the
    /// missing `}`), distinct from the `unexpected …` ERROR-node message.
    #[test]
    fn parse_check_reports_missing_tokens() {
        let src = "{\"a\": 1";
        let errors = parse_check(src, Lang::Json).unwrap();
        assert!(
            errors.iter().any(|e| e.message.contains("missing")),
            "expected a missing-token error: {errors:?}"
        );
    }

    /// Error collection is capped — a file of garbage cannot produce an
    /// unbounded error list.
    #[test]
    fn parse_check_error_list_is_bounded() {
        let src = "let = = ;\n".repeat(500);
        let errors = parse_check(&src, Lang::Rust).unwrap();
        assert!(!errors.is_empty());
        assert!(errors.len() <= MAX_ERRORS, "cap violated: {}", errors.len());
    }

    // ── query ───────────────────────────────────────────────────────

    /// A structural query returns each capture with its name, kind, and line.
    #[test]
    fn query_captures_function_names() {
        let src = "fn alpha() {}\nfn beta() {}\nstruct S;\n";
        let hits = query(src, Lang::Rust, "(function_item name: (identifier) @name)").unwrap();
        let names: Vec<&str> = hits.iter().map(|h| h.text.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
        assert_eq!(hits[0].capture, "name");
        assert_eq!(hits[0].line, 1);
        assert_eq!(hits[1].line, 2);
    }

    /// An invalid query is an error the caller can show — never a panic and
    /// never an empty "no matches" lie.
    #[test]
    fn query_invalid_syntax_is_an_error() {
        let err = query("fn a() {}", Lang::Rust, "(function_item").unwrap_err();
        assert!(matches!(err, Error::Query(_)), "{err:?}");
    }
}
