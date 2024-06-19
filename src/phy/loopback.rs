use alloc::collections::VecDeque;
use alloc::vec::Vec;

use crate::config::QUEUE_COUNT;
use crate::phy::{self, Device, DeviceCapabilities, Medium};
use crate::time::Instant;
#[cfg(not(feature = "std"))]
use spin::{Mutex, MutexGuard};
#[cfg(feature = "std")]
use std::sync::{Mutex, MutexGuard};

/// A loopback device.
#[derive(Debug)]
pub struct Loopback {
    pub(crate) queue: [Mutex<VecDeque<Vec<u8>>>; QUEUE_COUNT],
    medium: Medium,
}

#[allow(clippy::new_without_default)]
impl Loopback {
    /// Creates a loopback device.
    ///
    /// Every packet transmitted through this device will be received through it
    /// in FIFO order.
    pub fn new(medium: Medium) -> Loopback {
        Loopback {
            queue: Default::default(),
            medium,
        }
    }
}

impl Device for Loopback {
    type RxToken<'a> = RxToken;
    type TxToken<'a> = TxToken<'a>;

    fn capabilities(&self) -> DeviceCapabilities {
        DeviceCapabilities {
            max_transmission_unit: 65535,
            medium: self.medium,
            ..DeviceCapabilities::default()
        }
    }

    fn receive(
        &self,
        _timestamp: Instant,
        queue_id: usize,
    ) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        #[cfg(feature = "std")]
        let mut lock = self.queue[queue_id].lock().unwrap();
        #[cfg(not(feature = "std"))]
        let mut lock = self.queue[queue_id].lock();
        lock.pop_front().map(move |buffer| {
            let rx = RxToken { buffer };
            let tx = TxToken {
                #[cfg(feature = "std")]
                queue: self.queue[queue_id].lock().unwrap(),
                #[cfg(not(feature = "std"))]
                queue: self.queue[queue_id].lock(),
            };
            (rx, tx)
        })
    }

    fn transmit(&self, _timestamp: Instant, queue_id: usize) -> Option<Self::TxToken<'_>> {
        Some(TxToken {
            #[cfg(feature = "std")]
            queue: self.queue[queue_id].lock().unwrap(),
            #[cfg(not(feature = "std"))]
            queue: self.queue[queue_id].lock(),
        })
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
        f(&mut self.buffer)
    }
}

#[doc(hidden)]
#[derive(Debug)]
pub struct TxToken<'a> {
    queue: MutexGuard<'a, VecDeque<Vec<u8>>>,
}

impl<'a> phy::TxToken for TxToken<'a> {
    fn consume<R, F>(mut self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buffer = Vec::new();
        buffer.resize(len, 0);
        let result = f(&mut buffer);
        self.queue.push_back(buffer);
        result
    }
}
