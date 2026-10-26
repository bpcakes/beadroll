//! An issue's dependency edges, held typed (roll-lqjj.39): each one's target an issue's id, or the
//! text of a target that is no id, and its kind a name replay and a decoded projection hold once
//! for every edge of that kind; and the metadata each addition of an edge carries, which only an export prints
//! and a projection keeps with the issue's texts (roll-lqjj.6), so that a state read alone has the
//! edges and the events behind them, and their metadata once its texts are read in.

use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

use crate::id::EventId;
use crate::json::Object;

use super::small::IdMap;

mod tags;

pub use tags::DepTags;

/// What a dependency names: an issue's id, held as the id, when the text is an id's spelling (the
/// issue it names may not be there, or not yet), and the text otherwise, which names no issue (a
/// key, an external reference). Shown and ordered as the text it was named by.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    /// An id, as an issue's uid is.
    Id(EventId),
    /// Text that is no id's spelling.
    Text(Box<str>),
}

impl Target {
    /// The target `text` names: its id when it is an id's spelling, which [`EventId::parse`] reads
    /// exactly and [`EventId`]'s `Display` writes again, else the text.
    pub fn of(text: &str) -> Target {
        match EventId::parse(text) {
            Some(id) => Target::Id(id),
            None => Target::Text(text.into()),
        }
    }

    /// The target of text known to be no id's spelling, as a projection reads it back: `None` for
    /// one that is.
    pub(crate) fn text(text: String) -> Option<Target> {
        EventId::parse(&text)
            .is_none()
            .then(|| Target::Text(text.into()))
    }

    /// The id it names, if it is one.
    pub fn id(&self) -> Option<EventId> {
        match self {
            Target::Id(id) => Some(*id),
            Target::Text(_) => None,
        }
    }
}

impl fmt::Display for Target {
    /// The text it was named by.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Id(id) => id.fmt(f),
            Target::Text(text) => f.write_str(text),
        }
    }
}

impl Ord for Target {
    /// As the texts they were named by order bytewise: ids by their numbers, which order as their
    /// spellings do.
    fn cmp(&self, other: &Target) -> Ordering {
        match (self, other) {
            (Target::Id(a), Target::Id(b)) => a.cmp(b),
            (Target::Text(a), Target::Text(b)) => a.cmp(b),
            (Target::Id(a), Target::Text(b)) => a.to_string().as_str().cmp(b),
            (Target::Text(a), Target::Id(b)) => (**a).cmp(b.to_string().as_str()),
        }
    }
}

impl PartialOrd for Target {
    fn partial_cmp(&self, other: &Target) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A dependency: what it names and its kind (`blocks`, `parent-child`, `related` or any other
/// word). Ordered by target, then kind, as the pair of texts was.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Edge {
    pub(crate) to: Target,
    /// Held once for every edge of this kind, by replay and by a projection's name table as it is
    /// decoded; an extension interns the kinds of its batch alone, so an edge it adds holds its own
    /// until the projection is next decoded.
    pub(crate) kind: Arc<str>,
}

impl Edge {
    /// What it names.
    pub fn to(&self) -> &Target {
        &self.to
    }

    /// Its kind.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Whether it names `to` and is of `kind`.
    pub fn is(&self, to: &Target, kind: &str) -> bool {
        self.to == *to && *self.kind == *kind
    }
}

/// The metadata an addition of a dependency carries (`created_at`, `created_by`, and whatever an
/// import or the `dep_add` brought), kept with its issue's texts: `None` while they are not read
/// in, which only an export and applying more events need.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta(pub(crate) Option<Box<Object>>);

impl Meta {
    /// The metadata `members`.
    pub(crate) fn of(members: Object) -> Meta {
        Meta(Some(Box::new(members)))
    }

    /// Its members, once the issue's texts are read in.
    pub fn members(&self) -> Option<&Object> {
        self.0.as_deref()
    }
}

/// An issue's dependencies ([`super::Issue::deps`]), read from the additions behind them, which
/// hold the metadata each was added with: a removed dependency has none left.
#[derive(Clone, Copy)]
pub struct Deps<'a>(pub(super) &'a DepTags);

impl<'a> Deps<'a> {
    /// Each dependency and the metadata of its earliest addition.
    pub fn iter(self) -> DepsIter<'a> {
        let earliest: fn(_) -> _ = earliest;
        self.0.into_iter().filter_map(earliest)
    }

    /// Each dependency.
    pub fn keys(self) -> impl Iterator<Item = &'a Edge> {
        self.iter().map(|(edge, _)| edge)
    }

    /// The metadata of the earliest addition of the dependency on `to` of `kind`, if the issue has
    /// it.
    pub fn get(self, to: &Target, kind: &str) -> Option<&'a Meta> {
        let tags = self.0.get(to, kind)?;
        tags.first().map(|(_, meta)| meta)
    }

    /// Whether the issue depends on `to` with `kind`.
    pub fn contains(self, to: &Target, kind: &str) -> bool {
        self.get(to, kind).is_some()
    }

    /// How many dependencies the issue has.
    pub fn len(self) -> usize {
        self.iter().count()
    }

    /// Whether it has none.
    pub fn is_empty(self) -> bool {
        self.iter().next().is_none()
    }
}

impl<'a> IntoIterator for Deps<'a> {
    type Item = (&'a Edge, &'a Meta);
    type IntoIter = DepsIter<'a>;

    fn into_iter(self) -> DepsIter<'a> {
        self.iter()
    }
}

/// The iterator over [`Deps`].
pub type DepsIter<'a> = std::iter::FilterMap<
    <&'a DepTags as IntoIterator>::IntoIter,
    fn((&'a Edge, &'a IdMap<Meta>)) -> Option<(&'a Edge, &'a Meta)>,
>;

/// A dependency and the metadata of its earliest addition, if it has one left.
fn earliest<'a>((edge, tags): (&'a Edge, &'a IdMap<Meta>)) -> Option<(&'a Edge, &'a Meta)> {
    tags.first().map(|(_, meta)| (edge, meta))
}

impl fmt::Debug for Deps<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_is_an_id_exactly_when_its_text_is_an_ids_spelling_and_shows_that_text() {
        let id = "01a11621-e285-7398-ab1c-b9af4864eaa2";
        for (text, typed) in [
            (id, true),
            ("01A11621-E285-7398-AB1C-B9AF4864EAA2", false),
            ("01a11621-e285-7398-ab1c-b9af4864eaa2 ", false),
            ("ex-a1b2", false),
            ("", false),
        ] {
            let target = Target::of(text);
            assert_eq!(target.id().is_some(), typed, "{text:?}");
            assert_eq!(target.to_string(), text);
            assert_eq!(Target::text(text.to_owned()).is_some(), !typed, "{text:?}");
        }
    }

    #[test]
    fn targets_order_as_their_texts_do() {
        let texts = [
            "",
            "01a11621-e285-7398-ab1c-b9af4864eaa2",
            "01a11621-e285-7398-ab1c-b9af4864eaa3",
            "01a11621-e285-7398-ab1c-b9af4864eaa2x",
            "01a11621-e285",
            "01A11621-E285-7398-AB1C-B9AF4864EAA2",
            "ex-a1b2",
            "ffffffff-ffff-ffff-ffff-ffffffffffff",
            "fffffffg",
            "~",
            "é",
        ];
        for a in texts {
            for b in texts {
                assert_eq!(Target::of(a).cmp(&Target::of(b)), a.cmp(b), "{a:?} {b:?}");
            }
        }
    }
}
