//! IP/TCP/UDP header parsing for the packet-capture sampler.
//!
//! The capture path on both Unix platforms (macOS `/dev/bpf*`, Linux
//! `AF_PACKET`) hands us raw packets. This module decodes just enough of the
//! IP and transport headers to build the five-tuple the attribution layer
//! needs, and states the byte count each packet contributes. Anything that is
//! not a complete IPv4/IPv6 TCP or UDP datagram (fragments, ICMP, mangled
//! headers) is skipped: an unreadable packet is not a counted packet.

use std::net::{Ipv4Addr, Ipv6Addr};

/// Transport protocol numbers used here and by the socket tables.
pub const PROTO_TCP: u8 = 6;
pub const PROTO_UDP: u8 = 17;

/// The addressing information one packet carries after header parsing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FiveTuple {
    /// `PROTO_TCP` or `PROTO_UDP`; other protocols never reach attribution.
    pub proto: u8,
    pub src: std::net::IpAddr,
    pub dst: std::net::IpAddr,
    pub src_port: u16,
    pub dst_port: u16,
}

/// IP header parsing outcome: the tuple plus the full IP-layer length in
/// bytes (what the interface counters would have counted for this packet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketInfo {
    pub tuple: FiveTuple,
    /// ip.total_length (IPv4) or payload_class length (IPv6), including the
    /// IP header itself. This is the number credited to the flow.
    pub ip_bytes: u64,
}

/// Parse one IP datagram (no link-layer prefix — callers strip that first).
pub fn parse_ip(packet: &[u8]) -> Option<PacketInfo> {
    if packet.is_empty() {
        return None;
    }
    match packet[0] >> 4 {
        4 => parse_ipv4(packet),
        6 => parse_ipv6(packet),
        _ => None,
    }
}

fn parse_ipv4(packet: &[u8]) -> Option<PacketInfo> {
    if packet.len() < 20 {
        return None;
    }
    let ihl = (packet[0] & 0x0f) as usize * 4;
    if ihl < 20 || packet.len() < ihl {
        return None;
    }
    let total_len = u16::from_be_bytes([packet[2], packet[3]]) as usize;
    // A short capture buffer still credits the datagram's declared length:
    // the bytes existed on the wire even if the BPF buffer kept only a prefix.
    let ip_bytes = total_len.max(ihl) as u64;
    let proto = packet[9];
    let src = std::net::IpAddr::V4(Ipv4Addr::new(
        packet[12], packet[13], packet[14], packet[15],
    ));
    let dst = std::net::IpAddr::V4(Ipv4Addr::new(
        packet[16], packet[17], packet[18], packet[19],
    ));
    let (src_port, dst_port) = transport_ports(packet, ihl, proto)?;
    Some(PacketInfo {
        tuple: FiveTuple {
            proto,
            src,
            dst,
            src_port,
            dst_port,
        },
        ip_bytes,
    })
}

fn parse_ipv6(packet: &[u8]) -> Option<PacketInfo> {
    if packet.len() < 40 {
        return None;
    }
    let payload_len = u16::from_be_bytes([packet[4], packet[5]]) as usize;
    let proto = packet[6];
    let src = std::net::IpAddr::V6(Ipv6Addr::new(
        u16::from_be_bytes([packet[8], packet[9]]),
        u16::from_be_bytes([packet[10], packet[11]]),
        u16::from_be_bytes([packet[12], packet[13]]),
        u16::from_be_bytes([packet[14], packet[15]]),
        u16::from_be_bytes([packet[16], packet[17]]),
        u16::from_be_bytes([packet[18], packet[19]]),
        u16::from_be_bytes([packet[20], packet[21]]),
        u16::from_be_bytes([packet[22], packet[23]]),
    ));
    let dst = std::net::IpAddr::V6(Ipv6Addr::new(
        u16::from_be_bytes([packet[24], packet[25]]),
        u16::from_be_bytes([packet[26], packet[27]]),
        u16::from_be_bytes([packet[28], packet[29]]),
        u16::from_be_bytes([packet[30], packet[31]]),
        u16::from_be_bytes([packet[32], packet[33]]),
        u16::from_be_bytes([packet[34], packet[35]]),
        u16::from_be_bytes([packet[36], packet[37]]),
        u16::from_be_bytes([packet[38], packet[39]]),
    ));
    let (src_port, dst_port) = transport_ports(packet, 40, proto)?;
    Some(PacketInfo {
        tuple: FiveTuple {
            proto,
            src,
            dst,
            src_port,
            dst_port,
        },
        ip_bytes: (40 + payload_len) as u64,
    })
}

/// TCP and UDP both carry the two ports as the first four bytes of their
/// header; the extension headers of IPv6 are not followed (a fragmented or
/// extended datagram is skipped rather than misattributed).
fn transport_ports(packet: &[u8], transport_offset: usize, proto: u8) -> Option<(u16, u16)> {
    if proto != PROTO_TCP && proto != PROTO_UDP {
        return None;
    }
    let header = packet.get(transport_offset..transport_offset + 4)?;
    let src_port = u16::from_be_bytes([header[0], header[1]]);
    let dst_port = u16::from_be_bytes([header[2], header[3]]);
    Some((src_port, dst_port))
}

/// Strip the datalink prefix the capture device prepends.
///
/// * `DltNull` — loopback on macOS: a 4-byte family field.
/// * `Ethernet` — regular interfaces on macOS: a 14-byte ethernet header.
/// * Linux `AF_PACKET` bound with `ETH_P_IP` delivers bare IP datagrams, so
///   callers there pass [`Datalink::RawIp`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Datalink {
    RawIp,
    DltNull,
    Ethernet,
}

impl Datalink {
    pub fn header_len(self) -> usize {
        match self {
            Datalink::RawIp => 0,
            Datalink::DltNull => 4,
            Datalink::Ethernet => 14,
        }
    }

    /// Parse one captured frame, skipping the link-layer prefix.
    pub fn parse_frame(self, frame: &[u8]) -> Option<PacketInfo> {
        let header = self.header_len();
        let packet = frame.get(header..)?;
        // DLT_NULL carries the address family in the first four bytes; only
        // IPv4 (2) and IPv6 (24/28 variants) matter, everything else skips.
        if self == Datalink::DltNull {
            if frame.len() < 4 {
                return None;
            }
            let family = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
            match family {
                2 | 24 | 28 | 30 => {}
                _ => return None,
            }
        }
        parse_ip(packet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    /// A minimal IPv4 TCP datagram: 20-byte IP header + 4 bytes of TCP ports.
    fn ipv4_tcp_packet(total_len: u16) -> Vec<u8> {
        let mut p = vec![0u8; 24];
        p[0] = 0x45; // version 4, IHL 5
        p[2..4].copy_from_slice(&total_len.to_be_bytes());
        p[9] = PROTO_TCP;
        p[12..16].copy_from_slice(&[10, 0, 0, 1]);
        p[16..20].copy_from_slice(&[10, 0, 0, 2]);
        p[20..22].copy_from_slice(&1234u16.to_be_bytes());
        p[22..24].copy_from_slice(&443u16.to_be_bytes());
        p
    }

    #[test]
    fn parses_ipv4_tcp_tuple() {
        let info = parse_ip(&ipv4_tcp_packet(24)).unwrap();
        assert_eq!(info.tuple.proto, PROTO_TCP);
        assert_eq!(info.tuple.src, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
        assert_eq!(info.tuple.dst, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)));
        assert_eq!(info.tuple.src_port, 1234);
        assert_eq!(info.tuple.dst_port, 443);
        assert_eq!(info.ip_bytes, 24);
    }

    #[test]
    fn credits_declared_length_even_when_buffer_is_short() {
        // A full-size datagram whose capture kept only the headers: the flow
        // carried total_len bytes on the wire.
        let full = ipv4_tcp_packet(1500);
        let truncated = &full[..24];
        assert_eq!(parse_ip(truncated).unwrap().ip_bytes, 1500);
    }

    #[test]
    fn parses_ipv6_udp_tuple() {
        let mut p = vec![0u8; 44];
        p[0] = 0x60;
        p[4..6].copy_from_slice(&8u16.to_be_bytes()); // payload length
        p[6] = PROTO_UDP;
        p[8..24].fill(0xAA);
        p[24..40].fill(0xBB);
        p[40..42].copy_from_slice(&5353u16.to_be_bytes());
        p[42..44].copy_from_slice(&53u16.to_be_bytes());
        let info = parse_ip(&p).unwrap();
        assert_eq!(info.tuple.proto, PROTO_UDP);
        assert_eq!(info.tuple.src_port, 5353);
        assert_eq!(info.tuple.dst_port, 53);
        assert_eq!(info.ip_bytes, 48);
    }

    #[test]
    fn skips_non_transport_protocols() {
        let mut p = ipv4_tcp_packet(24);
        p[9] = 1; // ICMP
        assert!(parse_ip(&p).is_none());
    }

    #[test]
    fn dlt_null_strips_family_and_filters_non_ip() {
        let mut frame = vec![0u8; 4];
        frame[0] = 2; // AF_INET
        frame.extend_from_slice(&ipv4_tcp_packet(24));
        let info = Datalink::DltNull.parse_frame(&frame).unwrap();
        assert_eq!(info.tuple.src_port, 1234);

        frame[0] = 18; // AF_LINK: not an IP datagram
        assert!(Datalink::DltNull.parse_frame(&frame).is_none());
    }

    #[test]
    fn ethernet_strips_fourteen_byte_header() {
        let mut frame = vec![0u8; 14];
        frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes()); // ethertype IPv4
        frame.extend_from_slice(&ipv4_tcp_packet(24));
        let info = Datalink::Ethernet.parse_frame(&frame).unwrap();
        assert_eq!(info.tuple.dst_port, 443);
    }

    #[test]
    fn raw_ip_passes_through() {
        let info = Datalink::RawIp.parse_frame(&ipv4_tcp_packet(24)).unwrap();
        assert_eq!(info.ip_bytes, 24);
    }

    #[test]
    fn truncated_headers_are_skipped_not_panicked_on() {
        assert!(parse_ip(&[0x45]).is_none());
        assert!(parse_ip(&[]).is_none());
        assert!(parse_ip(&[0x60, 0, 0, 0]).is_none());
        // TCP header cut short:
        let mut p = ipv4_tcp_packet(24);
        p.truncate(22);
        assert!(parse_ip(&p).is_none());
    }
}
