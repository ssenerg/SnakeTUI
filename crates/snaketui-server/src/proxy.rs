//! The PROXY protocol, versions 1 and 2: what HAProxy and other load
//! balancers put before a connection's own bytes to say who the client
//! really is. Only connections from [`Trusted`] proxies may use it, and they
//! must.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use anyhow::{Context, Result, bail, ensure};
use tokio::io::{AsyncRead, AsyncReadExt};

/// Version 2 headers start with these 12 bytes.
const V2_SIGNATURE: &[u8; 12] = b"\r\n\r\n\0\r\nQUIT\n";
/// The longest version 1 header, line ending included.
const V1_MAX: usize = 107;

/// The proxies that may say who their clients are: addresses, or networks
/// such as `10.0.0.0/8`.
#[derive(Clone, Debug, Default)]
pub struct Trusted(Vec<(IpAddr, u8)>);

impl Trusted {
    pub fn parse(list: &[String]) -> Result<Self> {
        list.iter()
            .map(|entry| {
                let (ip, bits) = match entry.split_once('/') {
                    Some((ip, bits)) => (ip, Some(bits)),
                    None => (entry.as_str(), None),
                };
                let ip: IpAddr = ip
                    .parse()
                    .with_context(|| format!("{entry:?} isn't an address or network"))?;
                let max = if ip.is_ipv4() { 32 } else { 128 };
                let bits = match bits {
                    Some(bits) => bits
                        .parse()
                        .ok()
                        .filter(|&b| b <= max)
                        .with_context(|| format!("{entry:?} has a bad network size"))?,
                    None => max,
                };
                Ok((ip.to_canonical(), bits))
            })
            .collect::<Result<_>>()
            .map(Self)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = ip.to_canonical();
        self.0.iter().any(|&(net, bits)| match (net, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(bits)).unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = u128::MAX.checked_shl(128 - u32::from(bits)).unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        })
    }
}

/// Reads the PROXY header at the start of `stream`, and not a byte more:
/// the client's address, or `None` for the proxy's own connections, such
/// as health checks.
pub async fn read_header(stream: &mut (impl AsyncRead + Unpin)) -> Result<Option<SocketAddr>> {
    let mut start = [0; 5];
    stream.read_exact(&mut start).await?;
    if &start == b"PROXY" {
        let mut line = start.to_vec();
        while !line.ends_with(b"\r\n") {
            ensure!(line.len() < V1_MAX, "a PROXY line that doesn't end");
            line.push(stream.read_u8().await?);
        }
        return parse_v1(&line);
    }
    ensure!(start == V2_SIGNATURE[..5], "no PROXY header");
    let mut rest = [0; 11];
    stream.read_exact(&mut rest).await?;
    ensure!(rest[..7] == V2_SIGNATURE[5..], "no PROXY header");
    let (version_command, family) = (rest[7], rest[8]);
    let len = u16::from_be_bytes([rest[9], rest[10]]) as usize;
    let mut body = vec![0; len];
    stream.read_exact(&mut body).await?;
    ensure!(
        version_command >> 4 == 2,
        "PROXY version {}",
        version_command >> 4
    );
    match version_command & 0x0f {
        // LOCAL: the proxy's own connection.
        0 => return Ok(None),
        1 => {}
        other => bail!("PROXY command {other}"),
    }
    match family {
        // TCP over IPv4: source, destination, source port, destination port.
        0x11 if len >= 12 => {
            let ip = Ipv4Addr::new(body[0], body[1], body[2], body[3]);
            let port = u16::from_be_bytes([body[8], body[9]]);
            Ok(Some(SocketAddr::new(ip.into(), port)))
        }
        0x21 if len >= 36 => {
            let ip: [u8; 16] = body[..16].try_into().unwrap();
            let port = u16::from_be_bytes([body[32], body[33]]);
            Ok(Some(SocketAddr::new(Ipv6Addr::from(ip).into(), port)))
        }
        // Anything else (UNSPEC, UDP, Unix sockets): no client address.
        _ => Ok(None),
    }
}

/// `PROXY TCP4 <source> <destination> <source port> <destination port>\r\n`,
/// or `PROXY UNKNOWN ...\r\n`.
fn parse_v1(line: &[u8]) -> Result<Option<SocketAddr>> {
    let line = std::str::from_utf8(line).context("a PROXY line that isn't text")?;
    let mut words = line.trim_end().split(' ').skip(1);
    match words.next() {
        Some("TCP4" | "TCP6") => {
            let ip: IpAddr = words.next().context("no source")?.parse()?;
            let _destination = words.next().context("no destination")?;
            let port: u16 = words.next().context("no source port")?.parse()?;
            Ok(Some(SocketAddr::new(ip, port)))
        }
        Some("UNKNOWN") => Ok(None),
        _ => bail!("a PROXY line of an unknown kind"),
    }
}

/// A version 2 header for `client`, as a proxy sends it; for tests.
pub fn v2_header(client: SocketAddr, to: SocketAddr) -> Vec<u8> {
    let mut header = V2_SIGNATURE.to_vec();
    header.push(0x21);
    match (client, to) {
        (SocketAddr::V4(c), SocketAddr::V4(t)) => {
            header.push(0x11);
            header.extend_from_slice(&12u16.to_be_bytes());
            header.extend_from_slice(&c.ip().octets());
            header.extend_from_slice(&t.ip().octets());
            header.extend_from_slice(&c.port().to_be_bytes());
            header.extend_from_slice(&t.port().to_be_bytes());
        }
        _ => {
            let v6 = |a: SocketAddr| match a.ip() {
                IpAddr::V4(ip) => ip.to_ipv6_mapped(),
                IpAddr::V6(ip) => ip,
            };
            header.push(0x21);
            header.extend_from_slice(&36u16.to_be_bytes());
            header.extend_from_slice(&v6(client).octets());
            header.extend_from_slice(&v6(to).octets());
            header.extend_from_slice(&client.port().to_be_bytes());
            header.extend_from_slice(&to.port().to_be_bytes());
        }
    }
    header
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn read(bytes: &[u8]) -> (Result<Option<SocketAddr>>, Vec<u8>) {
        let mut stream = bytes;
        let header = read_header(&mut stream).await;
        (header, stream.to_vec())
    }

    #[actix_rt::test]
    async fn both_versions_give_the_client_and_leave_the_rest() {
        let client: SocketAddr = "203.0.113.7:51000".parse().unwrap();
        let to: SocketAddr = "10.0.0.5:22".parse().unwrap();
        let mut v2 = v2_header(client, to);
        v2.extend_from_slice(b"SSH-2.0-x\r\n");
        let (got, rest) = read(&v2).await;
        assert_eq!(got.unwrap(), Some(client));
        assert_eq!(rest, b"SSH-2.0-x\r\n", "not a byte of SSH taken");

        let six: SocketAddr = "[2001:db8::1]:4000".parse().unwrap();
        let (got, _) = read(&v2_header(six, "[2001:db8::2]:22".parse().unwrap())).await;
        assert_eq!(got.unwrap(), Some(six));

        let (got, rest) = read(b"PROXY TCP4 198.51.100.9 10.0.0.5 4242 22\r\nSSH-").await;
        assert_eq!(got.unwrap(), Some("198.51.100.9:4242".parse().unwrap()));
        assert_eq!(rest, b"SSH-");
        let (got, _) = read(b"PROXY UNKNOWN\r\n").await;
        assert_eq!(got.unwrap(), None);
    }

    #[actix_rt::test]
    async fn anything_else_is_refused() {
        assert!(read(b"SSH-2.0-OpenSSH_9.6\r\n").await.0.is_err());
        assert!(read(&[b'P'; 200]).await.0.is_err());
        assert!(read(b"PROXY TCP4 nonsense\r\n").await.0.is_err());
        let mut long = b"PROXY TCP4 ".to_vec();
        long.extend_from_slice(&[b'1'; 200]);
        assert!(read(&long).await.0.is_err(), "a line with no end");
    }

    #[test]
    fn trusted_proxies_are_addresses_or_networks() {
        let trusted = Trusted::parse(&[
            "10.0.0.2".into(),
            "192.168.0.0/16".into(),
            "fd00::/8".into(),
        ])
        .unwrap();
        assert!(trusted.contains("10.0.0.2".parse().unwrap()));
        assert!(!trusted.contains("10.0.0.3".parse().unwrap()));
        assert!(trusted.contains("192.168.44.1".parse().unwrap()));
        assert!(trusted.contains("::ffff:192.168.1.1".parse().unwrap()));
        assert!(trusted.contains("fd12::7".parse().unwrap()));
        assert!(!trusted.contains("2001:db8::1".parse().unwrap()));
        assert!(Trusted::parse(&["10.0.0.0/33".into()]).is_err());
        assert!(Trusted::parse(&["proxy.internal".into()]).is_err());
        assert!(
            Trusted::parse(&["0.0.0.0/0".into()])
                .unwrap()
                .contains("8.8.8.8".parse().unwrap())
        );
    }
}
