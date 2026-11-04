use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crate::{Listener, Stream};

struct Buffer {
    data: Mutex<VecDeque<u8>>,
    closed: AtomicBool,
    ready: Condvar,
}

impl Buffer {
    fn new() -> Self {
        Self {
            data: Mutex::new(VecDeque::new()),
            closed: AtomicBool::new(false),
            ready: Condvar::new(),
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.ready.notify_all();
    }
}

#[cfg(test)]
pub struct MemoryStream {
    incoming: Arc<Buffer>,
    outgoing: Arc<Buffer>,
}

#[cfg(test)]
impl MemoryStream {
    pub fn pair() -> (Self, Self) {
        let first = Arc::new(Buffer::new());
        let second = Arc::new(Buffer::new());
        (
            Self {
                incoming: Arc::clone(&first),
                outgoing: Arc::clone(&second),
            },
            Self {
                incoming: second,
                outgoing: first,
            },
        )
    }
}

#[cfg(test)]
impl Stream for MemoryStream {
    fn try_clone(&self) -> io::Result<Box<dyn Stream>> {
        Ok(Box::new(Self {
            incoming: Arc::clone(&self.incoming),
            outgoing: Arc::clone(&self.outgoing),
        }))
    }

    fn shutdown(&mut self) -> io::Result<()> {
        self.incoming.close();
        self.outgoing.close();
        Ok(())
    }
}

#[cfg(test)]
impl Read for MemoryStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let mut data = self.incoming.data.lock().expect("memory buffer poisoned");
        loop {
            if !data.is_empty() {
                let count = buffer.len().min(data.len());
                for slot in buffer.iter_mut().take(count) {
                    *slot = data.pop_front().expect("buffer length checked");
                }
                return Ok(count);
            }
            if self.incoming.closed.load(Ordering::Relaxed) {
                return Ok(0);
            }
            data = self
                .incoming
                .ready
                .wait(data)
                .expect("memory buffer poisoned");
        }
    }
}

#[cfg(test)]
pub struct MemoryListener {
    pending: Mutex<VecDeque<MemoryStream>>,
    ready: Condvar,
    shutdown: AtomicBool,
}

#[cfg(test)]
impl MemoryListener {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            shutdown: AtomicBool::new(false),
        }
    }

    pub fn push(&self, stream: MemoryStream) {
        self.pending
            .lock()
            .expect("memory listener poisoned")
            .push_back(stream);
        self.ready.notify_all();
    }
}

#[cfg(test)]
impl Listener for MemoryListener {
    fn accept(&self) -> io::Result<Box<dyn Stream>> {
        let mut pending = self.pending.lock().expect("memory listener poisoned");
        loop {
            if let Some(stream) = pending.pop_front() {
                return Ok(Box::new(stream));
            }
            if self.shutdown.load(Ordering::Relaxed) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "内存 listener 已关闭",
                ));
            }
            pending = self.ready.wait(pending).expect("memory listener poisoned");
        }
    }

    fn shutdown(&self) -> io::Result<()> {
        self.shutdown.store(true, Ordering::Relaxed);
        self.ready.notify_all();
        Ok(())
    }
}

#[cfg(test)]
impl Write for MemoryStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.outgoing.closed.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "内存流已关闭"));
        }
        let mut data = self.outgoing.data.lock().expect("memory buffer poisoned");
        data.extend(buffer.iter().copied());
        self.outgoing.ready.notify_all();
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
