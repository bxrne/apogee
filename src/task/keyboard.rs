//! Asynchronous keyboard input pipeline.
//!
//! The hardware keyboard interrupt fires from inside an interrupt handler
//! that *must not* allocate or block. To keep the handler tiny we split the
//! work in two:
//!
//! 1. The interrupt handler reads the scancode byte and pushes it onto
//!    [`SCANCODE_QUEUE`] via [`add_scancode`].
//! 2. The asynchronous [`print_keypresses`] task pops scancodes from a
//!    [`ScancodeStream`] (which implements [`Stream`]) and feeds them into
//!    the `pc-keyboard` decoder.
//!
//! [`AtomicWaker`] is used to wake the consuming task whenever the producer
//! pushes a new byte, so the executor only re-polls the task when there is
//! actually work to do.

use core::pin::Pin;
use core::task::{Context, Poll};

use conquer_once::spin::OnceCell;
use crossbeam_queue::ArrayQueue;
use futures_util::stream::Stream;
use futures_util::stream::StreamExt;
use futures_util::task::AtomicWaker;
use pc_keyboard::{DecodedKey, HandleControl, Keyboard, ScancodeSet1, layouts};

use crate::{print, println};

/// Bounded queue of raw scancodes shared between the interrupt handler and
/// the async keyboard task. Initialised lazily by [`ScancodeStream::new`].
static SCANCODE_QUEUE: OnceCell<ArrayQueue<u8>> = OnceCell::uninit();

/// Capacity of [`SCANCODE_QUEUE`]. 100 entries is more than enough for
/// realistic typing rates.
const SCANCODE_QUEUE_CAPACITY: usize = 100;

/// The waker registered by [`ScancodeStream::poll_next`] when the queue is
/// empty. [`add_scancode`] wakes it after every successful push.
static WAKER: AtomicWaker = AtomicWaker::new();

/// Pushes a scancode onto the global queue from the keyboard interrupt
/// handler.
///
/// **Must not block or allocate** — it is invoked from inside an interrupt
/// handler. If the queue is uninitialised or full, the scancode is dropped
/// and a warning is printed.
pub(crate) fn add_scancode(scancode: u8) {
    if let Ok(queue) = SCANCODE_QUEUE.try_get() {
        if queue.push(scancode).is_err() {
            println!("WARNING: scancode queue full; dropping keyboard input");
        } else {
            // Wake the consuming task *after* a successful push so it sees
            // the new byte on its next poll.
            WAKER.wake();
        }
    } else {
        println!("WARNING: scancode queue uninitialized");
    }
}

/// Asynchronous stream of raw keyboard scancodes.
///
/// Construction lazily initialises [`SCANCODE_QUEUE`]; only one instance may
/// exist per boot, enforced by panicking on a second `new()` call.
pub struct ScancodeStream {
    /// Zero-sized field whose only purpose is to prevent external callers
    /// from instantiating the struct directly — they must go through
    /// [`ScancodeStream::new`] so the queue is guaranteed to be initialised.
    _private: (),
}

impl Default for ScancodeStream {
    fn default() -> Self {
        Self::new()
    }
}

impl ScancodeStream {
    /// Initialises the global queue and returns a stream handle.
    ///
    /// Panics if called more than once.
    pub fn new() -> Self {
        SCANCODE_QUEUE
            .try_init_once(|| ArrayQueue::new(SCANCODE_QUEUE_CAPACITY))
            .expect("ScancodeStream::new should only be called once");
        ScancodeStream { _private: () }
    }
}

impl Stream for ScancodeStream {
    type Item = u8;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<u8>> {
        let queue = SCANCODE_QUEUE
            .try_get()
            .expect("scancode queue not initialized");

        // Fast path: if a scancode is already waiting, return immediately
        // without touching the waker — this avoids needless atomic traffic
        // when input is plentiful.
        if let Some(scancode) = queue.pop() {
            return Poll::Ready(Some(scancode));
        }

        // Slow path: register the waker, then re-check the queue. The
        // re-check closes the race where a scancode is pushed in between the
        // first `pop` and the `register` call.
        WAKER.register(cx.waker());
        match queue.pop() {
            Some(scancode) => {
                // Got one after all — clear the waker so we don't get a
                // spurious wake on the next interrupt.
                WAKER.take();
                Poll::Ready(Some(scancode))
            }
            None => Poll::Pending,
        }
    }
}

/// Async task that prints decoded key events forever.
///
/// Reads scancodes from a [`ScancodeStream`], pushes them through the
/// `pc-keyboard` decoder, and prints the resulting characters / raw keys.
/// The loop never terminates because the stream never returns `None`.
pub async fn print_keypresses() {
    let mut scancodes = ScancodeStream::new();
    let mut keyboard = Keyboard::new(
        ScancodeSet1::new(),
        layouts::Us104Key,
        HandleControl::Ignore,
    );

    while let Some(scancode) = scancodes.next().await {
        if let Ok(Some(key_event)) = keyboard.add_byte(scancode)
            && let Some(key) = keyboard.process_keyevent(key_event)
        {
            match key {
                DecodedKey::Unicode(character) => print!("{}", character),
                DecodedKey::RawKey(key) => print!("{:?}", key),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_scancode_queue_capacity_constant() {
        // Sanity-check the capacity is something sensible. A capacity of 0
        // would deadlock the producer; capacities below ~32 would routinely
        // drop input on bursty typing.
        const _: () = assert!(SCANCODE_QUEUE_CAPACITY >= 32);
    }

    #[test_case]
    fn test_add_scancode_does_not_panic_when_queue_uninitialized() {
        // The interrupt handler may fire before any task has constructed a
        // `ScancodeStream`. In that case we want a printed warning, not a
        // panic. We can't easily assert on the warning, but we can assert
        // the call returns normally.
        add_scancode(0xAB);
    }
}
