//! Start the Node OpenCode bridge and exchange JSON commands and events.
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::mpsc::{self, Sender},
    thread,
};

// Deliver a bridge event to the Slint event loop.
pub fn start(directory: String, emit: impl Fn(i64, String, Value) + Send + 'static) -> Sender<Value> {
    let (tx, rx) = mpsc::channel::<Value>();
    thread::spawn(move || {
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bridge/index.mjs");
        let mut command = Command::new("node");
        command.arg(script).arg(directory);
        command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        // Hide the Node console window on Windows for a clean fast launch.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                emit(-1, "error".into(), json!(format!("Cannot start OpenCode bridge: {error}")));
                return;
            }
        };
        let mut stdin = child.stdin.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let diagnostics = thread::spawn(move || {
            let mut tail = std::collections::VecDeque::new();
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if tail.len() == 12 {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
            tail.into_iter().collect::<Vec<_>>().join("\n")
        });
        // Keep writing commands while the reader waits for streamed events.
        let writer = thread::spawn(move || {
            for value in rx {
                if writeln!(stdin, "{value}").and_then(|_| stdin.flush()).is_err() {
                    break;
                }
            }
        });
        for line in BufReader::new(child.stdout.take().unwrap()).lines() {
            match line {
                Ok(line) => {
                    if let Ok(value) = serde_json::from_str::<Value>(&line)
                        && let (Some(id), Some(kind)) = (value["id"].as_i64(), value["kind"].as_str())
                    {
                        emit(id, kind.to_owned(), value["data"].clone());
                    }
                }
                Err(error) => {
                    emit(-1, "error".into(), json!(format!("OpenCode bridge output: {error}")));
                    break;
                }
            }
        }
        drop(writer);
        let status = child.wait();
        let details = diagnostics.join().unwrap_or_default();
        emit(-1, "exited".into(), json!(format!("OpenCode bridge stopped ({status:?}). {details}")));
    });
    tx
}
