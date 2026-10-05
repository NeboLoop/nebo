//! A turn the owner started from the phone app as an app store ships it
//! (iOS, Android). Both stores forbid pointing a buyer in the app at a
//! checkout outside the store, so such a turn is never handed a checkout,
//! billing or pricing link and never quotes NeboAI's web prices: plans,
//! credit and billing live in the app, at [`PLAN_IN_APP`].
//!
//! The ONE rule, read three ways: [`is_store_app`] (the platform test the
//! install pathway and every tool share), [`GUIDANCE`] (what the model is
//! told on such a turn) and [`withhold_links`] (what a tool result is
//! stripped of before the model reads it).

use std::borrow::Cow;
use std::sync::LazyLock;

/// Where the owner manages his plan, credit and billing in the phone app.
pub const PLAN_IN_APP: &str = "Open Settings → Account → Plan in the app.";

/// What the model is told on a turn from the phone app.
pub const GUIDANCE: &str = "The owner is using the NeboAI mobile app. Never quote NeboAI's prices \
from the web and never give a checkout, pricing, billing or payment link for NeboAI. For anything \
about the plan, credit or billing, tell them: Open Settings → Account → Plan in the app.";

/// Whether `platform` (a turn's `X-Nebo-Platform`) is the phone app as an
/// app store ships it.
pub fn is_store_app(platform: Option<&str>) -> bool {
    matches!(platform, Some("ios" | "android"))
}

/// A link to a NeboAI or NeboLoop host (or subdomain), or to Stripe's
/// checkout, billing-portal or payment-link host, captured with its host
/// (group 1) and path (group 2).
static NEBOAI_LINK: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?i)\bhttps?://((?:[a-z0-9-]+\.)*(?:neboai|neboloop)\.com|(?:checkout|billing|buy)\.stripe\.com)(?::\d+)?(/[^\s"'<>)\]]*)?"#,
    )
    .expect("NEBOAI_LINK is a literal")
});

/// Whether a link to `host` with `path` is a page where NeboAI sells
/// something, the list the mobile app keeps (`purchasePage`): every Stripe
/// checkout, billing-portal or payment link; on NeboAI, a path segment
/// named checkout, billing, pricing or upgrade, the Cloud and Talk sale
/// pages, the billing portal, and the app's plans, credit and usage pages.
/// Links to anyone else's site are not NeboAI's sale.
fn is_sale_link(host: &str, path: &str) -> bool {
    if host.to_ascii_lowercase().ends_with(".stripe.com") {
        return true;
    }
    let path = path.split(['?', '#']).next().unwrap_or_default().to_ascii_lowercase();
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segs.iter().any(|seg| {
        matches!(
            *seg,
            "checkout" | "billing" | "pricing" | "pricing-new" | "pricing-preview" | "upgrade"
        )
    }) {
        return true;
    }
    match segs.as_slice() {
        [first, ..] if matches!(*first, "cloud" | "talk" | "portal") => true,
        ["app", page, ..] => matches!(*page, "plans" | "credits" | "usage" | "portal"),
        _ => false,
    }
}

/// Whether `c` (a [`NEBOAI_LINK`] match) is a sale link.
fn is_sale(c: &regex::Captures<'_>) -> bool {
    is_sale_link(&c[1], c.get(2).map_or("", |p| p.as_str()))
}

/// `text` with every NeboAI checkout, billing or pricing link (and every
/// Stripe checkout, billing-portal or payment link) replaced by
/// [`PLAN_IN_APP`]. Unchanged (borrowed) when it has none.
pub fn withhold_links(text: &str) -> Cow<'_, str> {
    if !NEBOAI_LINK
        .captures_iter(text)
        .any(|c| is_sale(&c))
    {
        return Cow::Borrowed(text);
    }
    NEBOAI_LINK.replace_all(text, |c: &regex::Captures<'_>| {
        if is_sale(c) {
            PLAN_IN_APP.to_string()
        } else {
            c[0].to_string()
        }
    })
}

/// A notice the gateway wrote for the owner (an over-limit refusal, say)
/// as a phone-app turn may show it: every sentence that carries a sale link
/// is dropped and [`PLAN_IN_APP`] said instead. Unchanged (borrowed) when it
/// has none.
pub fn withhold_from_notice(text: &str) -> Cow<'_, str> {
    if !NEBOAI_LINK.captures_iter(text).any(|c| is_sale(&c)) {
        return Cow::Borrowed(text);
    }
    let mut kept = String::new();
    for sentence in sentences(text) {
        if NEBOAI_LINK.captures_iter(sentence).any(|c| is_sale(&c)) {
            continue;
        }
        kept.push_str(sentence);
    }
    let kept = kept.trim_end();
    Cow::Owned(if kept.is_empty() {
        PLAN_IN_APP.to_string()
    } else {
        format!("{kept} {PLAN_IN_APP}")
    })
}

/// `text` cut after each sentence's end (`.`, `!` or `?` then whitespace),
/// the whitespace kept with the sentence before it. A URL's dots are not
/// followed by whitespace, so a link stays whole.
fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        if matches!(ch, '.' | '!' | '?')
            && let Some(&(_, next)) = chars.peek()
            && next.is_whitespace()
        {
            while let Some(&(j, w)) = chars.peek() {
                if !w.is_whitespace() {
                    out.push(&text[start..j]);
                    start = j;
                    break;
                }
                chars.next();
            }
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_store_builds_are_the_store_app() {
        assert!(is_store_app(Some("ios")));
        assert!(is_store_app(Some("android")));
        assert!(!is_store_app(Some("macos")));
        assert!(!is_store_app(Some("web")));
        assert!(!is_store_app(None));
    }

    #[test]
    fn neboai_sale_links_are_withheld_and_nothing_else() {
        for link in [
            "https://neboai.com/checkout?plan=solo",
            "https://www.neboai.com/pricing",
            "https://neboai.com/app/billing",
            "http://app.neboloop.com/billing/portal?x=1",
            "https://NEBOAI.com/Checkout/abc",
            "https://neboai.com/cloud",
            "https://neboai.com/cloud?plan=business",
            "https://neboai.com/talk",
            "https://neboai.com/upgrade",
            "https://www.neboai.com/app/plans",
            "https://neboai.com/app/credits",
            "https://neboai.com/portal/session",
            "https://checkout.stripe.com/c/pay/cs_live_abc",
            "https://billing.stripe.com/p/session/xyz",
            "https://buy.stripe.com/test_00g",
        ] {
            let text = format!("Buy it here: {link} today.");
            let out = withhold_links(&text);
            assert_eq!(out, format!("Buy it here: {PLAN_IN_APP} today."), "{link}");
        }
        for kept in [
            "https://neboai.com/docs/billing-is-in-the-app",
            "https://neboai.com/app/notifications",
            "https://stripe.com/docs",
            "https://dashboard.stripe.com/payments",
            "https://shop.example.com/checkout",
            "https://shop.example.com/upgrade",
            "https://neboai.com/docs/cloud-bots",
            "https://neboai.com/help/talking-to-your-team",
            "No links at all, $25 a month.",
        ] {
            assert!(matches!(withhold_links(kept), Cow::Borrowed(_)), "{kept}");
        }
    }

    #[test]
    fn a_notice_with_a_sale_link_says_where_the_plan_is_in_the_app() {
        let notice = "You've used this month's plan. Upgrade at https://neboai.com/upgrade to keep going. \
                      It resets on the 1st.";
        assert_eq!(
            withhold_from_notice(notice),
            format!("You've used this month's plan. It resets on the 1st. {PLAN_IN_APP}")
        );
        assert_eq!(
            withhold_from_notice("Add credit: https://checkout.stripe.com/c/pay/cs_1"),
            PLAN_IN_APP
        );
        let plain = "You've used this month's plan. It resets on the 1st.";
        assert!(matches!(withhold_from_notice(plain), Cow::Borrowed(_)));
    }
}
