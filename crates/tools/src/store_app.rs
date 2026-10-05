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
pub const GUIDANCE: &str = "The owner is using the NeboAI phone app. Never quote NeboAI's prices \
from the web and never give a checkout, pricing, billing or payment link for NeboAI. For anything \
about the plan, credit or billing, tell them: Open Settings → Account → Plan in the app.";

/// Whether `platform` (a turn's `X-Nebo-Platform`) is the phone app as an
/// app store ships it.
pub fn is_store_app(platform: Option<&str>) -> bool {
    matches!(platform, Some("ios" | "android"))
}

/// A link to a NeboAI or NeboLoop host (or subdomain), captured with its
/// path (group 1).
static NEBOAI_LINK: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?i)\bhttps?://(?:[a-z0-9-]+\.)*(?:neboai|neboloop)\.com(?::\d+)?(/[^\s"'<>)\]]*)?"#,
    )
    .expect("NEBOAI_LINK is a literal")
});

/// Whether a NeboAI link's path is one of its checkout, billing or pricing
/// pages: a path segment named so. Links to anyone else's site, the owner's
/// own business payment links included, are not NeboAI's sale.
fn is_sale_path(path: &str) -> bool {
    let path = path.split(['?', '#']).next().unwrap_or_default();
    path.split('/').any(|seg| {
        matches!(
            seg.to_ascii_lowercase().as_str(),
            "checkout" | "billing" | "pricing"
        )
    })
}

/// `text` with every NeboAI checkout, billing or pricing link replaced by
/// [`PLAN_IN_APP`]. Unchanged (borrowed) when it has none.
pub fn withhold_links(text: &str) -> Cow<'_, str> {
    if !NEBOAI_LINK
        .captures_iter(text)
        .any(|c| c.get(1).is_some_and(|p| is_sale_path(p.as_str())))
    {
        return Cow::Borrowed(text);
    }
    NEBOAI_LINK.replace_all(text, |c: &regex::Captures<'_>| {
        if c.get(1).is_some_and(|p| is_sale_path(p.as_str())) {
            PLAN_IN_APP.to_string()
        } else {
            c[0].to_string()
        }
    })
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
        ] {
            let text = format!("Buy it here: {link} today.");
            let out = withhold_links(&text);
            assert_eq!(out, format!("Buy it here: {PLAN_IN_APP} today."), "{link}");
        }
        for kept in [
            "https://neboai.com/docs/billing-is-in-the-app",
            "https://neboai.com/app/notifications",
            "https://buy.stripe.com/owner-business-link",
            "https://shop.example.com/checkout",
            "No links at all, $25 a month.",
        ] {
            assert!(matches!(withhold_links(kept), Cow::Borrowed(_)), "{kept}");
        }
    }
}
