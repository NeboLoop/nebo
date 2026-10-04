//! JSX/TSX → self-contained interactive HTML via the embedded SWC compiler.
//!
//! Interactive artifacts (dashboards, charts, visualizations) are single-file
//! React components. The agent writes `dashboard.jsx`, converts `to: "html"`,
//! and the result renders through the existing sandboxed-iframe Work-panel
//! pathway — JSX is an INPUT format to the artifact pipeline, never a second
//! runtime. Pure Rust (SWC), identical on every platform, no host binaries.
//!
//! Bare npm imports resolve to esm.sh pinned to one React (the `?deps` pin
//! prevents the dual-React hazard); Tailwind loads from its CDN so components
//! can use utility classes. Each compile takes a `vendored` map (specifier →
//! a file under the app's `ui/`): a package saved into the app loads from
//! that file, offline; anything not in it loads from esm.sh (or the Tailwind
//! CDN) at run time, so an artifact converted with an empty map needs network
//! and shows the in-page error overlay offline instead of a blank frame.

use std::collections::{BTreeMap, BTreeSet};

use swc_core::common::comments::SingleThreadedComments;
use swc_core::common::errors::{Handler, HANDLER};
use swc_core::common::{sync::Lrc, FileName, Globals, Mark, SourceMap, GLOBALS};
use swc_core::ecma::ast::{EsVersion, ExportAll, ImportDecl, ModuleDecl, ModuleItem, NamedExport, Program, Str};
use swc_core::ecma::codegen::{text_writer::JsWriter, Emitter};
use swc_core::ecma::parser::{lexer::Lexer, EsSyntax, Parser, StringInput, Syntax, TsSyntax};
use swc_core::ecma::transforms::base::resolver;
use swc_core::ecma::transforms::react::{react, Options as ReactOptions, Runtime};
use swc_core::ecma::transforms::typescript::{tsx, Config as TsConfig, TsxConfig};
use swc_core::ecma::visit::{visit_mut_pass, VisitMut, VisitMutWith};

use crate::RenderError;

/// One React for the whole artifact — esm.sh dependencies are pinned to it.
const REACT_VERSION: &str = "18.3.1";

/// The Tailwind play CDN a single-file component page loads; also its key in
/// a `vendored` map and in the packages a compile reports.
pub const TAILWIND_CDN: &str = "https://cdn.tailwindcss.com";

/// A compiled file and the npm specifiers it loads (bare, as keys of a
/// `vendored` map; `react/jsx-dev-runtime` reported as `react/jsx-runtime`).
/// A single-file page also reports what its loader needs: `react`,
/// `react-dom/client` and [`TAILWIND_CDN`].
#[derive(Debug, Clone)]
pub struct Compiled {
    pub code: String,
    pub packages: BTreeSet<String>,
}

/// The esm.sh address of a bare specifier, pinned to one React: where a
/// package loads from when it was not saved into the app.
pub fn esm_url(spec: &str) -> String {
    if spec == "react" {
        format!("https://esm.sh/react@{REACT_VERSION}")
    } else if spec == "react/jsx-runtime" || spec == "react/jsx-dev-runtime" {
        format!("https://esm.sh/react@{REACT_VERSION}/jsx-runtime")
    } else if spec == "react-dom" {
        format!("https://esm.sh/react-dom@{REACT_VERSION}?deps=react@{REACT_VERSION}")
    } else if let Some(sub) = spec.strip_prefix("react-dom/") {
        format!("https://esm.sh/react-dom@{REACT_VERSION}/{sub}?deps=react@{REACT_VERSION}")
    } else {
        format!("https://esm.sh/{spec}?deps=react@{REACT_VERSION},react-dom@{REACT_VERSION}")
    }
}

/// Source language of the component file.
#[derive(Clone, Copy, PartialEq)]
pub enum JsxLang {
    Jsx,
    Tsx,
}

/// Compile a single-file React component to a self-contained HTML document.
/// Its imports stay bare and resolve through the page's import map: to
/// `./<file>` for a package in `vendored` (the page is `ui/index.html`), to
/// esm.sh for the rest. Tailwind likewise.
pub fn jsx_to_html(
    source: &str,
    title: &str,
    lang: JsxLang,
    vendored: &BTreeMap<String, String>,
) -> Result<Compiled, RenderError> {
    let Compiled { code, mut packages } = transform(source, lang)?;
    packages.extend(["react".to_string(), "react-dom/client".to_string(), TAILWIND_CDN.to_string()]);
    let at = |spec: &str| match vendored.get(spec) {
        Some(file) => format!("./{file}"),
        None if spec == TAILWIND_CDN => TAILWIND_CDN.to_string(),
        None => esm_url(spec),
    };
    let imports: serde_json::Map<String, serde_json::Value> = packages
        .iter()
        .filter(|p| p.as_str() != TAILWIND_CDN)
        .map(|p| (p.clone(), serde_json::Value::from(at(p))))
        .collect();
    let import_map = serde_json::to_string(&serde_json::json!({ "imports": imports }))
        .map_err(|e| RenderError::Export(format!("encode import map: {e}")))?
        .replace("</", "\\u003c/");
    // JSON-encode the module source for safe embedding in an inline script;
    // escape `</` so the literal can never terminate the <script> tag.
    let code_json = serde_json::to_string(&code)
        .map_err(|e| RenderError::Export(format!("encode component: {e}")))?
        .replace("</", "\\u003c/");
    let title_safe = title.replace('<', "&lt;").replace('>', "&gt;");
    let tailwind = at(TAILWIND_CDN).replace('"', "&quot;");
    let html = SHELL
        .replace("__TITLE__", &title_safe)
        .replace("__TAILWIND__", &tailwind)
        .replace("__IMPORT_MAP__", &import_map)
        .replace("__CODE_JSON__", &code_json);
    Ok(Compiled { code: html, packages })
}

/// Compile one TS/TSX/JSX module of an app page to a browser ES module, by
/// the file's extension (`.ts`, `.tsx`, `.jsx`). Unlike [`jsx_to_html`] it is
/// one file among many: relative imports stay, renamed to the `.js` file this
/// produces (`./board.tsx` → `./board.js`, `./board` → `./board.js`); a bare
/// npm import in `vendored` (specifier → path under `ui/`) becomes that file,
/// relative to this one (`./vendor/x.js`, `../vendor/x.js` from a subfolder);
/// any other goes to esm.sh pinned to one React, as there. `path` is relative
/// to `ui/`. No type checking: types are stripped. An error names the line
/// and column.
pub fn transpile_module(
    source: &str,
    path: &str,
    vendored: &BTreeMap<String, String>,
) -> Result<Compiled, RenderError> {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let (syntax, ts) = match ext.as_str() {
        "tsx" => (Syntax::Typescript(TsSyntax { tsx: true, ..Default::default() }), true),
        "ts" | "mts" => (Syntax::Typescript(TsSyntax::default()), true),
        "jsx" => (Syntax::Es(EsSyntax { jsx: true, ..Default::default() }), false),
        _ => return Err(RenderError::Input(format!("{path}: only .ts, .tsx and .jsx files are compiled"))),
    };
    // One `../` per folder the file sits in under ui/.
    let depth = path.split('/').filter(|s| !s.is_empty() && *s != ".").count().saturating_sub(1);
    let to_root = if depth == 0 { "./".to_string() } else { "../".repeat(depth) };
    compile(source, path, syntax, ts, Some((vendored, to_root)))
}

/// SWC pipeline: parse → resolve → strip types (tsx) → JSX automatic runtime
/// → rewrite bare imports to esm.sh → codegen.
fn transform(source: &str, lang: JsxLang) -> Result<Compiled, RenderError> {
    let syntax = match lang {
        JsxLang::Jsx => Syntax::Es(EsSyntax { jsx: true, ..Default::default() }),
        JsxLang::Tsx => Syntax::Typescript(TsSyntax { tsx: true, ..Default::default() }),
    };
    compile(source, "component", syntax, lang == JsxLang::Tsx, None)
}

/// The one SWC pipeline. `module`: an app module among others, with the
/// vendored packages and the way from its folder up to `ui/`; `None` is the
/// single-file artifact shape (a default export, no file imports, bare
/// imports left for the page's import map).
fn compile(
    source: &str,
    name: &str,
    syntax: Syntax,
    ts: bool,
    module: Option<(&BTreeMap<String, String>, String)>,
) -> Result<Compiled, RenderError> {
    let single_file = module.is_none();
    let cm: Lrc<SourceMap> = Default::default();
    // Transform-time diagnostics are surfaced via returned errors; the
    // handler sink just satisfies passes that report through HANDLER.
    let handler = Handler::with_emitter_writer(Box::new(std::io::sink()), Some(cm.clone()));
    let fm = cm.new_source_file(
        Lrc::new(FileName::Custom(name.into())),
        source.to_string(),
    );
    let at = |span: swc_core::common::Span| {
        let loc = cm.lookup_char_pos(span.lo);
        format!("{name}:{}:{}", loc.line, loc.col_display + 1)
    };
    let comments = SingleThreadedComments::default();
    let lexer = Lexer::new(syntax, EsVersion::latest(), StringInput::from(&*fm), Some(&comments));
    let mut parser = Parser::new_from(lexer);
    let program = parser
        .parse_program()
        .map_err(|e| RenderError::Input(format!("{} parse error: {}", at(swc_core::common::Spanned::span(&e)), e.kind().msg())))?;
    if let Some(e) = parser.take_errors().into_iter().next() {
        return Err(RenderError::Input(format!("{} parse error: {}", at(swc_core::common::Spanned::span(&e)), e.kind().msg())));
    }

    // The component must export default — that's what the shell mounts.
    let has_default = matches!(&program, Program::Module(m) if m.body.iter().any(|i| {
        matches!(
            i,
            ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(_))
                | ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultExpr(_))
        )
    }));
    if single_file && !has_default {
        return Err(RenderError::Input(
            "the component must `export default` a React component (e.g. `export default function App() { … }`)"
                .into(),
        ));
    }

    GLOBALS.set(&Globals::new(), || {
        HANDLER.set(&handler, || {
            let unresolved_mark = Mark::new();
            let top_level_mark = Mark::new();

            let mut program = program.apply(resolver(
                unresolved_mark,
                top_level_mark,
                ts,
            ));
            if ts {
                program = program.apply(tsx(
                    cm.clone(),
                    TsConfig::default(),
                    TsxConfig::default(),
                    &comments,
                    unresolved_mark,
                    top_level_mark,
                ));
            }
            program = program.apply(react(
                cm.clone(),
                Some(&comments),
                ReactOptions {
                    runtime: Some(Runtime::Automatic),
                    import_source: Some("react".into()),
                    ..Default::default()
                },
                top_level_mark,
                unresolved_mark,
            ));

            let mut rewrite = RewriteImports { error: None, module, packages: BTreeSet::new() };
            program.visit_mut_with(&mut visit_mut_pass(&mut rewrite));
            if let Some(msg) = rewrite.error {
                return Err(RenderError::Input(msg));
            }

            let mut buf = Vec::new();
            {
                let mut emitter = Emitter {
                    cfg: Default::default(),
                    cm: cm.clone(),
                    comments: None,
                    wr: JsWriter::new(cm.clone(), "\n", &mut buf, None),
                };
                emitter
                    .emit_program(&program)
                    .map_err(|e| RenderError::Export(format!("codegen: {e}")))?;
            }
            let code = String::from_utf8(buf).map_err(|e| RenderError::Export(format!("codegen utf8: {e}")))?;
            Ok(Compiled { code, packages: rewrite.packages })
        })
    })
}

/// Rewrite bare npm import specifiers to the vendored file or esm.sh,
/// pinned to one React, recording each. File-path imports are rejected for a
/// single-file artifact (whose bare imports stay for the import map); in an
/// app module they stay relative and name the compiled `.js` file.
struct RewriteImports<'a> {
    error: Option<String>,
    module: Option<(&'a BTreeMap<String, String>, String)>,
    packages: BTreeSet<String>,
}

impl VisitMut for RewriteImports<'_> {
    fn visit_mut_import_decl(&mut self, n: &mut ImportDecl) {
        if let Some(mapped) = self.map(&n.src.value.to_string_lossy()) {
            n.src = Box::new(Str::from(swc_core::atoms::Atom::from(mapped)));
        }
    }

    fn visit_mut_named_export(&mut self, n: &mut NamedExport) {
        if let Some(src) = n.src.as_mut()
            && let Some(mapped) = self.map(&src.value.to_string_lossy())
        {
            **src = Str::from(swc_core::atoms::Atom::from(mapped));
        }
    }

    fn visit_mut_export_all(&mut self, n: &mut ExportAll) {
        if let Some(mapped) = self.map(&n.src.value.to_string_lossy()) {
            n.src = Box::new(Str::from(swc_core::atoms::Atom::from(mapped)));
        }
    }
}

impl RewriteImports<'_> {
    /// The specifier the browser loads instead, or None to keep it.
    fn map(&mut self, src: &str) -> Option<String> {
        let src = src.to_string();
        let single_file = self.module.is_none();
        if !single_file && (src.starts_with("./") || src.starts_with("../")) {
            if src.ends_with(".css") || src.ends_with(".scss") {
                self.error = Some(format!(
                    "import \"{src}\": a page module cannot import a stylesheet. Link it from index.html instead: <link rel=\"stylesheet\" href=\"{src}\">"
                ));
                return None;
            }
            let (stem, ext) = match src.rsplit_once('/') {
                Some((_, file)) if file.contains('.') => src.rsplit_once('.').map(|(s, e)| (s.to_string(), e.to_string())).unwrap(),
                _ => (src.clone(), String::new()),
            };
            return match ext.as_str() {
                "ts" | "tsx" | "jsx" | "mts" | "" => Some(format!("{stem}.js")),
                _ => None,
            };
        }
        if !single_file && src.starts_with('/') {
            self.error = Some(format!(
                "import \"{src}\" starts with \"/\": the page is served under a prefix on the phone, so it would not load. Use a relative path (\"./...\")."
            ));
            return None;
        }
        if src.starts_with("./") || src.starts_with("../") || src.starts_with('/') {
            self.error = Some(format!(
                "import \"{src}\" is a file path — interactive artifacts are single-file \
                 components. Inline that code into the component and convert again"
            ));
            return None;
        } else if src.starts_with("@/") || src.starts_with("~/") {
            // shadcn-style project aliases — they exist in other tools' artifact
            // runtimes, not here. Without this check they fail at view time with
            // an opaque "Importing a module script failed".
            self.error = Some(format!(
                "import \"{src}\" is a project-path alias — component libraries like \
                 shadcn/ui are not available here. Build that UI with plain JSX elements \
                 styled with Tailwind classes (or a real npm package), then convert again"
            ));
            return None;
        } else if src.starts_with("http://") || src.starts_with("https://") {
            return None;
        }
        let spec = if src == "react/jsx-dev-runtime" { "react/jsx-runtime".to_string() } else { src.clone() };
        self.packages.insert(spec.clone());
        match &self.module {
            None => (spec != src).then_some(spec),
            Some((vendored, to_root)) => Some(match vendored.get(&spec) {
                Some(file) => format!("{to_root}{file}"),
                None => esm_url(&spec),
            }),
        }
    }
}

/// HTML shell: Tailwind CDN, #root, error overlay, blob-module loader that
/// imports the compiled component and mounts its default export.
const SHELL: &str = r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>__TITLE__</title>
<script type="importmap">__IMPORT_MAP__</script>
<script src="__TAILWIND__"></script>
<style>html,body{min-height:100%;margin:0}body{background:#fafafa;font-family:system-ui,-apple-system,sans-serif}#root{min-height:100vh}</style>
</head>
<body>
<div id="root"></div>
<script type="module">
const __show = (m) => {
  const r = document.getElementById("root");
  r.innerHTML = "";
  const p = document.createElement("pre");
  p.style.cssText = "color:#b91c1c;padding:16px;white-space:pre-wrap;font:12px ui-monospace,monospace";
  p.textContent = m;
  r.appendChild(p);
};
window.addEventListener("error", (e) => __show("Error: " + e.message));
window.addEventListener("unhandledrejection", (e) => __show("Error: " + (e.reason && e.reason.message ? e.reason.message : e.reason)));
try {
  const __src = __CODE_JSON__;
  const __mod = await import(URL.createObjectURL(new Blob([__src], { type: "text/javascript" })));
  const __App = __mod.default;
  if (!__App) throw new Error("The component must `export default` a React component.");
  const { createElement } = await import("react");
  const { createRoot } = await import("react-dom/client");
  createRoot(document.getElementById("root")).render(createElement(__App));
} catch (e) {
  __show("Error: " + (e && e.message ? e.message : e));
}
</script>
</body>
</html>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    #[test]
    fn jsx_component_compiles_to_html() {
        let src = r#"
import { useState } from "react";
export default function App() {
  const [n, setN] = useState(0);
  return <button className="p-4" onClick={() => setN(n + 1)}>Count: {n}</button>;
}
"#;
        let html = jsx_to_html(src, "counter.jsx", JsxLang::Jsx, &none()).expect("compile").code;
        assert!(html.contains("https://esm.sh/react@18.3.1"), "react rewritten");
        assert!(html.contains("jsx-runtime"), "automatic runtime import present");
        assert!(html.contains("cdn.tailwindcss.com"), "tailwind available");
        assert!(!html.contains("<button className"), "JSX was transformed");
    }

    #[test]
    fn tsx_types_are_stripped() {
        let src = r#"
type Props = { label: string };
export default function App({ label }: Props = { label: "hi" }) {
  const n: number = 1;
  return <div>{label}{n}</div>;
}
"#;
        let html = jsx_to_html(src, "t.tsx", JsxLang::Tsx, &none()).expect("compile tsx").code;
        assert!(!html.contains("type Props"), "types stripped");
    }

    #[test]
    fn bare_imports_rewrite_to_esm_sh_with_react_pin() {
        let src = r#"
import { LineChart } from "recharts";
export default function App() { return <LineChart data={[]} />; }
"#;
        let html = jsx_to_html(src, "c.jsx", JsxLang::Jsx, &none()).expect("compile").code;
        assert!(
            html.contains(r#""recharts":"https://esm.sh/recharts?deps=react@18.3.1,react-dom@18.3.1""#),
            "recharts pinned to our react: {html}"
        );
    }

    #[test]
    fn shadcn_alias_imports_are_rejected() {
        // Models trained on other artifact runtimes emit shadcn-style aliases.
        let src = r#"
import { Table } from "@/components/ui/table";
export default function App() { return <Table />; }
"#;
        let err = jsx_to_html(src, "c.jsx", JsxLang::Jsx, &none()).expect_err("must reject");
        assert!(err.to_string().contains("Tailwind"), "corrective: {err}");
    }

    #[test]
    fn relative_imports_are_rejected() {
        let src = r#"
import helper from "./helper";
export default function App() { return <div>{helper()}</div>; }
"#;
        let err = jsx_to_html(src, "c.jsx", JsxLang::Jsx, &none()).expect_err("must reject");
        assert!(err.to_string().contains("single-file"), "corrective: {err}");
    }

    #[test]
    fn missing_default_export_is_rejected() {
        let src = r#"export function App() { return <div>hi</div>; }"#;
        let err = jsx_to_html(src, "c.jsx", JsxLang::Jsx, &none()).expect_err("must reject");
        assert!(err.to_string().contains("export default"), "corrective: {err}");
    }

    /// An app's TSX module compiles to a browser module: types gone, JSX
    /// compiled, its own files imported as the `.js` they become, npm
    /// packages from esm.sh, the pinned three kept pinned.
    #[test]
    fn an_app_module_compiles_with_relative_imports() {
        let src = r#"
import { useState } from "react";
import * as THREE from "three@0.170.0";
import { startScene } from "./scene";
import type { Config } from "./config.ts";
export { tune } from "./tune.tsx";
type P = { n: number };
export default function App({ n }: P) { const [x] = useState<number>(n); return <p>{x}{THREE.REVISION}</p>; }
startScene();
"#;
        let js = transpile_module(src, "app.tsx", &none()).expect("compiles").code;
        assert!(js.contains(r#""./scene.js""#), "{js}");
        assert!(js.contains(r#""./tune.js""#), "{js}");
        assert!(js.contains("https://esm.sh/three@0.170.0?deps=react@18.3.1"), "{js}");
        assert!(js.contains("https://esm.sh/react@18.3.1/jsx-runtime"), "{js}");
        assert!(!js.contains("type P") && !js.contains("<p>"), "{js}");
        let ts = transpile_module("export const speed: number = 3;\n", "config.ts", &none()).expect("ts").code;
        assert!(ts.contains("export const speed = 3"), "{ts}");
    }

    /// What goes wrong says where, so the employee fixes it without asking.
    #[test]
    fn an_app_module_error_names_file_line_and_column() {
        let err = transpile_module("const a = 1;\nconst b = (;\n", "board.jsx", &none()).unwrap_err().to_string();
        assert!(err.contains("board.jsx:2:"), "{err}");
        let err = transpile_module("import './app.css';\n", "app.tsx", &none()).unwrap_err().to_string();
        assert!(err.contains("<link rel=\"stylesheet\""), "{err}");
        let err = transpile_module("import x from '/assets/x.js';\nconsole.log(x);\n", "app.tsx", &none()).unwrap_err().to_string();
        assert!(err.contains("relative path"), "{err}");
        assert!(transpile_module("x", "app.vue", &none()).is_err());
    }

    /// A package saved into the app loads from its file, relative to the
    /// importing module: `./vendor/...` at the top of ui/, `../vendor/...`
    /// from a subfolder. What is not saved still goes to esm.sh, and every
    /// bare specifier is reported so the caller can save it.
    #[test]
    fn vendored_packages_import_from_the_app_relative_to_the_module() {
        let vendored: BTreeMap<String, String> = [
            ("recharts".to_string(), "vendor/recharts@3.10.1-a1b2c3d4e5.js".to_string()),
            ("react/jsx-runtime".to_string(), "vendor/react-jsx-runtime@18.3.1-f6g7h8i9j0.js".to_string()),
        ]
        .into();
        let src = "import { LineChart } from 'recharts';\nimport gsap from 'gsap@3.12.5';\nexport const C = () => <LineChart />;\n";
        let top = transpile_module(src, "app.jsx", &vendored).unwrap();
        assert!(top.code.contains(r#""./vendor/recharts@3.10.1-a1b2c3d4e5.js""#), "{}", top.code);
        assert!(top.code.contains(r#""./vendor/react-jsx-runtime@18.3.1-f6g7h8i9j0.js""#), "{}", top.code);
        assert!(top.code.contains("https://esm.sh/gsap@3.12.5?deps="), "{}", top.code);
        let want: BTreeSet<String> = ["gsap@3.12.5", "react/jsx-runtime", "recharts"].map(String::from).into();
        assert_eq!(top.packages, want);
        let deep = transpile_module(src, "screens/home.jsx", &vendored).unwrap();
        assert!(deep.code.contains(r#""../vendor/recharts@3.10.1-a1b2c3d4e5.js""#), "{}", deep.code);
        let deeper = transpile_module(src, "screens/parts/card.jsx", &vendored).unwrap();
        assert!(deeper.code.contains(r#""../../vendor/recharts@3.10.1-a1b2c3d4e5.js""#), "{}", deeper.code);
    }

    /// A single-file page keeps bare imports and resolves them through its
    /// import map, so the blob-loaded component finds the saved files; the
    /// loader's React and Tailwind come from the app too.
    #[test]
    fn a_single_file_page_maps_vendored_packages_through_its_import_map() {
        let vendored: BTreeMap<String, String> = [
            ("react".to_string(), "vendor/react@18.3.1-a1b2c3d4e5.js".to_string()),
            ("react-dom/client".to_string(), "vendor/react-dom-client@18.3.1-a1b2c3d4e6.js".to_string()),
            (TAILWIND_CDN.to_string(), "vendor/tailwindcss@3.4.17-a1b2c3d4e7.js".to_string()),
        ]
        .into();
        let src = "import { useState } from 'react';\nexport default function App() { const [n] = useState(0); return <b>{n}</b>; }\n";
        let page = jsx_to_html(src, "t", JsxLang::Jsx, &vendored).unwrap();
        assert!(page.code.contains(r#""react":"./vendor/react@18.3.1-a1b2c3d4e5.js""#), "{}", page.code);
        assert!(page.code.contains(r#""react-dom/client":"./vendor/react-dom-client@18.3.1-a1b2c3d4e6.js""#), "{}", page.code);
        assert!(page.code.contains(r#"<script src="./vendor/tailwindcss@3.4.17-a1b2c3d4e7.js">"#), "{}", page.code);
        assert!(page.code.contains(r#""react/jsx-runtime":"https://esm.sh/react@18.3.1/jsx-runtime""#), "{}", page.code);
        assert!(!page.code.contains("cdn.tailwindcss.com"), "{}", page.code);
        let want: BTreeSet<String> = ["react", "react-dom/client", "react/jsx-runtime", TAILWIND_CDN].map(String::from).into();
        assert_eq!(page.packages, want);
    }
}
