//! Watches the queue and decides what the person should be told: the tooltip of the tray icon, what the edge glow
//! does, and the one notification at the end of a batch. It only decides; each system's backend shows the result
//! (a tray icon and a balloon on Windows, the menu bar and a notification on the Mac, a status icon on Linux).
//! A batch is judged by its own results: how many were done and failed when it began is remembered.

use std::path::PathBuf;

use crate::queue::{Counts, Entry, Status};

/// What the edge glow should do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GlowCmd {
    /// Files are converting.
    Working,
    /// The batch is over: green when everything went well, red when a file failed.
    Finish(bool),
    /// Cancelled, or nothing to report: fade out quietly.
    Stop,
}

/// The notification at the end of a batch.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub title: String,
    pub text: String,
    /// The file to show in its folder when the notification is clicked.
    pub target: Option<PathBuf>,
    pub error: bool,
}

/// What a change of the queue calls for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Update {
    /// The tray icon's tooltip.
    pub tip: String,
    pub glow: Option<GlowCmd>,
    pub notice: Option<Notice>,
}

#[derive(Default)]
pub struct Watcher {
    was_active: bool,
    /// How many were done and failed when the current batch began.
    base: (usize, usize),
}

impl Watcher {
    pub const fn new() -> Self {
        Watcher { was_active: false, base: (0, 0) }
    }

    /// The queue changed. `card_visible`: the queue card is open, so a notification would only repeat it.
    pub fn update(&mut self, c: Counts, entries: &[Entry], card_visible: bool) -> Update {
        let active = c.active() > 0;
        let was = std::mem::replace(&mut self.was_active, active);
        if active && !was {
            self.base = (c.done, c.failed);
        }
        let mut u = Update { tip: if active { format!("SwapCraft \u{b7} {} in corso", c.active()) } else { "SwapCraft".to_owned() }, ..Default::default() };
        if active {
            u.glow = Some(GlowCmd::Working);
            return u;
        }
        if !was {
            return u;
        }
        let (done, failed) = (c.done.saturating_sub(self.base.0), c.failed.saturating_sub(self.base.1));
        if failed > 0 {
            u.glow = Some(GlowCmd::Finish(false));
            if !card_visible {
                let (name, why) = entries.iter().rev().find_map(|e| if let Status::Failed(m) = &e.status { Some((e.name.clone(), m.clone())) } else { None }).unwrap_or_default();
                u.notice = Some(Notice { title: "Conversione non riuscita".to_owned(), text: format!("{name}: {why}"), target: None, error: true });
            }
        } else if done > 0 {
            u.glow = Some(GlowCmd::Finish(true));
            if !card_visible {
                let last = entries.iter().rev().find_map(|e| if let Status::Done(p) = &e.status { Some(p.clone()) } else { None });
                let text = if done == 1 {
                    let name = last.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    format!("{name} \u{e8} pronto. Clicca per vederlo nella cartella.")
                } else {
                    format!("{done} file convertiti. Clicca per vederli nella cartella.")
                };
                u.notice = Some(Notice { title: "SwapCraft".to_owned(), text, target: last, error: false });
            }
        } else {
            u.glow = Some(GlowCmd::Stop); // cancelled: nothing to announce
        }
        u
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(waiting: usize, running: usize, done: usize, failed: usize) -> Counts {
        Counts { waiting, running, done, failed }
    }
    fn entry(name: &str, status: Status) -> Entry {
        Entry { id: 1, name: name.to_owned(), to: "PDF".to_owned(), source: PathBuf::from(name), status }
    }

    #[test]
    fn a_batch_glows_while_it_runs_and_announces_itself_once_at_the_end() {
        let mut w = Watcher::default();
        let u = w.update(counts(0, 1, 0, 0), &[entry("a.docx", Status::Running)], false);
        assert_eq!((u.glow, u.notice.is_none(), u.tip.as_str()), (Some(GlowCmd::Working), true, "SwapCraft \u{b7} 1 in corso"));
        let done = [entry("a.docx", Status::Done(PathBuf::from("a.pdf")))];
        let u = w.update(counts(0, 0, 1, 0), &done, false);
        assert_eq!(u.glow, Some(GlowCmd::Finish(true)));
        let n = u.notice.expect("one notice at the end");
        assert_eq!((n.text.as_str(), n.target, n.error), ("a.pdf \u{e8} pronto. Clicca per vederlo nella cartella.", Some(PathBuf::from("a.pdf")), false));
        // nothing changed since: no second announcement
        let u = w.update(counts(0, 0, 1, 0), &done, false);
        assert_eq!((u.glow, u.notice, u.tip.as_str()), (None, None, "SwapCraft"));
    }

    #[test]
    fn a_failure_is_red_and_names_the_file_and_the_reason() {
        let mut w = Watcher::default();
        w.update(counts(0, 1, 0, 0), &[], false);
        let u = w.update(counts(0, 0, 0, 1), &[entry("b.png", Status::Failed("codec: unrecognized image format".to_owned()))], false);
        assert_eq!(u.glow, Some(GlowCmd::Finish(false)));
        let n = u.notice.unwrap();
        assert!(n.error && n.target.is_none() && n.text == "b.png: codec: unrecognized image format", "{n:?}");
    }

    #[test]
    fn a_batch_is_judged_by_its_own_results_not_the_old_ones() {
        let mut w = Watcher::default();
        // an earlier batch failed once; this one succeeds
        w.update(counts(0, 1, 0, 1), &[], false);
        w.update(counts(0, 0, 0, 1), &[], false);
        w.update(counts(0, 1, 0, 1), &[], false);
        let u = w.update(counts(0, 0, 1, 1), &[entry("c.csv", Status::Done(PathBuf::from("c.xlsx")))], false);
        assert_eq!(u.glow, Some(GlowCmd::Finish(true)), "the old failure is not this batch's");
    }

    #[test]
    fn an_open_card_gets_no_notification_and_a_cancel_gets_none_at_all() {
        let mut w = Watcher::default();
        w.update(counts(0, 1, 0, 0), &[], true);
        let u = w.update(counts(0, 0, 1, 0), &[entry("d.docx", Status::Done(PathBuf::from("d.pdf")))], true);
        assert_eq!((u.glow, u.notice), (Some(GlowCmd::Finish(true)), None));
        let mut w = Watcher::default();
        w.update(counts(0, 1, 0, 0), &[], false);
        let u = w.update(counts(0, 0, 0, 0), &[], false);
        assert_eq!((u.glow, u.notice), (Some(GlowCmd::Stop), None));
    }
}
