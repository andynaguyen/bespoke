use std::io::{BufRead, BufReader, Write};

use crate::app_handle::APP_IDENTIFIER;

pub enum Instance {
    Primary(Listener),
    Forwarded,
}

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
        let _ = std::fs::remove_file(&path);
        UnixListener::bind(path)
    }
}
