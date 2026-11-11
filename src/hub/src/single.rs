//! One hub at a time. The first hub listens on a loopback port it writes to `hub.port`; a second start finds it,
//! hands over the files it was given (dragged onto the program, or "Open with") and ends, and the first one comes
//! to the front with those files in its drop panel. Started by the right-click menu (`--run`) the hub never takes
//! part: that is a small progress window, and Explorer starts one per file.
//!
//! Only the loopback interface is used, and a message only puts files in the drop panel: nothing is converted
//! without a click.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

const PORT_FILE: &str = "hub.port";
const HELLO: &str = "SWAPCRAFT-HUB 1";

/// Tries to hand `files` to a hub already running. True when one took them: this process should end.
pub fn hand_over(files: &[PathBuf]) -> bool {
    let Some(port) = crate::settings::data_dir().and_then(|d| std::fs::read_to_string(d.join(PORT_FILE)).ok()).and_then(|s| s.trim().parse::<u16>().ok()) else {
        return false;
    };
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(400)) else { return false };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut msg = format!("{HELLO}\n");
    for f in files {
        // one path per line; a path with a line break in it cannot be sent this way and is left out
        let p = std::path::absolute(f).unwrap_or_else(|_| f.clone());
        let s = p.to_string_lossy();
        if !s.contains('\n') {
            msg.push_str(&s);
            msg.push('\n');
        }
    }
    msg.push('\n');
    if stream.write_all(msg.as_bytes()).is_err() {
        return false;
    }
    // the running hub answers only when it understood: anything else listening on that port is not a hub
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer).is_ok() && answer.trim() == "ok"
}

/// Starts listening for later starts. Each one sends the files it was given (maybe none) on the channel, and
/// `wake` brings the window forward.
pub fn listen(wake: impl Fn() + Send + 'static) -> Option<mpsc::Receiver<Vec<PathBuf>>> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    let dir = crate::settings::data_dir()?;
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(dir.join(PORT_FILE), format!("{port}\n")).ok()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut reader = BufReader::new(&stream);
            let mut first = String::new();
            if reader.read_line(&mut first).is_err() || first.trim_end() != HELLO {
                continue;
            }
            let mut files = Vec::new();
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if line.is_empty() {
                    break;
                }
                let p = PathBuf::from(line);
                if p.exists() {
                    files.push(p);
                }
            }
            let _ = (&stream).write_all(b"ok\n");
            if tx.send(files).is_err() {
                break;
            }
            wake();
        }
    });
    Some(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_start_hands_its_files_to_the_first() {
        let dir = std::env::temp_dir().join(format!("vf_single_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: no other test of this crate reads these variables
        unsafe {
            std::env::set_var("APPDATA", &dir);
            std::env::set_var("XDG_CONFIG_HOME", &dir);
        }
        assert!(!hand_over(&[]), "nobody is listening yet");
        let rx = listen(|| {}).expect("listening");
        let file = dir.join("a.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(hand_over(std::slice::from_ref(&file)));
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), vec![file]);
        assert!(hand_over(&[]), "a plain start only wakes the first hub");
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Vec::<PathBuf>::new());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
