//! What mmore remembers between sessions: searches, filters, `|` commands, and
//! marks, in a small text file (by default `~/.local/state/mmore/history`).
//!
//! One entry per line, the kind first and fields separated by tabs:
//! `search\tPATTERN`, `filter\tPATTERN`, `command\tCOMMAND`, and
//! `mark\tLETTER\tSOURCE\tROWS_IN\tPATH` (the path last, as it may contain
//! anything but a newline). Unknown lines are skipped. Sessions add to the file
//! when they end, so several at once do not lose each other's entries.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// How many of each kind (searches, filters, commands) are kept.
const KEEP: usize = 100;

/// A mark kept between sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedMark {
    pub letter: char,
    /// The file, as an absolute path.
    pub path: PathBuf,
    pub source: usize,
    pub rows_in: usize,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct History {
    /// Oldest first.
    pub searches: Vec<String>,
    pub filters: Vec<String>,
    pub commands: Vec<String>,
    pub marks: Vec<SavedMark>,
}

/// Which list an entry goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Search,
    Filter,
    Command,
}

impl History {
    /// The history in `path`; nothing if it does not exist or cannot be read.
    pub fn load(path: &Path) -> History {
        fs::read_to_string(path)
            .map(|text| History::parse(&text))
            .unwrap_or_default()
    }

    fn parse(text: &str) -> History {
        let mut history = History::default();
        for line in text.lines() {
            let mut fields = line.splitn(5, '\t');
            match (fields.next(), fields.next()) {
                (Some("search"), Some(text)) => history.searches.push(text.into()),
                (Some("filter"), Some(text)) => history.filters.push(text.into()),
                (Some("command"), Some(text)) => history.commands.push(text.into()),
                (Some("mark"), Some(letter)) => {
                    let mark = (|| {
                        let letter = letter.chars().next().filter(|_| letter.len() == 1)?;
                        let source = fields.next()?.parse().ok()?;
                        let rows_in = fields.next()?.parse().ok()?;
                        let path = PathBuf::from(fields.next()?);
                        Some(SavedMark {
                            letter,
                            path,
                            source,
                            rows_in,
                        })
                    })();
                    if let Some(mark) = mark {
                        history.marks.retain(|m| m.letter != mark.letter);
                        history.marks.push(mark);
                    }
                }
                _ => {}
            }
        }
        history
    }

    fn render(&self) -> String {
        let mut text = String::from("# mmore history (searches, filters, commands, marks)\n");
        for (kind, list) in [
            ("search", &self.searches),
            ("filter", &self.filters),
            ("command", &self.commands),
        ] {
            for entry in list {
                text.push_str(&format!("{kind}\t{entry}\n"));
            }
        }
        for m in &self.marks {
            text.push_str(&format!(
                "mark\t{}\t{}\t{}\t{}\n",
                m.letter,
                m.source,
                m.rows_in,
                m.path.display()
            ));
        }
        text
    }

    pub fn list(&self, kind: Kind) -> &[String] {
        match kind {
            Kind::Search => &self.searches,
            Kind::Filter => &self.filters,
            Kind::Command => &self.commands,
        }
    }

    /// Add `entry` as the newest of its kind (moving it there if it was older).
    pub fn add(&mut self, kind: Kind, entry: &str) {
        let list = match kind {
            Kind::Search => &mut self.searches,
            Kind::Filter => &mut self.filters,
            Kind::Command => &mut self.commands,
        };
        if entry.is_empty() || entry.contains(['\n', '\t']) {
            return;
        }
        list.retain(|e| e != entry);
        list.push(entry.to_string());
        if list.len() > KEEP {
            list.drain(..list.len() - KEEP);
        }
    }

    /// Add what this session did (`session`) to the history file at `path`, as
    /// it is now (another session may have written to it meanwhile).
    pub fn save(path: &Path, session: &History) -> io::Result<()> {
        let mut history = History::load(path);
        for kind in [Kind::Search, Kind::Filter, Kind::Command] {
            for entry in session.list(kind) {
                history.add(kind, entry);
            }
        }
        for mark in &session.marks {
            history.marks.retain(|m| m.letter != mark.letter);
            history.marks.push(mark.clone());
        }
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        // Written next to it, then moved into place, so a reader never sees half.
        let temp = path.with_extension(format!("tmp{}", std::process::id()));
        let mut options = fs::File::options();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Searches and commands can be private.
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(history.render().as_bytes())?;
        drop(file);
        fs::rename(&temp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_kept_newest_last_without_repeats() {
        let mut h = History::default();
        for e in ["a", "b", "a", "", "with\ttab"] {
            h.add(Kind::Search, e);
        }
        assert_eq!(h.searches, ["b", "a"]);
        for i in 0..150 {
            h.add(Kind::Command, &format!("cmd {i}"));
        }
        assert_eq!(h.commands.len(), KEEP);
        assert_eq!(h.commands[0], "cmd 50");
    }

    #[test]
    fn saving_adds_to_what_is_there() {
        let dir = std::env::temp_dir().join(format!("mmore-history-{}", std::process::id()));
        let path = dir.join("state").join("history");
        let mut first = History::default();
        first.add(Kind::Search, "error");
        first.add(Kind::Filter, "!debug");
        first.marks.push(SavedMark {
            letter: 'a',
            path: "/tmp/one file.txt".into(),
            source: 41,
            rows_in: 2,
        });
        History::save(&path, &first).unwrap();
        // Another session, later: its entries come after, and its mark a wins.
        let mut second = History::default();
        second.add(Kind::Search, "warn");
        second.add(Kind::Search, "error");
        second.add(Kind::Command, "wc -l");
        second.marks.push(SavedMark {
            letter: 'a',
            path: "/tmp/two.txt".into(),
            source: 7,
            rows_in: 0,
        });
        History::save(&path, &second).unwrap();
        let h = History::load(&path);
        assert_eq!(h.searches, ["warn", "error"]);
        assert_eq!(h.filters, ["!debug"]);
        assert_eq!(h.commands, ["wc -l"]);
        assert_eq!(h.marks.len(), 1);
        assert_eq!(h.marks[0].path, PathBuf::from("/tmp/two.txt"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bad_lines_are_skipped() {
        let h = History::parse("junk\nmark\tab\t1\t2\t/x\nmark\tb\tx\t0\t/y\nsearch\tok\n");
        assert_eq!(h.searches, ["ok"]);
        assert!(h.marks.is_empty());
    }
}
