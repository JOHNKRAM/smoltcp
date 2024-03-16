use std::cell::RefCell;
use std::io;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::Rc;
use std::sync::{Mutex, MutexGuard};
use std::vec::Vec;

use crate::config::QUEUE_COUNT;
use crate::phy::{self, sys, Device, DeviceCapabilities, Medium};
use crate::time::Instant;

/// A virtual TUN (IP) or TAP (Ethernet) interface.
#[derive(Debug)]
pub struct TunTapInterface {
    lower: [Mutex<sys::TunTapInterfaceDesc>; QUEUE_COUNT],
    mtu: usize,
    medium: Medium,
}

// impl AsRawFd for TunTapInterface {
//     fn as_raw_fd(&self) -> RawFd {
//         self.lower.borrow().as_raw_fd()
//     }
// }

impl TunTapInterface {
    /// Attaches to a TUN/TAP interface called `name`, or creates it if it does not exist.
    ///
    /// If `name` is a persistent interface configured with UID of the current user,
    /// no special privileges are needed. Otherwise, this requires superuser privileges
    /// or a corresponding capability set on the executable.
    pub fn new(name: &str, medium: Medium) -> io::Result<TunTapInterface> {
        let mtu = sys::TunTapInterfaceDesc::get_mtu(name, medium)?;
        let mut t = TunTapInterface {
            lower: Default::default(),
            mtu,
            medium,
        };
        for i in 0..QUEUE_COUNT {
            *t.lower[i].get_mut().unwrap() = sys::TunTapInterfaceDesc::new(name, medium)?;
        }
        Ok(t)
    }

    // /// Attaches to a TUN/TAP interface specified by file descriptor `fd`.
    // ///
    // /// On platforms like Android, a file descriptor to a tun interface is exposed.
    // /// On these platforms, a TunTapInterface cannot be instantiated with a name.
    // pub fn from_fd(fd: RawFd, medium: Medium, mtu: usize) -> io::Result<TunTapInterface> {
    //     let lower = sys::TunTapInterfaceDesc::from_fd(fd, mtu)?;
    //     Ok(TunTapInterface {
    //         lower: Rc::new(RefCell::new(lower)),
    //         mtu,
    //         medium,
    //     })
    // }

    pub fn as_raw_fd(&self, queue_id: usize) -> RawFd {
        self.lower[queue_id].lock().unwrap().as_raw_fd()
    }
}

impl Device for TunTapInterface {
    type RxToken<'a> = RxToken;
    type TxToken<'a> = TxToken<'a>;

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            max_transmission_unit: self.mtu,
            medium: self.medium,
            ..DeviceCapabilities::default()
        }
    }

    fn receive(
        &self,
        _timestamp: Instant,
        queue_id: usize,
    ) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let lower = self.lower[queue_id].try_lock().ok()?;
        let mut buffer = vec![0; self.mtu];
        match lower.recv(&mut buffer[..]) {
            Ok(size) => {
                buffer.resize(size, 0);
                let rx = RxToken { buffer };
                let tx = TxToken { lower: lower };
                Some((rx, tx))
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => None,
            Err(err) => panic!("{}", err),
        }
    }

    fn transmit(&self, _timestamp: Instant, queue_id: usize) -> Option<Self::TxToken<'_>> {
        self.lower[queue_id]
            .try_lock()
            .ok()
            .map(|lower| TxToken { lower: lower })
    }
}

#[doc(hidden)]
pub struct RxToken {
    buffer: Vec<u8>,
}

impl phy::RxToken for RxToken {
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        f(&mut self.buffer[..])
    }
}

#[doc(hidden)]
pub struct TxToken<'a> {
    lower: MutexGuard<'a, sys::TunTapInterfaceDesc>,
}

impl<'a> phy::TxToken for TxToken<'a> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut lower = self.lower;
        let mut buffer = vec![0; len];
        let result = f(&mut buffer);
        match lower.send(&buffer[..]) {
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                net_debug!("phy: tx failed due to WouldBlock")
            }
            Err(err) => panic!("{}", err),
        }
        result
    }
}
