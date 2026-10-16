//! The line records of an `lcov.info`, as `cargo llvm-cov report --lcov` writes it: which lines of
//! each source file have code, and how often each ran. Only `SF:` (a file) and `DA:` (a line and
//! its count) are read; everything else a report holds is left to its HTML.

use std::path::Path;

/// One source file's lines that have code.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct File {
    /// The path relative to the repository's root, or as the report gave it when it is elsewhere.
    pub(super) path: String,
    /// Each line with code, in order, with the number of times it ran.
    lines: Vec<(u32, u64)>,
}

impl File {
    /// How many lines have code.
    pub(super) fn total(&self) -> usize {
        self.lines.len()
    }

    /// How many of them ran at least once.
    pub(super) fn covered(&self) -> usize {
        self.lines.iter().filter(|(_, hits)| *hits > 0).count()
    }

    /// The runs of lines with code that never ran, as first and last line: a comment or a blank
    /// line between two lines that never ran does not end the run, a line that ran does.
    pub(super) fn unreached(&self) -> Vec<(u32, u32)> {
        let mut runs: Vec<(u32, u32)> = Vec::new();
        let mut open = false;
        for &(line, hits) in &self.lines {
            if hits > 0 {
                open = false;
                continue;
            }
            match runs.last_mut() {
                Some(run) if open => run.1 = line,
                _ => {
                    runs.push((line, line));
                    open = true;
                }
            }
        }
        runs
    }
}

/// The files `text` records, in its order, their paths made relative to `root`. A record that
/// does not read is an error: a report read wrongly would say less than it holds.
pub(super) fn parse(text: &str, root: &Path) -> Result<Vec<File>, String> {
    let mut files: Vec<File> = Vec::new();
    for (index, entry) in text.lines().enumerate() {
        let at = index + 1;
        if let Some(path) = entry.strip_prefix("SF:") {
            let relative = Path::new(path)
                .strip_prefix(root)
                .unwrap_or(Path::new(path));
            files.push(File {
                path: relative.to_string_lossy().into_owned(),
                lines: Vec::new(),
            });
        } else if let Some(record) = entry.strip_prefix("DA:") {
            // `DA:LINE,COUNT`, with a checksum after the count when the report has one.
            let mut fields = record.split(',');
            let line = fields.next().and_then(|line| line.parse().ok());
            let hits = fields.next().and_then(|hits| hits.parse().ok());
            let (Some(line), Some(hits)) = (line, hits) else {
                return Err(format!("line {at}: not a line and its count: {entry}"));
            };
            let Some(file) = files.last_mut() else {
                return Err(format!("line {at}: a DA record before any SF record"));
            };
            file.lines.push((line, hits));
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPORT: &str = "\
TN:
SF:/repo/crates/a/src/lib.rs
FN:1,f
FNDA:3,f
FNF:1
FNH:1
DA:1,3
DA:2,0
DA:3,0
DA:5,2
DA:7,0
DA:8,0
LF:6
LH:2
end_of_record
SF:/elsewhere/b.rs
DA:1,1,abc123
end_of_record
";

    #[test]
    fn a_report_is_read_by_its_files_and_their_lines() {
        let files = parse(REPORT, Path::new("/repo")).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "crates/a/src/lib.rs");
        assert_eq!((files[0].covered(), files[0].total()), (2, 6));
        assert_eq!(
            files[1].path, "/elsewhere/b.rs",
            "outside the root as it was"
        );
        assert_eq!(
            files[1].lines,
            [(1, 1)],
            "a checksum after the count is set aside"
        );
    }

    #[test]
    fn lines_that_never_ran_are_runs_that_a_line_that_ran_ends() {
        let files = parse(REPORT, Path::new("/repo")).unwrap();
        // 2-3, then 7-8 after line 5 ran; line 4 and 6 have no code and end nothing
        assert_eq!(files[0].unreached(), [(2, 3), (7, 8)]);
        assert_eq!(files[1].unreached(), []);
    }

    #[test]
    fn a_record_that_does_not_read_is_an_error_naming_its_line() {
        let root = Path::new("/repo");
        let error = |text: &str| parse(text, root).unwrap_err();
        let unread = "line 2: not a line and its count";
        assert!(error("SF:/repo/a.rs\nDA:x,1\n").starts_with(unread));
        assert!(error("SF:/repo/a.rs\nDA:3,many\n").starts_with(unread));
        assert!(error("SF:/repo/a.rs\nDA:3\n").starts_with(unread));
        assert!(error("DA:3,1\n").starts_with("line 1: a DA record before"));
    }
}
