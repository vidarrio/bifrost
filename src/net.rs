//! Address helpers shared by forward-auth (trusting the proxy) and rate
//! limiting (finding the real client behind it).

use std::net::IpAddr;

/// An IP network, e.g. `10.0.0.0/24` (a bare address is a /32 or /128).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cidr {
    net: IpAddr,
    bits: u8,
}

impl Cidr {
    pub fn parse(s: &str) -> Result<Cidr, String> {
        let (ip, bits) = match s.split_once('/') {
            Some((ip, bits)) => (ip, Some(bits)),
            None => (s, None),
        };
        let net: IpAddr = ip.parse().map_err(|_| format!("bad address {s:?}"))?;
        let max = if net.is_ipv4() { 32 } else { 128 };
        let bits = match bits {
            Some(b) => b
                .parse::<u8>()
                .ok()
                .filter(|&b| b <= max)
                .ok_or(format!("bad prefix in {s:?}"))?,
            None => max,
        };
        Ok(Cidr { net, bits })
    }

    /// Parse a comma-separated list.
    pub fn parse_list(list: &str) -> Result<Vec<Cidr>, String> {
        list.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(Cidr::parse)
            .collect()
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        // An IPv4 peer may arrive IPv4-mapped on a dual-stack socket.
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
            v4 => v4,
        };
        match (self.net, ip) {
            (IpAddr::V4(n), IpAddr::V4(a)) => {
                let mask = u32::MAX.checked_shl(32 - self.bits as u32).unwrap_or(0);
                u32::from(n) & mask == u32::from(a) & mask
            }
            (IpAddr::V6(n), IpAddr::V6(a)) => {
                let mask = u128::MAX.checked_shl(128 - self.bits as u32).unwrap_or(0);
                u128::from(n) & mask == u128::from(a) & mask
            }
            _ => false,
        }
    }
}

/// The unit a client is counted as for rate limits and login backoff: its
/// IPv4 address, or its IPv6 /64 (one client usually holds a whole /64, so
/// counting single IPv6 addresses wouldn't bound it).
pub fn client_group(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6((u128::from(v6) & !(u64::MAX as u128)).into()),
        },
        v4 => v4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clients_group_by_ipv4_address_or_ipv6_64() {
        let g = |s: &str| client_group(s.parse().unwrap()).to_string();
        assert_eq!(g("192.0.2.7"), "192.0.2.7");
        assert_eq!(g("::ffff:192.0.2.7"), "192.0.2.7");
        assert_eq!(g("2001:db8:1:2:aaaa::1"), "2001:db8:1:2::");
        assert_eq!(g("2001:db8:1:2:ffff:ffff:ffff:ffff"), "2001:db8:1:2::");
        assert_ne!(g("2001:db8:1:3::1"), g("2001:db8:1:2::1"));
    }

    #[test]
    fn cidrs() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let c = Cidr::parse("10.0.0.0/24").unwrap();
        assert!(c.contains(ip("10.0.0.86")));
        assert!(c.contains(ip("::ffff:10.0.0.86")));
        assert!(!c.contains(ip("10.0.1.1")));
        assert!(Cidr::parse("10.0.0.1").unwrap().contains(ip("10.0.0.1")));
        assert!(Cidr::parse("0.0.0.0/0").unwrap().contains(ip("8.8.8.8")));
        assert!(Cidr::parse("10.0.0.0/33").is_err());
        assert!(Cidr::parse("::1").is_ok());
        assert_eq!(Cidr::parse_list(" 10.0.0.0/8, ,::1 ").unwrap().len(), 2);
    }
}
