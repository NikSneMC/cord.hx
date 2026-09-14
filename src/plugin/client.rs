//! Client end of the cord server pipe.
//!
//! Messages written by the editor are raw msgpack; messages sent by the
//! server are prefixed with a 4-byte big-endian length.

use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::codec;

#[cfg(unix)]
type Stream = std::os::unix::net::UnixStream;
#[cfg(windows)]
type Stream = std::fs::File;

pub struct Connection {
    stream: Arc<Mutex<Option<Stream>>>,
}

/// Connects to the pipe at `path`. `on_message` is called from a reader
/// thread with every decoded message, and with `None` once the connection
/// is closed. Errors are libuv-style codes (`ENOENT`, `ECONNREFUSED`, ...).
pub fn connect(
    path: &str,
    mut on_message: impl FnMut(Option<Value>) + Send + 'static,
) -> Result<Connection, String> {
    let stream = open(path).map_err(|e| error_code(&e))?;
    let reader = stream.try_clone().map_err(|e| error_code(&e))?;
    let stream = Arc::new(Mutex::new(Some(stream)));

    let shared = Arc::clone(&stream);
    std::thread::Builder::new()
        .name("cord-client".to_string())
        .spawn(move || {
            read_loop(reader, &mut on_message);
            if let Some(stream) = shared.lock().unwrap().take() {
                shutdown(&stream);
            }
            on_message(None);
        })
        .map_err(|e| e.to_string())?;

    Ok(Connection { stream })
}

impl Connection {
    /// Encodes `message` as msgpack and writes it.
    pub fn send(&self, message: &Value) -> Result<(), String> {
        let data = codec::value_to_msgpack(message.clone())?;
        let mut guard = self.stream.lock().unwrap();
        let Some(stream) = guard.as_mut() else {
            return Err("No pipe connection".to_string());
        };
        stream
            .write_all(&data)
            .and_then(|_| stream.flush())
            .map_err(|e| error_code(&e))
    }

    pub fn close(&self) {
        if let Some(stream) = self.stream.lock().unwrap().take() {
            shutdown(&stream);
        }
    }

    pub fn is_open(&self) -> bool {
        self.stream.lock().unwrap().is_some()
    }
}

fn read_loop(mut reader: Stream, on_message: &mut impl FnMut(Option<Value>)) {
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];

    loop {
        let n = match reader.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        buffer.extend_from_slice(&chunk[..n]);

        while buffer.len() >= 4 {
            let length = u32::from_be_bytes([
                buffer[0], buffer[1], buffer[2], buffer[3],
            ]) as usize;
            if buffer.len() < 4 + length {
                break;
            }
            let message: Vec<u8> = buffer.drain(..4 + length).skip(4).collect();
            on_message(Some(
                match rmpv::decode::read_value(&mut message.as_slice()) {
                    Ok(value) => codec::rmpv_to_json(value),
                    Err(e) => {
                        serde_json::json!({ "type": "decode_error", "data": e.to_string() })
                    }
                },
            ));
        }
    }
}

#[cfg(unix)]
fn open(path: &str) -> io::Result<Stream> {
    std::os::unix::net::UnixStream::connect(path)
}

#[cfg(windows)]
fn open(path: &str) -> io::Result<Stream> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
}

#[cfg(unix)]
fn shutdown(stream: &Stream) {
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

#[cfg(windows)]
fn shutdown(_stream: &Stream) {}

/// Maps an I/O error to the libuv-style codes `server::connect` matches on.
pub fn error_code(e: &io::Error) -> String {
    use io::ErrorKind as K;

    match e.kind() {
        K::NotFound => "ENOENT".to_string(),
        K::ConnectionRefused => "ECONNREFUSED".to_string(),
        K::ConnectionReset => "ECONNRESET".to_string(),
        K::TimedOut => "ETIMEDOUT".to_string(),
        K::PermissionDenied => "EACCES".to_string(),
        K::BrokenPipe => "EPIPE".to_string(),
        _ => e.to_string(),
    }
}
