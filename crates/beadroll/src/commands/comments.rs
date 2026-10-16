//! `bead comments`: an issue's comments, or `comments add ID TEXT` to record one.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};

use beadroll_core::code::Code;
use beadroll_core::json::{Object, Value};
use beadroll_core::text::{str_of, strip};
use beadroll_sync::writing_spent;

use super::{
    Context, Failure, Outcome, arg_text, encodable, need_tracker, read_utf8, resolve_arg, view_of,
};

/// `comments`' arguments.
pub(crate) struct CommentArgs {
    pub(crate) words: Vec<OsString>,
    pub(crate) message: Option<OsString>,
    pub(crate) file: Option<OsString>,
    pub(crate) author: Option<OsString>,
}

/// Lists an issue's comments, or (`add`) records one: from `-m`, from `-f` (`-` is standard
/// input), or the words after the issue.
pub(crate) fn comments(ctx: &Context<'_>, args: &CommentArgs) -> Result<Outcome, Failure> {
    let (action, words) = match args.words.split_first() {
        Some((first, rest)) if matches!(first.as_bytes(), b"add" | b"list") => {
            (first.as_bytes(), rest)
        }
        _ => (&b"list"[..], &args.words[..]),
    };
    let Some(key) = words.first() else {
        return Err(Failure::new(
            Code::Validation,
            "usage: bead comments [add] ISSUE [TEXT]",
        ));
    };
    if action == b"add" {
        return add(ctx, args, key, &words[1..]);
    }
    let st = view_of(ctx.ws, &[key])?;
    need_tracker(&st)?;
    let detail = st.detail(resolve_arg(&st, key)?);
    let comments = match detail.get("comments") {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    let mut lines: Vec<String> = comments
        .iter()
        .map(|c| {
            let or = |key: &str| {
                c.get(key)
                    .filter(|v| v.truthy())
                    .map_or_else(|| "?".to_owned(), str_of)
            };
            let created: String = or("created_at").chars().take(16).collect();
            let text = c.get("text").map(str_of).unwrap_or_default();
            format!("[{created}] {}: {text}", or("author"))
        })
        .collect();
    if lines.is_empty() {
        lines.push("No comments".to_owned());
    }
    Ok(Outcome::new(Value::Array(comments), lines).answered_from(st))
}

/// Records a comment on the issue `key` names.
fn add(
    ctx: &Context<'_>,
    args: &CommentArgs,
    key: &OsString,
    rest: &[OsString],
) -> Result<Outcome, Failure> {
    let (text, valid) = match (&args.message, &args.file) {
        (Some(message), _) if !message.is_empty() => arg_text(message),
        (_, Some(file)) if !file.is_empty() => {
            (read_utf8(file, OsStr::new("the comment file"))?, true)
        }
        _ => {
            let joined = rest
                .iter()
                .map(|w| w.as_bytes())
                .collect::<Vec<_>>()
                .join(&b' ');
            arg_text(&OsString::from_vec(joined))
        }
    };
    if strip(&text).is_empty() {
        return Err(Failure::new(Code::Validation, "comment text is required"));
    }
    let author = args
        .author
        .as_deref()
        .filter(|a| !a.is_empty())
        .map(arg_text);
    let ((id, issue_key), spent) = writing_spent(ctx.ws, ctx.actor, |tx| {
        let (uid, issue_key) = {
            let st = tx.state()?;
            need_tracker(st)?;
            let issue = resolve_arg(st, key)?;
            (issue.uid().to_string(), issue.key().to_owned())
        };
        let mut data = Object::new();
        data.insert("text".into(), Value::from(text.as_str()));
        if let Some((author, _)) = &author {
            data.insert("author".into(), Value::from(author.as_str()));
        }
        let event = tx.emit("comment", Some(&uid), data, None)?;
        encodable(valid && author.as_ref().is_none_or(|(_, ok)| *ok))?;
        let id = event.get("id").map(str_of).unwrap_or_default();
        Ok::<_, Failure>((id, issue_key))
    })?;
    let mut result = Object::new();
    result.insert("id".into(), Value::from(id));
    result.insert("issue_id".into(), Value::from(issue_key.as_str()));
    result.insert("text".into(), Value::from(text));
    Ok(Outcome::new(result, vec![format!("Comment added to {issue_key}")]).wrote_in(spent))
}
