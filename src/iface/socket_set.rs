use core::{
    fmt,
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
};
use managed::ManagedSlice;

use super::socket_meta::Meta;
use crate::{
    config::QUEUE_COUNT,
    socket::{AnySocket, Socket},
};

#[cfg(not(feature = "std"))]
use spin::{RwLock, RwLockReadGuard, RwLockWriteGuard};
#[cfg(feature = "std")]
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Opaque struct with space for storing one socket.
///
/// This is public so you can use it to allocate space for storing
/// sockets when creating an Interface.
#[derive(Debug, Default)]
pub struct SocketStorage<'a> {
    inner: Option<Item<'a>>,
}

impl<'a> SocketStorage<'a> {
    pub const EMPTY: Self = Self { inner: None };
}

/// An item of a socket set.
#[derive(Debug)]
pub struct Item<'a> {
    pub meta: RwLock<Meta>,
    pub queue_id: AtomicUsize,
    pub socket: RwLock<Socket<'a>>,
}

impl<'a> Item<'a> {
    pub fn socket_read(&self) -> RwLockReadGuard<Socket<'a>> {
        #[cfg(feature = "std")]
        return self.socket.read().unwrap();
        #[cfg(not(feature = "std"))]
        return self.socket.read();
    }

    pub fn socket_write(&self) -> RwLockWriteGuard<Socket<'a>> {
        #[cfg(feature = "std")]
        return self.socket.write().unwrap();
        #[cfg(not(feature = "std"))]
        return self.socket.write();
    }

    pub fn socket_try_read(&self) -> Option<RwLockReadGuard<Socket<'a>>> {
        #[cfg(feature = "std")]
        return self.socket.try_read().ok();
        #[cfg(not(feature = "std"))]
        return self.socket.try_read();
    }

    pub fn socket_into_inner(self) -> Socket<'a> {
        #[cfg(feature = "std")]
        return self.socket.into_inner().unwrap();
        #[cfg(not(feature = "std"))]
        return self.socket.into_inner();
    }

    pub fn meta_read(&self) -> RwLockReadGuard<Meta> {
        #[cfg(feature = "std")]
        return self.meta.read().unwrap();
        #[cfg(not(feature = "std"))]
        return self.meta.read();
    }

    pub fn meta_write(&self) -> RwLockWriteGuard<Meta> {
        #[cfg(feature = "std")]
        return self.meta.write().unwrap();
        #[cfg(not(feature = "std"))]
        return self.meta.write();
    }
}

/// A handle, identifying a socket in an Interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct SocketHandle(usize);

impl fmt::Display for SocketHandle {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// An extensible set of sockets.
///
/// The lifetime `'a` is used when storing a `Socket<'a>`.  If you're using
/// owned buffers for your sockets (passed in as `Vec`s) you can use
/// `SocketSet<'static>`.
#[derive(Debug)]
pub struct SocketSet<'a> {
    sockets: ManagedSlice<'a, SocketStorage<'a>>, //内部控制
    counter: AtomicUsize,
}

impl<'a> SocketSet<'a> {
    /// Create a socket set using the provided storage.
    pub fn new<SocketsT>(sockets: SocketsT) -> SocketSet<'a>
    where
        SocketsT: Into<ManagedSlice<'a, SocketStorage<'a>>>,
    {
        let sockets = sockets.into();
        SocketSet {
            sockets,
            counter: AtomicUsize::new(0),
        }
    }

    /// Add a socket to the set, and return its handle.
    ///
    /// # Panics
    /// This function panics if the storage is fixed-size (not a `Vec`) and is full.
    pub fn add<T: AnySocket<'a>>(&mut self, socket: T) -> SocketHandle {
        fn put<'a>(
            index: usize,
            slot: &mut SocketStorage<'a>,
            socket: Socket<'a>,
            queue_id: usize,
        ) -> SocketHandle {
            net_trace!("[{}]: adding", index);
            let handle = SocketHandle(index);
            let mut meta = Meta::default();
            meta.handle = handle;
            *slot = SocketStorage {
                inner: Some(Item {
                    meta: RwLock::new(meta),
                    queue_id: AtomicUsize::new(queue_id),
                    socket: RwLock::new(socket),
                }),
            };
            handle
        }

        let socket = socket.upcast();

        for (index, slot) in self.sockets.iter_mut().enumerate() {
            if slot.inner.is_none() {
                return put(
                    index,
                    slot,
                    socket,
                    self.counter.fetch_add(1, Relaxed) % QUEUE_COUNT,
                );
            }
        }

        match &mut self.sockets {
            ManagedSlice::Borrowed(_) => panic!("adding a socket to a full SocketSet"),
            #[cfg(feature = "alloc")]
            ManagedSlice::Owned(sockets) => {
                sockets.push(SocketStorage { inner: None });
                let index = sockets.len() - 1;
                put(
                    index,
                    &mut sockets[index],
                    socket,
                    self.counter.fetch_add(1, Relaxed) % QUEUE_COUNT,
                )
            }
        }
    }

    // /// Get a socket from the set by its handle, as mutable.
    // ///
    // /// # Panics
    // /// This function may panic if the handle does not belong to this socket set
    // /// or the socket has the wrong type.
    // pub fn get<T: AnySocket<'a>>(&self, handle: SocketHandle) -> &T {
    //     match self.sockets[handle.0].inner.as_ref() {
    //         Some(item) => {
    //             T::downcast(&item.socket).expect("handle refers to a socket of a wrong type")
    //         }
    //         None => panic!("handle does not refer to a valid socket"),
    //     }
    // }

    /// Get a mutable socket from the set by its handle, as mutable.
    ///
    /// # Panics
    /// This function may panic if the handle does not belong to this socket set
    /// or the socket has the wrong type.
    pub fn get_mut<T: AnySocket<'a>>(&self, handle: SocketHandle) -> RwLockWriteGuard<Socket<'a>> {
        match self.sockets[handle.0].inner.as_ref() {
            #[cfg(feature = "std")]
            Some(item) => item.socket_write(),
            #[cfg(not(feature = "std"))]
            Some(item) => item.socket.write(),
            None => panic!("handle does not refer to a valid socket"),
        }
    }

    /// Remove a socket from the set, without changing its state.
    ///
    /// # Panics
    /// This function may panic if the handle does not belong to this socket set.
    pub fn remove(&mut self, handle: SocketHandle) -> Socket<'a> {
        net_trace!("[{}]: removing", handle.0);
        match self.sockets[handle.0].inner.take() {
            #[cfg(feature = "std")]
            Some(item) => item.socket_into_inner(),
            #[cfg(not(feature = "std"))]
            Some(item) => item.socket.into_inner(),
            None => panic!("handle does not refer to a valid socket"),
        }
    }

    // /// Get an iterator to the inner sockets.
    // pub fn iter(&self) -> impl Iterator<Item = (SocketHandle, &Socket<'a>)> {
    //     self.items().map(|i| (i.meta.handle, &i.socket))
    // }

    // /// Get a mutable iterator to the inner sockets.
    // pub fn iter_mut(&mut self) -> impl Iterator<Item = (SocketHandle, &mut Socket<'a>)> {
    //     self.items_mut().map(|i| (i.meta.handle, &mut i.socket))
    // }

    /// Iterate every socket in this set.
    pub fn items(&self) -> impl Iterator<Item = &Item<'a>> + '_ {
        self.sockets.iter().filter_map(|x| x.inner.as_ref())
    }

    /// Iterate every socket in this set.
    pub(crate) fn items_mut(&mut self) -> impl Iterator<Item = &mut Item<'a>> + '_ {
        self.sockets.iter_mut().filter_map(|x| x.inner.as_mut())
    }
}
