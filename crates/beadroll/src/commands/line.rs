//! A line of human output as parts that say what they are (an issue id, a status, a warning),
//! never how they look: the command line escapes each part and, on a terminal that shows colour,
//! paints it (`cli::paint`).

/// What a part of a line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    /// Text with nothing to say about it.
    Plain,
    /// An issue's id.
    Id,
    /// What the line is about: a title.
    Strong,
    /// What a reader may skip: types, branches, times.
    Dim,
    /// Done: closed, created, a check mark.
    Done,
    /// Being worked on: in progress.
    Active,
    /// Held up: blocked.
    Held,
    /// Put off: deferred.
    Deferred,
    /// Gone: deleted.
    Gone,
    /// The most urgent priority, P0.
    Urgent,
    /// A high priority, P1.
    High,
    /// A low priority, P3 and P4.
    Low,
    /// Someone: an assignee or an owner.
    Who,
    /// Something to act on: unpublished work, a conflict.
    Warning,
    /// Something that failed.
    Error,
    /// A heading of a laid-out text.
    Heading,
    /// A command or option in a laid-out text.
    Term,
    /// bead's own text, laid out and painted already (a help page clap rendered): written as it
    /// is, never escaped.
    Verbatim,
}

impl Tone {
    /// The tone of an issue status.
    pub(crate) fn of_status(status: &str) -> Tone {
        match status {
            "closed" => Tone::Done,
            "in_progress" => Tone::Active,
            "blocked" => Tone::Held,
            "deferred" => Tone::Deferred,
            "tombstone" => Tone::Gone,
            _ => Tone::Plain,
        }
    }

    /// The tone of a priority, as `list` prints it (`P0`).
    pub(crate) fn of_priority(priority: &str) -> Tone {
        match priority {
            "0" => Tone::Urgent,
            "1" => Tone::High,
            "3" | "4" => Tone::Low,
            _ => Tone::Plain,
        }
    }
}

/// A line of human output: its parts, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Line(Vec<(Tone, String)>);

impl Line {
    /// An empty line.
    pub(crate) fn new() -> Line {
        Line::default()
    }

    /// The line with `text` after it, as `tone`.
    pub(crate) fn with(mut self, tone: Tone, text: impl Into<String>) -> Line {
        self.push(tone, text);
        self
    }

    /// Adds `text`, as `tone`.
    pub(crate) fn push(&mut self, tone: Tone, text: impl Into<String>) {
        let text = text.into();
        if !text.is_empty() {
            self.0.push((tone, text));
        }
    }

    /// The whole line as `tone`.
    pub(crate) fn toned(tone: Tone, text: impl Into<String>) -> Line {
        Line::new().with(tone, text)
    }

    /// The line with `other`'s parts after its own.
    pub(crate) fn then(mut self, other: Line) -> Line {
        self.0.extend(other.0);
        self
    }

    /// Its parts.
    pub(crate) fn parts(&self) -> &[(Tone, String)] {
        &self.0
    }

    /// Its text, without tones.
    pub(crate) fn text(&self) -> String {
        self.0.iter().map(|(_, text)| text.as_str()).collect()
    }
}

impl From<String> for Line {
    fn from(text: String) -> Line {
        Line::toned(Tone::Plain, text)
    }
}

impl From<&str> for Line {
    fn from(text: &str) -> Line {
        Line::toned(Tone::Plain, text)
    }
}
