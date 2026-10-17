//! Where an issue's texts are (roll-lqjj.6): its long fields' values, which their registers hold
//! alone (roll-lqjj.10), its comments, its extras, and the metadata each addition of each of its
//! edges carries (roll-lqjj.39). Replay makes them in the issue. A projection keeps them in a file
//! of their own, and a state read from it has them there until a command that prints them reads
//! them in; what a list and a later event need of them without reading them stays in the issue:
//! the events behind each long field's register, which of those fields two writers set
//! differently, and each edge with the events that added it, which the issue holds besides.

use crate::event::Field;
use crate::id::EventId;

/// The fields an issue says at length, kept with its texts.
pub const TEXT_FIELDS: [Field; 4] = [
    Field::Description,
    Field::Design,
    Field::AcceptanceCriteria,
    Field::Notes,
];

/// Whether `field` is one of [`TEXT_FIELDS`].
pub fn is_text(field: Field) -> bool {
    TEXT_FIELDS.contains(&field)
}

/// Where a projection keeps an issue's texts: the offset of their bytes in its text file, how many
/// there are, and their sum ([`crate::projection::sum`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextPlace {
    /// Where they begin.
    pub at: u64,
    /// How many bytes they are.
    pub len: u32,
    /// The sum of those bytes.
    pub sum: [u8; 16],
}

/// What an issue keeps of texts it has not read: where they are, the events behind each long
/// field's register (in the register's order), and the long fields two writers set differently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    /// Where they are.
    pub place: TextPlace,
    /// The events behind each long field's register.
    pub tags: Vec<(Field, Vec<EventId>)>,
    /// The long fields disputed.
    pub disputed: Vec<Field>,
}

/// Where an issue's texts are.
#[derive(Clone, Default)]
pub enum Texts {
    /// In the issue, and written nowhere yet, or changed since they were read.
    #[default]
    Here,
    /// In the issue, as read from this place, unchanged: written there again as they are.
    Read(TextPlace),
    /// In a text file only. The issue's long fields, their registers, its comments and its extras
    /// are empty, and its edges' metadata absent, until they are read in.
    There(Box<Stored>),
}

impl Texts {
    /// Whether the texts are in the issue.
    pub fn here(&self) -> bool {
        !matches!(self, Texts::There(_))
    }
}

impl std::fmt::Debug for Texts {
    /// As [`PartialEq`] compares them: texts in the issue alike, wherever they were read from.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Texts::There(stored) => f.debug_tuple("There").field(stored).finish(),
            Texts::Here | Texts::Read(_) => f.write_str("Here"),
        }
    }
}

impl PartialEq for Texts {
    /// Texts read in are the texts replay made: where they came from is no difference.
    fn eq(&self, other: &Texts) -> bool {
        match (self, other) {
            (Texts::There(a), Texts::There(b)) => a == b,
            (Texts::There(_), _) | (_, Texts::There(_)) => false,
            _ => true,
        }
    }
}
