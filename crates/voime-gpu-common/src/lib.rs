//! Shared frame protocol for the two GPU Final workers.
//!
//! See `voime-gpu-common` USER §4 for the wire contract.

use std::io::Read;
use std::time::Duration;

pub mod frame;
pub mod lifecycle;
pub mod message;

pub use frame::{read_frame, write_frame, Frame};
pub use lifecycle::{error_response, expect_result, handshake, ReadyInfo};
pub use message::{ErrorCode, GpuBackend, Request, Response};

pub const GPU_PROTOCOL_VERSION: u32 = 1;
pub const MAX_HEADER_BYTES: usize = 64 * 1024;
pub const MAX_AUDIO_BYTES: usize = 64 * 1024 * 1024;
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(120);
pub const TRANSCRIBE_TIMEOUT: Duration = Duration::from_secs(60);
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Reads exactly `buffer.len()` bytes. Returns `Ok(false)` for a clean EOF
/// only when nothing has been read yet.
pub(crate) fn read_exact_or_eof<R: Read>(
    reader: &mut R,
    buffer: &mut [u8],
) -> anyhow::Result<bool> {
    if buffer.is_empty() {
        return Ok(true);
    }
    let mut offset = 0;
    while offset < buffer.len() {
        match reader.read(&mut buffer[offset..]) {
            Ok(0) if offset == 0 => return Ok(false),
            Ok(0) => anyhow::bail!("帧被截断"),
            Ok(read) => offset += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(true)
}
