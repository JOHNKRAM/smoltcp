use super::*;
use core::sync::atomic::Ordering::Relaxed;

#[cfg(feature = "socket-dns")]
use crate::socket::dns::Socket as DnsSocket;

#[cfg(feature = "socket-udp")]
use crate::socket::udp::Socket as UdpSocket;

impl InterfaceInner {
    pub(super) fn process_udp<'frame>(
        &self,
        sockets: &SocketSet,
        meta: PacketMeta,
        handled_by_raw_socket: bool,
        ip_repr: IpRepr,
        ip_payload: &'frame [u8],
        queue_id: usize,
    ) -> Option<Packet<'frame>> {
        let (src_addr, dst_addr) = (ip_repr.src_addr(), ip_repr.dst_addr());
        let udp_packet = check!(UdpPacket::new_checked(ip_payload));
        let udp_repr = check!(UdpRepr::parse(
            &udp_packet,
            &src_addr,
            &dst_addr,
            &self.caps.checksum
        ));

        #[cfg(feature = "socket-udp")]
        for udp_socket in sockets.items() {
            if let Some(socket) = udp_socket.socket.try_read().ok() {
                if let Some(socket) = UdpSocket::downcast(&socket) {
                    if !socket.accepts(self, &ip_repr, &udp_repr) {
                        continue;
                    }
                } else {
                    continue;
                }
            } else {
                continue;
            }
            udp_socket.queue_id.store(queue_id, Relaxed);
            let mut socket = udp_socket.socket.write().unwrap();
            let udp_socket = UdpSocket::downcast_mut(&mut socket).unwrap();
            udp_socket.process(self, meta, &ip_repr, &udp_repr, udp_packet.payload());
            return None;
        }

        #[cfg(feature = "socket-dns")]
        for dns_socket in sockets.items() {
            if let Some(socket) = dns_socket.socket.try_read().ok() {
                if let Some(socket) = DnsSocket::downcast(&socket) {
                    if !socket.accepts(&ip_repr, &udp_repr) {
                        continue;
                    }
                } else {
                    continue;
                }
            } else {
                continue;
            }
            dns_socket.queue_id.store(queue_id, Relaxed);
            let mut socket = dns_socket.socket.write().unwrap();
            let dns_socket = DnsSocket::downcast_mut(&mut socket).unwrap();
            dns_socket.process(self, &ip_repr, &udp_repr, udp_packet.payload());
            return None;
        }

        // The packet wasn't handled by a socket, send an ICMP port unreachable packet.
        match ip_repr {
            #[cfg(feature = "proto-ipv4")]
            IpRepr::Ipv4(_) if handled_by_raw_socket => None,
            #[cfg(feature = "proto-ipv6")]
            IpRepr::Ipv6(_) if handled_by_raw_socket => None,
            #[cfg(feature = "proto-ipv4")]
            IpRepr::Ipv4(ipv4_repr) => {
                let payload_len =
                    icmp_reply_payload_len(ip_payload.len(), IPV4_MIN_MTU, ipv4_repr.buffer_len());
                let icmpv4_reply_repr = Icmpv4Repr::DstUnreachable {
                    reason: Icmpv4DstUnreachable::PortUnreachable,
                    header: ipv4_repr,
                    data: &ip_payload[0..payload_len],
                };
                self.icmpv4_reply(ipv4_repr, icmpv4_reply_repr)
            }
            #[cfg(feature = "proto-ipv6")]
            IpRepr::Ipv6(ipv6_repr) => {
                let payload_len =
                    icmp_reply_payload_len(ip_payload.len(), IPV6_MIN_MTU, ipv6_repr.buffer_len());
                let icmpv6_reply_repr = Icmpv6Repr::DstUnreachable {
                    reason: Icmpv6DstUnreachable::PortUnreachable,
                    header: ipv6_repr,
                    data: &ip_payload[0..payload_len],
                };
                self.icmpv6_reply(ipv6_repr, icmpv6_reply_repr)
            }
        }
    }
}
