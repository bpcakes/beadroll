//! An issue's texts as bytes of their own (roll-lqjj.6): its long fields' values, its registers in
//! their order (the long ones with their values), its comments and its extras, which a projection
//! keeps in its text file; and what an issue keeps of them unread ([`Stored`]).

use crate::event::Field;
use crate::id::EventId;
use crate::query::split;
use crate::state::{Comment, Issue, Stored, TEXT_FIELDS, TextPlace, Texts, VecMap, is_text};

use super::codec::{Reader, Unreadable, Writer};
use super::state_codec::{read_tags, write_tags};

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

/// An issue's texts as bytes of their own: its long fields' values, its registers in their order
/// (the long ones with their values), its comments and its extras.
pub(crate) fn write_texts(issue: &Issue) -> Vec<u8> {
    let mut w = Writer::default();
    for field in TEXT_FIELDS {
        w.optional_value(issue.field(field));
    }
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
    w.finish()
}

/// Reads `issue`'s texts in from `bytes`, the ones [`write_texts`] wrote at `place`: they must be
/// whole and have the events its long fields' registers were written with. Nothing changes when
/// they are not.
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
    let mut values = Vec::with_capacity(TEXT_FIELDS.len());
    for field in TEXT_FIELDS {
        values.push((field, r.optional_value()?));
    }
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
    r.done()?;
    // the texts this issue was written with: the same events behind each long field's register
    let tags: Vec<(Field, Vec<EventId>)> = (order.iter())
        .filter_map(|(field, tags)| {
            tags.as_ref()
                .map(|tags| (*field, tags.iter().map(|(id, _)| *id).collect()))
        })
        .collect();
    if tags != stored.tags {
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
    for (field, value) in values {
        issue.set(field, value);
    }
    issue.comments = comments;
    issue.extra = extra;
    issue.texts = Texts::Read(place);
    Ok(())
}
