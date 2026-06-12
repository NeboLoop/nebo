//! Human-like input synthesis for the CDP (Obscura) tier — a direct port of the
//! Chrome extension's humanization (`chrome-extension/src/tools.ts`) so tier-2
//! browses **exactly like the extension**: curved, eased, jittered mouse paths,
//! human click durations, and irregular typing cadence.
//!
//! Why this exists: detection systems key on input *patterns*, not just
//! fingerprints. Mouse teleports, 0ms keystrokes, and constructed `?q=` URLs are
//! the bot signature that got the headless Obscura path flagged. Stealth flags
//! don't fix behavioral signals — synthesizing real human motion does.

use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventParams,
    DispatchMouseEventType, MouseButton,
};
use rand::Rng;
use std::time::Duration;

use crate::BrowserError;

fn err<E: std::fmt::Display>(ctx: &str) -> impl Fn(E) -> BrowserError + '_ {
    move |e| BrowserError::Other(format!("{ctx}: {e}"))
}

/// Sleep a random duration in `[min_ms, max_ms)` — the irregular gaps a hand produces.
async fn human_delay(min_ms: f64, max_ms: f64) {
    let ms = rand::thread_rng().gen_range(min_ms..max_ms);
    tokio::time::sleep(Duration::from_millis(ms as u64)).await;
}

/// One uniform sample in `[0, 1)`.
fn unit() -> f64 {
    rand::thread_rng().gen_range(0.0..1.0)
}

/// Move the pointer to `(tx, ty)` along an eased quadratic-bezier path with jitter,
/// starting from `from` (or a random nearby offset when unknown). Returns the final
/// position so the caller can thread it into the next move. Mirrors the extension's
/// `humanMouseMove` step-for-step.
pub async fn human_mouse_move(
    page: &Page,
    from: Option<(f64, f64)>,
    tx: f64,
    ty: f64,
) -> Result<(f64, f64), BrowserError> {
    let (fx, fy) = from.unwrap_or_else(|| {
        (
            tx + 80.0 + unit() * 160.0,
            (ty - 120.0 - unit() * 160.0).max(0.0),
        )
    });
    let dist = ((tx - fx).powi(2) + (ty - fy).powi(2)).sqrt();
    let steps = ((dist / 40.0).round() as i64).clamp(4, 18);
    // Random control point bends the path — straight-line travel is robotic.
    let (cx, cy) = (
        (fx + tx) / 2.0 + (unit() - 0.5) * (120.0_f64).min(dist / 2.0),
        (fy + ty) / 2.0 + (unit() - 0.5) * (120.0_f64).min(dist / 2.0),
    );
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let e = t * t * (3.0 - 2.0 * t); // smoothstep: slow-fast-slow like a real hand
        let (jx, jy) = if i < steps {
            ((unit() - 0.5) * 2.0, (unit() - 0.5) * 2.0)
        } else {
            (0.0, 0.0)
        };
        let x = (1.0 - e).powi(2) * fx + 2.0 * (1.0 - e) * e * cx + e * e * tx + jx;
        let y = (1.0 - e).powi(2) * fy + 2.0 * (1.0 - e) * e * cy + e * e * ty + jy;
        page.execute(
            DispatchMouseEventParams::builder()
                .r#type(DispatchMouseEventType::MouseMoved)
                .x(x)
                .y(y)
                .button(MouseButton::None)
                .buttons(0)
                .build()
                .map_err(err("build mouseMoved"))?,
        )
        .await
        .map_err(err("mouseMoved"))?;
        human_delay(8.0, 22.0).await;
    }
    Ok((tx, ty))
}

/// Human click at `(x, y)`: curved approach, settle, press with a 50–110ms human
/// hold, release. Returns the resting pointer position.
pub async fn human_click(
    page: &Page,
    from: Option<(f64, f64)>,
    x: f64,
    y: f64,
) -> Result<(f64, f64), BrowserError> {
    let pos = human_mouse_move(page, from, x, y).await?;
    human_delay(60.0, 160.0).await;
    page.execute(
        DispatchMouseEventParams::builder()
            .r#type(DispatchMouseEventType::MousePressed)
            .x(x)
            .y(y)
            .button(MouseButton::Left)
            .buttons(1)
            .click_count(1)
            .build()
            .map_err(err("build mousePressed"))?,
    )
    .await
    .map_err(err("mousePressed"))?;
    human_delay(50.0, 110.0).await;
    page.execute(
        DispatchMouseEventParams::builder()
            .r#type(DispatchMouseEventType::MouseReleased)
            .x(x)
            .y(y)
            .button(MouseButton::Left)
            .buttons(0)
            .click_count(1)
            .build()
            .map_err(err("build mouseReleased"))?,
    )
    .await
    .map_err(err("mouseReleased"))?;
    Ok(pos)
}

/// Type `text` with human cadence: irregular 35–110ms inter-key gaps (compressed
/// 4× for long strings so a paste-sized input doesn't take minutes). Each printable
/// char is a keyDown(text)+keyUp pair; `\n` becomes Enter.
pub async fn human_type(page: &Page, text: &str) -> Result<(), BrowserError> {
    let scale = if text.chars().count() > 200 { 0.25 } else { 1.0 };
    let mut first = true;
    for ch in text.chars() {
        if !first {
            human_delay(35.0 * scale, 110.0 * scale).await;
        }
        first = false;
        if ch == '\n' || ch == '\r' {
            press_key(page, "Enter").await?;
        } else {
            type_char(page, ch).await?;
        }
    }
    Ok(())
}

/// Dispatch a single printable character as a keyDown(text)+keyUp pair.
async fn type_char(page: &Page, ch: char) -> Result<(), BrowserError> {
    let s = ch.to_string();
    page.execute(
        DispatchKeyEventParams::builder()
            .r#type(DispatchKeyEventType::KeyDown)
            .text(s.clone())
            .unmodified_text(s.clone())
            .key(s.clone())
            .build()
            .map_err(err("build keyDown"))?,
    )
    .await
    .map_err(err("keyDown"))?;
    page.execute(
        DispatchKeyEventParams::builder()
            .r#type(DispatchKeyEventType::KeyUp)
            .key(s)
            .build()
            .map_err(err("build keyUp"))?,
    )
    .await
    .map_err(err("keyUp"))?;
    Ok(())
}

/// Press a named key (Enter, Tab, Escape, Backspace, arrows). Returns an error for
/// unmapped keys rather than silently typing the literal name (the extension's
/// `press` bug we already fixed there).
pub async fn press_key(page: &Page, key: &str) -> Result<(), BrowserError> {
    let (k, code, vk, text): (&str, &str, i64, Option<&str>) = match key {
        "Enter" | "enter" => ("Enter", "Enter", 13, Some("\r")),
        "Tab" | "tab" => ("Tab", "Tab", 9, Some("\t")),
        "Escape" | "escape" => ("Escape", "Escape", 27, None),
        "Backspace" | "backspace" => ("Backspace", "Backspace", 8, None),
        "ArrowDown" | "arrowdown" => ("ArrowDown", "ArrowDown", 40, None),
        "ArrowUp" | "arrowup" => ("ArrowUp", "ArrowUp", 38, None),
        "ArrowLeft" | "arrowleft" => ("ArrowLeft", "ArrowLeft", 37, None),
        "ArrowRight" | "arrowright" => ("ArrowRight", "ArrowRight", 39, None),
        other => {
            return Err(BrowserError::Other(format!(
                "press: unknown key '{other}' (CDP tier supports Enter/Tab/Escape/Backspace/arrows; use type for text)"
            )));
        }
    };
    let mut down = DispatchKeyEventParams::builder()
        .r#type(DispatchKeyEventType::KeyDown)
        .key(k)
        .code(code)
        .windows_virtual_key_code(vk);
    if let Some(t) = text {
        down = down.text(t).unmodified_text(t);
    }
    page.execute(down.build().map_err(err("build key down"))?)
        .await
        .map_err(err("key down"))?;
    page.execute(
        DispatchKeyEventParams::builder()
            .r#type(DispatchKeyEventType::KeyUp)
            .key(k)
            .code(code)
            .windows_virtual_key_code(vk)
            .build()
            .map_err(err("build key up"))?,
    )
    .await
    .map_err(err("key up"))?;
    Ok(())
}
