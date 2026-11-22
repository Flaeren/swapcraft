//! A drag source for the self-test: another X client that does what a file manager does when someone drags a file
//! (presses button 1, takes the `XdndSelection`, answers requests for `text/uri-list`, then sends the XDND messages to
//! the window under the pointer), so the bar's detection and its drop target are tried over the real protocol, not
//! through a shortcut. Only the self-test uses it.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::*;
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::wrapper::ConnectionExt as _;

type Res<T> = Result<T, Box<dyn std::error::Error>>;

/// Where to drop: the pointer position and the window that takes the drop.
pub struct Target {
    pub x: i32,
    pub y: i32,
    pub window: Window,
}

pub struct Source {
    tx: Sender<Target>,
    join: JoinHandle<Result<(), String>>,
}

impl Source {
    /// Starts the drag of `file`: button 1 goes down and the drag takes the selection.
    pub fn start(file: &str) -> Source {
        let (tx, rx) = channel();
        let file = file.to_owned();
        let join = std::thread::spawn(move || run(&file, rx).map_err(|e| e.to_string()));
        Source { tx, join }
    }

    /// The pointer is over `target`: the thread sends the XDND messages and drops (the caller goes on serving the bar).
    pub fn go(&self, target: Target) {
        let _ = self.tx.send(target);
    }

    /// What the thread says once the drop is over (or failed).
    pub fn finish(self) -> Result<(), String> {
        self.join.join().map_err(|_| "the drag source panicked".to_owned())?
    }
}

fn atom(c: &impl Connection, name: &[u8]) -> Res<Atom> {
    Ok(c.intern_atom(false, name)?.reply()?.atom)
}

fn run(file: &str, rx: Receiver<Target>) -> Res<()> {
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let (selection, uri_list, targets, status, finished, enter, position, drop, copy) = (
        atom(&conn, b"XdndSelection")?,
        atom(&conn, b"text/uri-list")?,
        atom(&conn, b"TARGETS")?,
        atom(&conn, b"XdndStatus")?,
        atom(&conn, b"XdndFinished")?,
        atom(&conn, b"XdndEnter")?,
        atom(&conn, b"XdndPosition")?,
        atom(&conn, b"XdndDrop")?,
        atom(&conn, b"XdndActionCopy")?,
    );
    let win = conn.generate_id()?;
    conn.create_window(0, win, root, 0, 0, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new())?;
    // the drag begins: button 1 down, and the source owns the selection
    conn.xtest_fake_input(4, 1, 0, root, 0, 0, 0)?;
    conn.set_selection_owner(win, selection, x11rb::CURRENT_TIME)?;
    conn.flush()?;
    let body = format!("file://{}\r\n", file.replace(' ', "%20"));
    let started = Instant::now();
    let mut target: Option<Target> = None;
    let mut sent_drop = false;
    let mut accepted = false;
    let mut result: Res<()> = Err("the drop was never finished".into());
    while started.elapsed() < Duration::from_secs(20) {
        if target.is_none() {
            if let Ok(t) = rx.try_recv() {
                // enter, then position: the target answers with a status
                let data = [win, 5 << 24, uri_list, 0, 0];
                conn.send_event(false, t.window, EventMask::NO_EVENT, ClientMessageEvent::new(32, t.window, enter, data))?;
                let pos = [win, 0, ((t.x as u32) << 16) | (t.y as u32 & 0xffff), x11rb::CURRENT_TIME, copy];
                conn.send_event(false, t.window, EventMask::NO_EVENT, ClientMessageEvent::new(32, t.window, position, pos))?;
                conn.flush()?;
                target = Some(t);
            }
        }
        while let Some(ev) = conn.poll_for_event()? {
            match ev {
                Event::SelectionRequest(r) => {
                    let mut property = r.property;
                    if r.target == uri_list {
                        conn.change_property8(PropMode::REPLACE, r.requestor, r.property, uri_list, body.as_bytes())?;
                    } else if r.target == targets {
                        conn.change_property32(PropMode::REPLACE, r.requestor, r.property, AtomEnum::ATOM, &[targets, uri_list])?;
                    } else {
                        property = 0;
                    }
                    let note = SelectionNotifyEvent { response_type: SELECTION_NOTIFY_EVENT, sequence: 0, time: r.time, requestor: r.requestor, selection: r.selection, target: r.target, property };
                    conn.send_event(false, r.requestor, EventMask::NO_EVENT, note)?;
                    conn.flush()?;
                }
                Event::ClientMessage(m) if m.type_ == status => {
                    let d = m.data.as_data32();
                    accepted = d[1] & 1 != 0;
                    if let (Some(t), false) = (&target, sent_drop) {
                        if accepted {
                            let data = [win, 0, x11rb::CURRENT_TIME, 0, 0];
                            conn.send_event(false, t.window, EventMask::NO_EVENT, ClientMessageEvent::new(32, t.window, drop, data))?;
                            conn.flush()?;
                            sent_drop = true;
                        } else {
                            result = Err("the bar did not accept the position".into());
                            break;
                        }
                    }
                }
                Event::ClientMessage(m) if m.type_ == finished => {
                    let d = m.data.as_data32();
                    result = if d[1] & 1 != 0 { Ok(()) } else { Err("the bar finished the drop but did not accept it".into()) };
                    sent_drop = false;
                    target = None;
                    // the drag is over: button up, selection released
                    conn.xtest_fake_input(5, 1, 0, root, 0, 0, 0)?;
                    conn.set_selection_owner(0u32, selection, x11rb::CURRENT_TIME)?;
                    conn.flush()?;
                    return result;
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    conn.xtest_fake_input(5, 1, 0, root, 0, 0, 0)?;
    conn.set_selection_owner(0u32, selection, x11rb::CURRENT_TIME)?;
    conn.flush()?;
    let _ = accepted;
    result
}
