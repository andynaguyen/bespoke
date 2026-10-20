//! Single-instance handling: a second launch forwards its arguments to the
//! running instance over a local socket and exits. Remote-control flags
//! (`--toggle-transcription`, `--cancel`, ...) rely on this.
//!
//! Unix uses a socket file in the per-user runtime/temp directory. Windows
//! listens on a loopback TCP port recorded in the local data directory.

use std::io::{BufRead, BufReader, Write};

use crate::app_handle::APP_IDENTIFIER;

/// Either we are the primary instance (and own the listener), or the
/// arguments were delivered to the instance that already runs.
pub enum Instance {
    Primary(Listener),
    Forwarded,
}

/// Tries to forward `args` to a running instance; otherwise claims the
/// instance slot.
pub fn acquire(args: &[String]) -> Instance {
    if imp::forward(args).is_ok() {
        return Instance::Forwarded;
    }
    match imp::listen() {
        Ok(listener) => Instance::Primary(Listener(Some(listener))),
        Err(e) => {
            log::warn!("Single-instance listener unavailable: {e}");
            Instance::Primary(Listener(None))
        }
    }
}

pub struct Listener(Option<imp::RawListener>);

impl Listener {
    /// Calls `on_args` with each forwarded argument list, on a background
    /// thread, for the life of the process.
    pub fn serve(self, on_args: impl Fn(Vec<String>) + Send + 'static) {
        let Some(listener) = self.0 else {
            return;
        };
        std::thread::Builder::new()
            .name("single-instance".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let args: Vec<String> = BufReader::new(stream)
                        .lines()
                        .map_while(Result::ok)
                        .collect();
                    on_args(args);
                }
            })
            .ok();
    }
}

fn write_args(mut stream: impl Write, args: &[String]) -> std::io::Result<()> {
    for arg in args {
        writeln!(stream, "{}", arg.replace('\n', " "))?;
    }
    stream.flush()
}

#[cfg(unix)]
mod imp {
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;

    pub type RawListener = UnixListener;

    fn socket_path() -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        dir.join(format!("{}.sock", super::APP_IDENTIFIER))
    }

    pub fn forward(args: &[String]) -> std::io::Result<()> {
        let stream = UnixStream::connect(socket_path())?;
        super::write_args(stream, args)
    }

    pub fn listen() -> std::io::Result<UnixListener> {
        let path = socket_path();
        // A socket file left by a crashed instance refuses connections; the
        // failed `forward` above proved nobody is listening, so replace it.
        let _ = std::fs::remove_file(&path);
        UnixListener::bind(path)
    }
}

#[cfg(windows)]
mod imp {
    use std::net::{Ipv4Addr, TcpListener, TcpStream};
    use std::path::PathBuf;

    pub type RawListener = TcpListener;

    fn port_file() -> Option<PathBuf> {
        dirs::data_local_dir().map(|dir| dir.join(super::APP_IDENTIFIER).join("instance.port"))
    }

    pub fn forward(args: &[String]) -> std::io::Result<()> {
        let path = port_file().ok_or(std::io::ErrorKind::NotFound)?;
        let port: u16 = std::fs::read_to_string(path)?
            .trim()
            .parse()
            .map_err(|_| std::io::ErrorKind::InvalidData)?;
        let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))?;
        super::write_args(stream, args)
    }

    pub fn listen() -> std::io::Result<TcpListener> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let path = port_file().ok_or(std::io::ErrorKind::NotFound)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, listener.local_addr()?.port().to_string())?;
        Ok(listener)
    }
}
