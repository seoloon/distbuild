use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Returns true if `addr` is loopback, RFC1918 private, or link-local —
/// the only address classes DistBuild workers accept connections from,
/// per PROMPT.md's security section ("Reject non-loopback / non-private
/// IPs (RFC1918 + link-local only)").
pub fn is_allowed_addr(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => is_allowed_v4(v4),
        IpAddr::V6(v6) => is_allowed_v6(v6),
    }
}

fn is_allowed_v4(addr: Ipv4Addr) -> bool {
    addr.is_loopback() || addr.is_private() || addr.is_link_local()
}

fn is_allowed_v6(addr: Ipv6Addr) -> bool {
    if addr.is_loopback() {
        return true;
    }
    if let Some(v4) = addr.to_ipv4_mapped() {
        return is_allowed_v4(v4);
    }
    // fe80::/10
    (addr.segments()[0] & 0xffc0) == 0xfe80
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_loopback() {
        assert!(is_allowed_addr("127.0.0.1".parse().unwrap()));
        assert!(is_allowed_addr("::1".parse().unwrap()));
    }

    #[test]
    fn allows_rfc1918_private_ranges() {
        assert!(is_allowed_addr("10.1.2.3".parse().unwrap()));
        assert!(is_allowed_addr("172.16.0.1".parse().unwrap()));
        assert!(is_allowed_addr("172.31.255.254".parse().unwrap()));
        assert!(is_allowed_addr("192.168.1.42".parse().unwrap()));
    }

    #[test]
    fn allows_link_local() {
        assert!(is_allowed_addr("169.254.1.1".parse().unwrap()));
        assert!(is_allowed_addr("fe80::1".parse().unwrap()));
    }

    #[test]
    fn rejects_public_addresses() {
        assert!(!is_allowed_addr("8.8.8.8".parse().unwrap()));
        assert!(!is_allowed_addr("1.1.1.1".parse().unwrap()));
        assert!(!is_allowed_addr("2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn rejects_addresses_just_outside_the_private_ranges() {
        // 172.32.0.0 is one step past the 172.16.0.0/12 block.
        assert!(!is_allowed_addr("172.32.0.1".parse().unwrap()));
        // 192.169.0.0 is one step past the 192.168.0.0/16 block.
        assert!(!is_allowed_addr("192.169.0.1".parse().unwrap()));
    }
}
