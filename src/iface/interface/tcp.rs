use log::info;

use super::*;
use core::sync::atomic::Ordering::Relaxed;

use crate::socket::tcp::Socket;

impl InterfaceInner {
    pub(crate) fn process_tcp<'frame>(
        &self,
        sockets: &SocketSet,
        ip_repr: IpRepr,
        ip_payload: &'frame [u8],
        queue_id: usize,
        now: Instant,
    ) -> Option<Packet<'frame>> {
        let (src_addr, dst_addr) = (ip_repr.src_addr(), ip_repr.dst_addr());
        let tcp_packet = check!(TcpPacket::new_checked(ip_payload));
        let tcp_repr = check!(TcpRepr::parse(
            &tcp_packet,
            &src_addr,
            &dst_addr,
            &self.caps.checksum
        ));

        for tcp_socket in sockets.items() {
            if let Some(socket) = tcp_socket.socket.try_read().ok() {
                if let Some(socket) = Socket::downcast(&socket) {
                    if !socket.accepts(self, &ip_repr, &tcp_repr) {
                        continue;
                    }
                } else {
                    continue;
                }
            } else {
                continue;
            }
            let old_queue_id = tcp_socket.queue_id.swap(queue_id, Relaxed);
            if old_queue_id != queue_id {
                info!(
                    "socket {} queue {} {}",
                    tcp_socket.meta.read().unwrap().handle,
                    old_queue_id,
                    queue_id
                );
            }
            let mut socket = tcp_socket.socket.write().unwrap();
            let tcp_socket = Socket::downcast_mut(&mut socket).unwrap();
            return tcp_socket
                .process(self, &ip_repr, &tcp_repr, now)
                .map(|(ip, tcp)| Packet::new(ip, IpPayload::Tcp(tcp)));
        }

        if tcp_repr.control == TcpControl::Rst
            || ip_repr.dst_addr().is_unspecified()
            || ip_repr.src_addr().is_unspecified()
        {
            // Never reply to a TCP RST packet with another TCP RST packet. We also never want to
            // send a TCP RST packet with unspecified addresses.
            None
        } else {
            // The packet wasn't handled by a socket, send a TCP RST packet.
            let (ip, tcp) = tcp::Socket::rst_reply(&ip_repr, &tcp_repr);
            Some(Packet::new(ip, IpPayload::Tcp(tcp)))
        }
    }
}
