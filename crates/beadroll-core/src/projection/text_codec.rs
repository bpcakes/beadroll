//! An issue's texts as bytes of their own (roll-lqjj.6): its registers in their order, the long
//! ones with their values, which are its long fields' values too (roll-lqjj.10: each written
//! once), its comments, its extras and the metadata of each addition behind each of its edges
//! (roll-lqjj.39), which a projection keeps in its text file; and what an issue keeps of them
//! unread ([`Stored`]). An edge and the events that added it are in the state part, which an
//! issue's edges' metadata read in must match, as its long fields' registers must match
//! [`Stored::tags`]: nothing more of them is kept unread.

use crate::event::Field;
use crate::id::EventId;
use crate::json::Object;
use crate::query::split;
use crate::state::{Comment, DepTags, Issue, Meta, Stored, TextPlace, Texts, VecMap, is_text};

use super::codec::{Reader, Unreadable, Writer};
use super::state_codec::{read_tags, write_tags};

/// The metadata of an addition whose issue's texts are not in it: never written, since only texts
/// in their issue are.
static NO_MEMBERS: std::sync::LazyLock<Object> = std::sync::LazyLock::new(Object::new);

/// What an issue whose texts are in it keeps of them once they are written at `place`: the events
/// behind each long field's register, and the long fields disputed.
pub(super) fn summary(issue: &Issue, place: TextPlace) -> Stored {
    let (mut tags, mut disputed) = (Vec::new(), Vec::new());
    for (field, register) in issue.reg.iter().filter(|(field, _)| is_text(**field)) {
        tags.push((*field, register.iter().map(|(id, _)| *id).collect()));
        if split(*field, register) {
            disputed.push(*field);
        }
    }
    Stored {
        place,
        tags,
        disputed,
    }
}

/// An issue's texts as bytes of their own: its registers in their order (the long ones with their
/// values, which its long fields show), its comments, its extras and its edges' metadata.
pub(crate) fn write_texts(issue: &Issue) -> Vec<u8> {
    let mut w = Writer::default();
    w.count(issue.reg.len());
    for (field, tags) in &issue.reg {
        w.count(field.index());
        w.flag(is_text(*field));
        if is_text(*field) {
            write_tags(&mut w, tags);
        }
    }
    w.count(issue.comments.len());
    for comment in &issue.comments {
        match comment {
            Comment::Native {
                uid,
                seq,
                author,
                text,
                where_,
            } => {
                w.byte(0);
                w.id(*uid);
                w.number(*seq);
                w.name(author);
                w.text(text);
                w.optional_shared(where_.as_ref());
            }
            Comment::Imported(members) => {
                w.byte(1);
                w.object(members);
            }
        }
    }
    w.flag(issue.extra.is_some());
    if let Some(extra) = &issue.extra {
        w.object(extra);
    }
    write_edge_metadata(&mut w, &issue.dep_tags);
    w.finish()
}

/// The metadata of each addition behind each edge of `edges`, in their order, each with the event
/// that added it. An issue's texts are written only once they are in it ([`Texts::here`]), every
/// addition's metadata with them.
fn write_edge_metadata(w: &mut Writer, edges: &DepTags) {
    w.count(edges.len());
    for tags in edges.values() {
        w.count(tags.len());
        for (id, meta) in tags {
            w.id(*id);
            w.object(meta.members().unwrap_or(&NO_MEMBERS));
        }
    }
}

/// The metadata [`write_edge_metadata`] wrote, each edge's additions by id.
fn read_edge_metadata(r: &mut Reader<'_>) -> Result<Vec<Vec<(EventId, Object)>>, Unreadable> {
    let n = r.count()?;
    let mut edges = Vec::with_capacity(n);
    for _ in 0..n {
        let m = r.count()?;
        let mut tags = Vec::with_capacity(m);
        for _ in 0..m {
            tags.push((r.id()?, r.object()?));
        }
        edges.push(tags);
    }
    Ok(edges)
}

/// Whether `metadata`, read from an issue's texts, is of `edges`, the issue's: the same additions
/// of each edge, in order.
fn of_edges(metadata: &[Vec<(EventId, Object)>], edges: &DepTags) -> bool {
    metadata.len() == edges.len()
        && (metadata.iter().zip(edges.values())).all(|(read, tags)| {
            read.len() == tags.len()
                && (read.iter().zip(tags.iter())).all(|((id, _), (tag, _))| id == tag)
        })
}

/// Reads `issue`'s texts in from `bytes`, the ones [`write_texts`] wrote at `place`: they must be
/// whole and have the events its long fields' registers were written with, and the additions its
/// edges have. Nothing changes when they are not.
pub(crate) fn read_texts(
    issue: &mut Issue,
    bytes: &[u8],
    place: TextPlace,
) -> Result<(), Unreadable> {
    let Texts::There(stored) = &issue.texts else {
        return Ok(());
    };
    if stored.place != place {
        return Err(Unreadable("texts of another place"));
    }
    let mut r = Reader::new(bytes);
    let mut order = Vec::new();
    for _ in 0..r.count()? {
        let field = Field::ALL.get(r.index()?).copied();
        let field = field.ok_or(Unreadable("a field"))?;
        let long = r.flag()?;
        if long != is_text(field) {
            return Err(Unreadable("a long field"));
        }
        order.push((field, long.then(|| read_tags(&mut r)).transpose()?));
    }
    let mut comments = Vec::new();
    for _ in 0..r.count()? {
        comments.push(match r.byte()? {
            0 => Comment::Native {
                uid: r.id()?,
                seq: r.number()?,
                author: r.name()?,
                text: r.text()?,
                where_: r.optional_shared()?,
            },
            1 => Comment::Imported(r.object()?),
            _ => return Err(Unreadable("a comment")),
        });
    }
    let extra = r.flag()?.then(|| r.object()).transpose()?.map(Box::new);
    let metadata = read_edge_metadata(&mut r)?;
    r.done()?;
    // the texts this issue was written with: the same events behind each long field's register
    let tags: Vec<(Field, Vec<EventId>)> = (order.iter())
        .filter_map(|(field, tags)| {
            tags.as_ref()
                .map(|tags| (*field, tags.iter().map(|(id, _)| *id).collect()))
        })
        .collect();
    if tags != stored.tags || !of_edges(&metadata, &issue.dep_tags) {
        return Err(Unreadable("texts of another issue"));
    }
    // registers in the order they were made: as the texts were written, then any made since
    let mut held = std::mem::take(&mut issue.reg).into_entries();
    let mut reg = Vec::with_capacity(order.len() + held.len());
    for (field, tags) in order {
        match tags {
            Some(tags) => reg.push((field, tags)),
            None => {
                if let Some(at) = held.iter().position(|(was, _)| *was == field) {
                    reg.push(held.remove(at));
                }
            }
        }
    }
    reg.extend(held);
    issue.reg = VecMap::from_distinct(reg);
    issue.comments = comments;
    issue.extra = extra;
    for (tags, read) in issue.dep_tags.values_mut().zip(metadata) {
        for (meta, (_, members)) in tags.values_mut().zip(read) {
            *meta = Meta::of(members);
        }
    }
    issue.texts = Texts::Read(place);
    Ok(())
}
