#![allow(clippy::collapsible_if)]

mod utils;

use std::cmp;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::ops::DerefMut;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{wait as phy_wait, Device, Medium};
use smoltcp::socket::tcp;
use smoltcp::socket::AnySocket;
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{EthernetAddress, IpAddress, IpCidr};

const THREADS: usize = 4;
const AMOUNT: usize = 1_000_000_000;

enum Client {
    Reader,
    Writer,
}

fn client(kind: Client, id: usize) {
    let port = match kind {
        Client::Reader => 1234 + 2 * id as u16,
        Client::Writer => 1235 + 2 * id as u16,
    };
    let mut stream = TcpStream::connect(("192.168.69.1", port)).unwrap();
    let mut buffer = vec![0; 1_000_000];

    let start = Instant::now();

    let mut processed = 0;
    while processed < AMOUNT {
        let length = cmp::min(buffer.len(), AMOUNT - processed);
        let result = match kind {
            Client::Reader => stream.read(&mut buffer[..length]),
            Client::Writer => stream.write(&buffer[..length]),
        };
        match result {
            Ok(0) => break,
            Ok(result) => {
                // print!("(P:{})", result);
                // assert!(buffer[..length].iter().all(|x| { *x == 0u8 }));
                processed += result
            }
            Err(err) => panic!("cannot process: {err}"),
        }
    }
    println!("{id}, {processed}");

    let end = Instant::now();

    let elapsed = (end - start).total_millis() as f64 / 1000.0;

    if CLIENT_DONE.fetch_add(1, Ordering::SeqCst) + 1 == THREADS {
        println!(
            "throughput: {:.3} Gbps",
            AMOUNT as f64 / elapsed / 0.125e9 * THREADS as f64
        );
    }
}

static CLIENT_DONE: AtomicUsize = AtomicUsize::new(0);

fn main() {
    #[cfg(feature = "log")]
    utils::setup_logging("info");

    let (mut opts, mut free) = utils::create_options();
    utils::add_tuntap_options(&mut opts, &mut free);
    utils::add_middleware_options(&mut opts, &mut free);
    free.push("MODE");

    let mut matches = utils::parse_options(&opts, free);
    let device = utils::parse_tuntap_options(&mut matches);
    let fd = device.as_raw_fd();
    let mut device =
        utils::parse_middleware_options(&mut matches, device, /*loopback=*/ false);

    let mut config = match device.capabilities().medium {
        Medium::Ethernet => {
            Config::new(EthernetAddress([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]).into())
        }
        Medium::Ip => Config::new(smoltcp::wire::HardwareAddress::Ip),
        Medium::Ieee802154 => todo!(),
    };
    config.random_seed = rand::random();

    let mut iface = Interface::new(config, &mut device, Instant::now());
    iface.update_ip_addrs(|ip_addrs| {
        ip_addrs
            .push(IpCidr::new(IpAddress::v4(192, 168, 69, 1), 24))
            .unwrap();
    });

    let mut tcp1_handles: Vec<SocketHandle> = Vec::new();
    let mut tcp2_handles: Vec<SocketHandle> = Vec::new();
    let mut sockets = SocketSet::new(vec![]);
    for id in 0..THREADS {
        let tcp1_rx_buffer = tcp::SocketBuffer::new(vec![0; 65535]);
        let tcp1_tx_buffer = tcp::SocketBuffer::new(vec![0; 65535]);
        let mut tcp1_socket = tcp::Socket::new(tcp1_rx_buffer, tcp1_tx_buffer);

        let tcp2_rx_buffer = tcp::SocketBuffer::new(vec![0; 65535]);
        let tcp2_tx_buffer = tcp::SocketBuffer::new(vec![0; 65535]);
        let mut tcp2_socket = tcp::Socket::new(tcp2_rx_buffer, tcp2_tx_buffer);
        tcp1_socket.listen(1234 + 2 * id as u16).unwrap();
        tcp2_socket.listen(1235 + 2 * id as u16).unwrap();
        tcp1_handles.push(sockets.add(tcp1_socket));
        tcp2_handles.push(sockets.add(tcp2_socket));
        let mode = match matches.free[0].as_ref() {
            "reader" => Client::Reader,
            "writer" => Client::Writer,
            _ => panic!("invalid mode"),
        };

        thread::spawn(move || client(mode, id.clone()));
    }
    let default_timeout = Some(Duration::from_millis(1000));
    let mut processeds = vec![0 as usize; THREADS];
    while CLIENT_DONE.load(Ordering::SeqCst) != THREADS {
        let timestamp = Instant::now();
        iface.poll(timestamp, &mut device, &mut sockets);

        // tcp:1234: emit data
        for id in 0..THREADS {
            let processed = processeds[id];
            let tcp1_handle = tcp1_handles[id];
            let socket = sockets.get_mut::<tcp::Socket>(tcp1_handle);
            if !socket.is_open() {
                socket.listen(1234 + 2 * id as u16).unwrap();
            }

            if socket.can_send() {
                if processed < AMOUNT {
                    let length = socket
                        .send(|buffer| {
                            let length = cmp::min(buffer.len(), AMOUNT - processed);
                            (length, length)
                        })
                        .unwrap();
                    processeds[id] += length;
                }
            }
        }

        // tcp:1235: sink data
        for id in 0..THREADS {
            let processed = processeds[id];
            let tcp2_handle = tcp2_handles[id];
            let socket = sockets.get_mut::<tcp::Socket>(tcp2_handle);
            if !socket.is_open() {
                socket.listen(1235 + 2 * id as u16).unwrap();
            }

            if socket.can_recv() {
                if processed < AMOUNT {
                    let length = socket
                        .recv(|buffer| {
                            let length = cmp::min(buffer.len(), AMOUNT - processed);
                            // assert!(buffer[..length].iter().all(|x| { *x == 0u8 }));
                            (length, length)
                        })
                        .unwrap();
                    processeds[id] += length;
                }
            }
        }

        match iface.poll_at(timestamp, &sockets) {
            Some(poll_at) if timestamp < poll_at => {
                phy_wait(fd, Some(poll_at - timestamp)).expect("wait error");
            }
            Some(_) => (),
            None => {
                phy_wait(fd, default_timeout).expect("wait error");
            }
        }
    }
}
