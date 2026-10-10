//! Packet-capture attribution shared by the Unix samplers.
//!
//! The per-platform device code (macOS `/dev/bpf*`, Linux `AF_PACKET`) only
//! delivers raw frames; everything semantic happens here: strip the datalink
//! header, parse the five-tuple, and credit the packet's bytes to a process
//! through the current socket table.
//!
//! Attribution is honest about being inference: TCP flows are attributed via
//! an exact socket-table lookup, UDP by "the local port belongs to a bound
//! socket of that pid". A port whose previous owner released it makes the
//! next datagram land on the new owner — the sampler tags this entire layer
//! `port-inference` so the output never presents it as kernel accounting.

use std::collections::BTreeMap;
use std::net::IpAddr;

use super::packet::{Datalink, FiveTuple, PROTO_TCP, PROTO_UDP};
use x_core::net_top::PidBytes;

/// One socket row, normalized across platforms.
///
/// macOS and Linux shape their `RawSocket` differently (optional pid, queue
/// fields), so the samplers hand this layer plain values instead: the
/// attribution code should not know which platform's struct it is reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketEntry {
    pub proto: u8,
    pub local_ip: IpAddr,
    pub local_port: u16,
    pub remote: Option<(IpAddr, u16)>,
    pub pid: i32,
}

/// A snapshot of who owns which socket, used to attribute packets.
///
/// Built from one connection-table read per sampling interval, not per packet.
#[derive(Debug, Default)]
pub struct OwnerTable {
    /// (proto, local_ip, local_port) -> pid.
    local: BTreeMap<(u8, IpAddr, u16), i32>,
    /// (proto, local_port) -> pid, for wildcard listeners and UDP fallback.
    local_port: BTreeMap<(u8, u16), i32>,
    /// (proto, local_ip, local_port, remote_ip, remote_port) -> pid. Checked
    /// first for TCP so two connections sharing a local port (rare, but legal
    /// when the remotes differ) go to the right process.
    exact: BTreeMap<(u8, IpAddr, u16, IpAddr, u16), i32>,
}

impl OwnerTable {
    pub fn from_entries(entries: &[SocketEntry]) -> Self {
        let mut table = OwnerTable::default();
        for entry in entries {
            table
                .local
                .insert((entry.proto, entry.local_ip, entry.local_port), entry.pid);
            table
                .local_port
                .entry((entry.proto, entry.local_port))
                .or_insert(entry.pid);
            if let Some((rip, rport)) = entry.remote {
                table.exact.insert(
                    (entry.proto, entry.local_ip, entry.local_port, rip, rport),
                    entry.pid,
                );
            }
        }
        table
    }

    /// The pid a packet's tuple belongs to, or `None` when the flow is
    /// unattributable (kernel traffic, foreign host, closed connection).
    /// Tries "source is local" first, then "destination is local": the host
    /// never owns both ends of a real flow.
    pub fn attribute(&self, tuple: &FiveTuple) -> Option<i32> {
        self.attribute_direction(tuple.src, tuple.src_port, tuple.dst, tuple.dst_port, tuple)
            .or_else(|| {
                self.attribute_direction(
                    tuple.dst,
                    tuple.dst_port,
                    tuple.src,
                    tuple.src_port,
                    tuple,
                )
            })
    }

    fn attribute_direction(
        &self,
        local_ip: IpAddr,
        local_port: u16,
        remote_ip: IpAddr,
        remote_port: u16,
        tuple: &FiveTuple,
    ) -> Option<i32> {
        if let Some(pid) =
            self.exact
                .get(&(tuple.proto, local_ip, local_port, remote_ip, remote_port))
        {
            return Some(*pid);
        }
        if let Some(pid) = self.local.get(&(tuple.proto, local_ip, local_port)) {
            return Some(*pid);
        }
        self.local_port.get(&(tuple.proto, local_port)).copied()
    }

    /// Whether (ip, port) names an endpoint this table has seen as local.
    fn is_local(&self, ip: IpAddr, port: u16) -> bool {
        self.local.contains_key(&(PROTO_TCP, ip, port))
            || self.local.contains_key(&(PROTO_UDP, ip, port))
            || self.local_port.contains_key(&(PROTO_TCP, port))
            || self.local_port.contains_key(&(PROTO_UDP, port))
    }
}

/// Byte counters accumulated between `take()` calls, per pid.
#[derive(Debug, Default)]
pub struct FlowAggregator {
    pending: BTreeMap<i32, PidBytes>,
    baseline_taken: bool,
}

impl FlowAggregator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Credit one captured frame. Unparsable or unattributable frames are
    /// silently dropped: unmapped traffic is derived from host counters minus
    /// the attributed sum, never counted a second time here.
    pub fn credit(&mut self, frame: &[u8], datalink: Datalink, owners: &OwnerTable) {
        let Some(info) = datalink.parse_frame(frame) else {
            return;
        };
        let Some(pid) = owners.attribute(&info.tuple) else {
            return;
        };
        // Outbound TX, inbound RX: whichever endpoint the owner table
        // recognises as local decides the direction.
        let entry = self.pending.entry(pid).or_default();
        if owners.is_local(info.tuple.src, info.tuple.src_port) {
            entry.tx += info.ip_bytes;
        } else {
            entry.rx += info.ip_bytes;
        }
    }

    /// Drain the counters accumulated so far. The first call establishes the
    /// baseline and returns `None` — a rate needs a full interval, and
    /// inventing one from a partial window would overstate it.
    pub fn take(&mut self) -> Option<BTreeMap<i32, PidBytes>> {
        if self.baseline_taken {
            return Some(std::mem::take(&mut self.pending));
        }
        self.baseline_taken = true;
        self.pending.clear();
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn entry(proto: u8, local_port: u16, remote: Option<u16>, wildcard: bool) -> SocketEntry {
        SocketEntry {
            proto,
            local_ip: if wildcard {
                IpAddr::V4(Ipv4Addr::UNSPECIFIED)
            } else {
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5))
            },
            local_port,
            remote: remote.map(|r| (IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)), r)),
            pid: 4242,
        }
    }

    /// IPv4 TCP frame between 10.0.0.5:<local> and 93.184.216.34:<remote>.
    fn tcp_frame(src_local: bool, local: u16, remote: u16, payload_len: u16) -> Vec<u8> {
        let (src, dst): ([u8; 4], [u8; 4]) = if src_local {
            ([10, 0, 0, 5], [93, 184, 216, 34])
        } else {
            ([93, 184, 216, 34], [10, 0, 0, 5])
        };
        let (sport, dport) = if src_local {
            (local, remote)
        } else {
            (remote, local)
        };
        let mut p = vec![0u8; 40];
        p[0] = 0x45;
        p[2..4].copy_from_slice(&(40 + payload_len).to_be_bytes());
        p[9] = PROTO_TCP;
        p[12..16].copy_from_slice(&src);
        p[16..20].copy_from_slice(&dst);
        p[20..22].copy_from_slice(&sport.to_be_bytes());
        p[22..24].copy_from_slice(&dport.to_be_bytes());
        p[32] = 0x50; // TCP data offset 5
        let mut frame = vec![0u8; 14];
        frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        frame.extend_from_slice(&p);
        frame
    }

    #[test]
    fn outbound_tcp_is_attributed_and_counted_as_tx() {
        let owners = OwnerTable::from_entries(&[entry(PROTO_TCP, 51000, Some(443), false)]);
        let mut agg = FlowAggregator::new();
        agg.take(); // baseline

        agg.credit(
            &tcp_frame(true, 51000, 443, 500),
            Datalink::Ethernet,
            &owners,
        );
        let drained = agg.take().unwrap();
        assert_eq!(drained.get(&4242).unwrap().tx, 540);
        assert_eq!(drained.get(&4242).unwrap().rx, 0);
    }

    #[test]
    fn inbound_tcp_is_rx() {
        let owners = OwnerTable::from_entries(&[entry(PROTO_TCP, 51000, Some(443), false)]);
        let mut agg = FlowAggregator::new();
        agg.take();

        agg.credit(
            &tcp_frame(false, 51000, 443, 100),
            Datalink::Ethernet,
            &owners,
        );
        let drained = agg.take().unwrap();
        // ip_bytes counts the whole datagram: 20 IP + 20 TCP + 100 payload.
        assert_eq!(drained.get(&4242).unwrap().rx, 140);
        assert_eq!(drained.get(&4242).unwrap().tx, 0);
    }

    #[test]
    fn unknown_flow_is_dropped_not_attributed() {
        let owners = OwnerTable::from_entries(&[entry(PROTO_TCP, 51000, Some(443), false)]);
        let mut agg = FlowAggregator::new();
        agg.take();
        // Nobody owns local port 55000:
        agg.credit(
            &tcp_frame(true, 55000, 443, 100),
            Datalink::Ethernet,
            &owners,
        );
        let drained = agg.take().unwrap();
        assert!(drained.is_empty());
    }

    #[test]
    fn wildcard_listener_answers_for_any_local_ip() {
        let mut e = entry(PROTO_UDP, 53, None, true);
        e.pid = 777;
        let owners = OwnerTable::from_entries(&[e]);
        let mut agg = FlowAggregator::new();
        agg.take();
        let mut p = vec![0u8; 28]; // IP(20) + UDP(8)
        p[0] = 0x45;
        p[2..4].copy_from_slice(&28u16.to_be_bytes());
        p[9] = PROTO_UDP;
        p[12..16].copy_from_slice(&[10, 9, 9, 9]); // not a configured address
        p[16..20].copy_from_slice(&[10, 0, 0, 5]);
        p[20..22].copy_from_slice(&5353u16.to_be_bytes());
        p[22..24].copy_from_slice(&53u16.to_be_bytes());
        agg.credit(&p, Datalink::RawIp, &owners);
        let drained = agg.take().unwrap();
        assert_eq!(drained.get(&777).unwrap().rx, 28);
    }

    #[test]
    fn first_take_is_a_baseline_and_returns_none() {
        let mut agg = FlowAggregator::new();
        let owners = OwnerTable::from_entries(&[entry(PROTO_TCP, 51000, Some(443), false)]);
        agg.credit(
            &tcp_frame(true, 51000, 443, 10),
            Datalink::Ethernet,
            &owners,
        );
        assert!(agg.take().is_none());
        agg.credit(
            &tcp_frame(true, 51000, 443, 10),
            Datalink::Ethernet,
            &owners,
        );
        assert!(agg.take().is_some());
    }
}
