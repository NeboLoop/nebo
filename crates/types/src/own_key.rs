//! The owner's own key for a paid provider, on its way through Janus.
//!
//! Every paid call on a key the owner brought (chat, embeddings,
//! transcription, voice, web search, decisions) goes to Janus with the key in
//! `X-Provider-API-Key` and the provider it belongs to in `X-Provider` (owner,
//! 2026-10-19): Janus sends the call to that provider on his key, never to
//! another, and charges the platform request fee alone. Things that are not
//! paid services stay off Janus: Ollama and other servers on his own
//! machine or network ([`is_local_url`]), the coding agents on this computer,
//! and linked computers.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The caller's own provider key.
pub const KEY_HEADER: &str = "X-Provider-API-Key";
/// The provider the key belongs to, by Janus's name for it.
pub const PROVIDER_HEADER: &str = "X-Provider";
/// The owner's own endpoint, for a provider he reaches at a URL he typed.
pub const BASE_URL_HEADER: &str = "X-Provider-Base-URL";

/// One own key, with where it goes. Debug never prints the key.
#[derive(Clone, PartialEq, Eq)]
pub struct OwnKey {
    /// Janus's name for the provider (`anthropic`, `openai`, `google`,
    /// `deepseek`, `openrouter`, `xai`, `openai_compatible`, `brave`,
    /// `tavily`, `google_cse`, `serpapi`, `systemone_compatible`).
    pub provider: String,
    pub key: String,
    /// The owner's endpoint (`openai_compatible`, `systemone_compatible`).
    pub base_url: Option<String>,
}

impl OwnKey {
    /// The headers every Janus request on this key carries, as
    /// (name, value): the key, its provider, and the owner's endpoint when
    /// there is one.
    pub fn headers(&self) -> Vec<(&'static str, String)> {
        let mut out = vec![(KEY_HEADER, self.key.clone()), (PROVIDER_HEADER, self.provider.clone())];
        if let Some(url) = self.base_url.as_deref().map(str::trim).filter(|u| !u.is_empty()) {
            out.push((BASE_URL_HEADER, url.to_string()));
        }
        out
    }
}

impl std::fmt::Debug for OwnKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnKey")
            .field("provider", &self.provider)
            .field("key", &"[redacted]")
            .field("base_url", &self.base_url)
            .finish()
    }
}

/// Whether `url` is a server on the owner's own machine or network (LM
/// Studio, vLLM on the LAN, a box on his VPN), which is no paid service and
/// which Janus could not reach: anything but `https`, a host with no dot
/// (`my-nas`), `localhost`, `.local`, `.localhost`, `.internal`, `.lan`
/// and `.home.arpa` names, and loopback, private, link-local, shared
/// (100.64/10), unspecified and unique-local addresses.
pub fn is_local_url(url: &str) -> bool {
    let url = url.trim();
    let Some(rest) = url.get(..8).filter(|s| s.eq_ignore_ascii_case("https://")).map(|_| &url[8..]) else {
        return true;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");
    let host = match host_port.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => host_port.split(':').next().unwrap_or(""),
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return true;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return local_ip(ip);
    }
    !host.contains('.')
        || host == "localhost"
        || [".localhost", ".local", ".internal", ".lan", ".home.arpa"].iter().any(|s| host.ends_with(s))
}

fn local_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => local_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return local_v4(v4);
            }
            let first = v6.segments()[0];
            v6.is_loopback()
                || v6 == Ipv6Addr::UNSPECIFIED
                || (first & 0xfe00) == 0xfc00 // unique local fc00::/7
                || (first & 0xffc0) == 0xfe80 // link local fe80::/10
        }
    }
}

fn local_v4(v4: Ipv4Addr) -> bool {
    let [a, b, ..] = v4.octets();
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_unspecified()
        || (a == 100 && (64..128).contains(&b)) // shared address space
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_on_the_owners_machine_or_network_are_local() {
        for url in [
            "http://localhost:1234/v1",
            "http://api.example.com/v1",
            "https://localhost/v1",
            "https://127.0.0.1:8000/v1",
            "https://10.0.0.5/v1",
            "https://192.168.1.20:8080/v1",
            "https://172.16.4.1/v1",
            "https://169.254.169.254/latest",
            "https://100.100.1.1/v1",
            "https://[::1]:8000/v1",
            "https://[fd00::1]/v1",
            "https://[fe80::1]/v1",
            "https://[::ffff:10.0.0.1]/v1",
            "https://0.0.0.0/v1",
            "https://my-nas:8000/v1",
            "https://studio.local/v1",
            "https://box.lan/v1",
            "https://gpu.internal/v1",
            "https://router.home.arpa/v1",
            "",
            "ftp://api.example.com",
        ] {
            assert!(is_local_url(url), "{url} should be local");
        }
    }

    #[test]
    fn public_https_endpoints_are_paid_services() {
        for url in [
            "https://api.groq.com/openai/v1",
            "https://api.x.ai/v1",
            "HTTPS://api.together.xyz/v1",
            "https://user@api.mistral.ai:443/v1",
            "https://8.8.8.8/v1",
            "https://[2606:4700::1111]/v1",
            "https://api.typesafe.ai",
        ] {
            assert!(!is_local_url(url), "{url} should go through Janus");
        }
    }

    #[test]
    fn headers_carry_the_key_its_provider_and_the_owners_endpoint() {
        let k = OwnKey { provider: "openai_compatible".into(), key: "sk-1".into(), base_url: Some(" https://api.groq.com/openai/v1 ".into()) };
        assert_eq!(
            k.headers(),
            vec![
                (KEY_HEADER, "sk-1".to_string()),
                (PROVIDER_HEADER, "openai_compatible".to_string()),
                (BASE_URL_HEADER, "https://api.groq.com/openai/v1".to_string()),
            ]
        );
        let k = OwnKey { provider: "anthropic".into(), key: "sk-2".into(), base_url: None };
        assert_eq!(k.headers().len(), 2);
    }

    #[test]
    fn debug_never_prints_the_key() {
        let k = OwnKey { provider: "openai".into(), key: "sk-SENTINEL".into(), base_url: None };
        assert!(!format!("{k:?}").contains("SENTINEL"));
    }
}
