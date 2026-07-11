//! Shared rendering for the branded HTML pages the browser lands on after an
//! auth redirect (MCP OAuth callback, NeboAI sign-in callback). One canonical
//! design so every auth page looks identical — call [`auth_result_page`].

use axum::response::Html;

/// Render the branded auth-result page.
///
/// * `success` — picks the green success glyph or the red error glyph.
/// * `heading` — short headline (e.g. "All set", "Sign-in failed").
/// * `message` — detail line; escaped, so it may carry raw provider error text.
///
/// Self-contained (no external assets) since the browser navigates here directly.
///
/// NeboAI OAuth is opened with the system browser (`open::that`), not `window.open`,
/// so browsers block `window.close()`. The CTA therefore returns to the app; close
/// is best-effort only for rare script-opened popups (e.g. some MCP flows).
pub fn auth_result_page(success: bool, heading: &str, message: &str) -> Html<String> {
    // Brand palette (matches app.css): green success, red error.
    let accent = if success { "#138a4a" } else { "#cc2222" };
    // Animated status glyph drawn with SVG strokes.
    let glyph = if success {
        r#"<path class="draw" d="M14 27 l8 8 l16 -18" />"#
    } else {
        r#"<path class="draw" d="M18 18 l20 20 M38 18 l-20 20" />"#
    };
    let safe_heading = escape(heading);
    let safe_message = escape(message);

    Html(format!(
        r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Nebo</title>
<style>
:root{{--accent:{accent};--bg:#f7fbfc;--card:#ffffff;--ink:#0e1c26;--muted:#5b6b75;--line:#e2ebef}}
@media (prefers-color-scheme: dark){{:root{{--bg:#0b1014;--card:#121a20;--ink:#e6eef2;--muted:#8a9aa4;--line:#22303a}}}}
*{{box-sizing:border-box}}
html,body{{height:100%}}
body{{margin:0;font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif;
  background:radial-gradient(1200px 600px at 50% -10%,color-mix(in srgb,var(--accent) 9%,var(--bg)),var(--bg));
  color:var(--ink);display:flex;align-items:center;justify-content:center;padding:24px}}
.card{{width:100%;max-width:380px;background:var(--card);border:1px solid var(--line);border-radius:20px;
  padding:40px 32px 32px;text-align:center;box-shadow:0 1px 2px rgba(0,0,0,.04),0 20px 50px -20px rgba(0,0,0,.25);
  animation:rise .5s cubic-bezier(.16,1,.3,1) both}}
@keyframes rise{{from{{opacity:0;transform:translateY(12px)}}to{{opacity:1;transform:none}}}}
.ring{{width:76px;height:76px;margin:0 auto 22px;border-radius:50%;display:flex;align-items:center;justify-content:center;
  background:color-mix(in srgb,var(--accent) 12%,transparent)}}
.ring svg{{width:42px;height:42px}}
.ring path{{fill:none;stroke:var(--accent);stroke-width:5;stroke-linecap:round;stroke-linejoin:round}}
.draw{{stroke-dasharray:80;stroke-dashoffset:80;animation:draw .55s .2s ease forwards}}
@keyframes draw{{to{{stroke-dashoffset:0}}}}
h1{{margin:0 0 8px;font-size:22px;font-weight:650;letter-spacing:-.01em}}
.msg{{margin:0;font-size:15px;line-height:1.5;color:var(--muted)}}
.hint{{margin:24px 0 0;font-size:13px;color:var(--muted);opacity:.8}}
.btn{{display:inline-block;margin-top:20px;padding:10px 22px;border:0;border-radius:10px;cursor:pointer;
  font-size:14px;font-weight:600;color:#fff;background:var(--accent);transition:opacity .15s}}
.btn:hover{{opacity:.9}}
.wordmark{{margin-top:28px;font-size:12px;font-weight:600;letter-spacing:.18em;text-transform:uppercase;color:var(--muted);opacity:.7}}
</style></head>
<body>
<div class="card">
  <div class="ring"><svg viewBox="0 0 52 52">{glyph}</svg></div>
  <h1>{safe_heading}</h1>
  <p class="msg">{safe_message}</p>
  <button class="btn" type="button" id="done">Return to Nebo</button>
  <p class="hint" id="hint">Returning to Nebo…</p>
  <div class="wordmark">Nebo</div>
</div>
<script>
(function(){{
  var returned = false;
  function returnToNebo(){{
    if (returned) return;
    returned = true;
    // Best-effort: only works when this tab was opened via window.open().
    try {{ window.close(); }} catch (e) {{}}
    // System-browser OAuth (open::that) cannot be closed by script — go home.
    setTimeout(function(){{
      if (!window.closed) window.location.replace('/');
    }}, 150);
  }}
  document.getElementById('done').addEventListener('click', returnToNebo);
  setTimeout(returnToNebo, 2500);
}})();
</script>
</body></html>"#
    ))
}

/// Escape text for safe interpolation into the page body.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
