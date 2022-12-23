use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

use crossbeam::utils::Backoff;
use serde::Serialize;

/// A cross-thread communications channel built on Crossbeam's Sender and Receiver,
/// which has a limited capacity based on the (rough!) memory usage of the messages
/// in the channel.
/// This is needed because even though crossbeam provides a bounded capacity channel,
/// it is based on the number of messages in the channel, not the size of the messages.
/// Because our goal with this is to limit memory usage, and our messages have varying sizes
/// (some contain file contents, and some don't), we can't use a bounded channel for this without
/// setting a very conservative capacity, which might reduce performance in the case where
/// all the messages are small and so a large capacity would have been fine.
pub fn new<T>(memory_capacity: usize) -> (Sender<T>, Receiver<T>) {
    // Create crossbeam channels for the underlying logic, then wrap them in our own structs,
    // along with a shared counter for memory usage.
    // We send the memory usage along with each message, so the receiving end doesn't need to re-calculate this
    let (s, r) = crossbeam::channel::unbounded::<(T, usize)>();
    let counter = Arc::new(AtomicUsize::new(0));
    (
        Sender::<T> { inner: s, memory_capacity, channel_memory_usage: counter.clone() },
        Receiver::<T> { inner: r, channel_memory_usage: counter },
    )
}



pub struct Sender<T> {
    inner: crossbeam::channel::Sender<(T, usize)>,
    memory_capacity: usize,
    channel_memory_usage: Arc<AtomicUsize>,
}

impl<T: Serialize> Sender<T> {
    /// Blocks if there is insufficient memory available in the channel.
    pub fn send(&self, msg: T) -> Result<(), crossbeam::channel::SendError<T>> {
        // Get the rough memory size of the message, by seeing how many bytes it would take to serialize it
        let memory_usage = bincode::serialized_size(&msg).expect("Error in serialized_size") as usize;

        // Note that we increment the counter _before_ we block for space, so that we only need
        // to do one atomic operation rather than two (in the common case that there is space).
        let old_usage = self.channel_memory_usage.fetch_add(memory_usage, Ordering::Relaxed);
        // Block if necessary, using crossbeam's exponential backoff utility
        // Note that we compare the old usage, not the new one, so that no matter how big a single message is,
        // we will always send it. Otherwise a large message might block forever waiting for space that can never
        // be available! This does mean that we can exceed the capacity, but we don't need a hard limit so this is fine.
        if old_usage > self.memory_capacity {
            let backoff = Backoff::new();
            while self.channel_memory_usage.load(Ordering::Relaxed) - memory_usage > self.memory_capacity {
                backoff.snooze();
            }
        }

        self.inner.send((msg, memory_usage)).map_err(|e| crossbeam::channel::SendError::<T>(e.0.0))
    }
}



pub struct Receiver<T> {
    inner: crossbeam::channel::Receiver<(T, usize)>,
    channel_memory_usage: Arc<AtomicUsize>,
}

impl<T> Receiver<T> {
    pub fn recv(&self) -> Result<T, crossbeam::channel::RecvError> {
        let (msg, memory_usage) = self.inner.recv()?;

        // Reduce the memory usage counter, which may unblock a sender
        self.channel_memory_usage.fetch_sub(memory_usage, Ordering::Relaxed);

        Ok(msg)
    }

    pub fn try_recv(&self) -> Result<T, crossbeam::channel::TryRecvError> {
        let (msg, memory_usage) = self.inner.try_recv()?;

        // Reduce the memory usage counter, which may unblock a sender
        self.channel_memory_usage.fetch_sub(memory_usage, Ordering::Relaxed);

        Ok(msg)
    }
}

/// Limited version of crossbeam::Select. We can't expose the underlying crossbeam channels
/// as it wouldn't maintain our memory usage counters, so instead we wrap it.
pub fn select_ready<R> (r1: &Receiver<R>, r2: &Receiver<R>) -> usize {
    let mut s = crossbeam::channel::Select::new();
    s.recv(&r1.inner);
    s.recv(&r2.inner);
    s.ready()
}