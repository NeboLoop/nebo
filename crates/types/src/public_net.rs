//! Which addresses are this computer's or its network's, and which are the
//! public internet: the ONE classifier every guard asks (the web tool's
//! fetches, an app page's frames in the bot's own browser).

/// Whether an IP is publicly routable — the classification half of the SSRF
/// guard, pure and unit-testable. Covers loopback, RFC1918, link-local,
/// CGNAT, reserved v4 ranges, and the private/special IPv6 ranges.
pub fn is_public_ip(ip: std::net::IpAddr) -> bool {
    // Unwrap IPv4-mapped IPv6 (::ffff:a.b.c.d) into the v4 address.
    match ip.to_canonical() {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || o[0] == 0 // 0.0.0.0/8 "this network"
                || (o[0] == 100 && (o[1] & 0xC0) == 64) // CGNAT 100.64.0.0/10
                || (o[0] == 192 && o[1] == 0 && o[2] == 0) // 192.0.0.0/24
                || o[0] >= 240) // 240.0.0.0/4 reserved
        }
        std::net::IpAddr::V6(v6) => {
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_multicast())
        }
    }
}

/// Whether a host name always means this computer: `localhost`, a name
/// under `.localhost`, with or without a trailing dot, any case.
pub fn is_local_name(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    h == "localhost" || h.ends_with(".localhost")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_public_ip_blocks_private_and_special_ranges() {
        let blocked: &[&str] = &[
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "100.64.0.1",
            "192.0.0.192",
            "255.255.255.255",
            "240.0.0.1",
            "::1",
            "::",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
        ];
        for ip in blocked {
            let parsed: std::net::IpAddr = ip.parse().unwrap();
            assert!(!is_public_ip(parsed), "{} should be blocked", ip);
        }
    }

    #[test]
    fn is_public_ip_allows_public_ranges() {
        // 172.2.x.x and 172.200.x.x are regressions for the old substring
        // check, which wrongly blocked them via the "://172.2" prefix.
        let allowed: &[&str] = &[
            "1.1.1.1",
            "8.8.8.8",
            "172.2.1.1",
            "172.200.5.5",
            "100.128.0.1",
            "93.184.216.34",
            "2606:4700::1111",
        ];
        for ip in allowed {
            let parsed: std::net::IpAddr = ip.parse().unwrap();
            assert!(is_public_ip(parsed), "{} should be allowed", ip);
        }
    }

    #[test]
    fn local_names_are_this_computer() {
        for n in ["localhost", "LOCALHOST", "localhost.", "app.localhost", "a.b.localhost."] {
            assert!(is_local_name(n), "{n}");
        }
        for n in ["example.com", "localhost.example.com", "notlocalhost"] {
            assert!(!is_local_name(n), "{n}");
        }
    }
}
