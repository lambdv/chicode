//! Single-instance handoff so extra launches open a fast in-process window.
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    sync::{Mutex, OnceLock},
    time::Duration,
};

// Queue dirs that arrive before the UI thread is ready.
static PENDING: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

/// Pending dirs waiting for the event loop.
fn pending() -> &'static Mutex<Vec<String>> {
    PENDING.get_or_init(|| Mutex::new(Vec::new()))
}

/// Pick a loopback port per directory so different folders stay independent.
pub fn port_for_dir(dir: &str) -> u16 {
    let mut hash = DefaultHasher::new();
    dir.hash(&mut hash);
    41000 + (hash.finish() % 1000) as u16
}

/// Bind as primary, or return None when another instance owns the port.
pub fn try_primary(port: u16) -> Option<TcpListener> {
    TcpListener::bind(format!("127.0.0.1:{port}")).ok()
}

/// Tell the primary to open a window, then exit fast.
pub fn notify(port: u16, dir: &str) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_millis(300),
    ) else {
        return false;
    };
    let _ = stream.write_all(format!("{dir}\n").as_bytes());
    true
}

/// Remember a dir for later UI-thread handling.
pub fn queue(dir: String) {
    pending().lock().unwrap().push(dir);
}

/// Take all queued dirs.
pub fn take_queued() -> Vec<String> {
    std::mem::take(&mut *pending().lock().unwrap())
}

/// Listen for second launches and forward them to the UI thread.
pub fn listen(listener: TcpListener) {
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            // Read one line with a short timeout so stale clients never block us.
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            let mut line = String::new();
            if BufReader::new(&stream).read_line(&mut line).is_ok() {
                let dir = line.trim().to_owned();
                queue(if dir.is_empty() { String::new() } else { dir });
                // Wake the UI thread; it drains the queue and opens a window.
                let _ = slint::invoke_from_event_loop(|| {
                    crate::app::drain_requests();
                });
            }
        }
    });
}
