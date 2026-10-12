//! Normalises transcript ids, keys, machines, commits, times and sandbox paths into placeholders
//! numbered by first appearance, so equivalent outputs compare equally across runs.

use std::collections::BTreeMap;

/// Placeholders, numbered per kind in the order values first appear.
#[derive(Default)]
pub(crate) struct Normaliser {
    roots: Vec<String>,
    seen: BTreeMap<(&'static str, String), usize>,
    counts: BTreeMap<&'static str, usize>,
}

impl Normaliser {
    /// A normaliser that writes each of `roots` (the sandbox's directory, as given and resolved)
    /// as `<ROOT>`.
    pub(crate) fn new(roots: Vec<String>) -> Normaliser {
        let mut roots = roots;
        // the longest first, so a resolved path is not half replaced by a shorter spelling
        roots.sort_by_key(|root| std::cmp::Reverse(root.len()));
        Normaliser {
            roots,
            ..Normaliser::default()
        }
    }

    fn placeholder(&mut self, kind: &'static str, value: &str) -> String {
        let next = self.counts.get(kind).copied().unwrap_or(0) + 1;
        let number = *self
            .seen
            .entry((kind, value.to_owned()))
            .or_insert_with(|| next);
        if number == next {
            self.counts.insert(kind, next);
        }
        format!("<{kind}{number}>")
    }

    /// `text` with every varying value replaced.
    pub(crate) fn apply(&mut self, text: &str) -> String {
        let mut text = text.to_owned();
        for root in self.roots.clone() {
            text = text.replace(&root, "<ROOT>");
        }
        let bytes = text.as_bytes();
        let mut out = String::with_capacity(text.len());
        let mut at = 0;
        while at < bytes.len() {
            let starts = at == 0 || !is_word(bytes[at - 1]);
            if starts && let Some((kind, len)) = token_at(bytes, at) {
                let value = &text[at..at + len];
                match kind {
                    "TIME" => out.push_str("<TIME>"),
                    "KEY" => {
                        // a minted key: its prefix stays, its digest does not
                        let shown = self.placeholder("KEY", value);
                        out.push_str(&shown);
                    }
                    _ => {
                        let shown = self.placeholder(kind, value);
                        out.push_str(&shown);
                    }
                }
                at += len;
                continue;
            }
            let c = text[at..].chars().next().unwrap_or_default();
            out.push(c);
            at += c.len_utf8().max(1);
        }
        out
    }
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_hex(b: u8) -> bool {
    b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
}

/// The kind and length of the varying value at `at`, if one starts there and ends at a word
/// boundary.
fn token_at(bytes: &[u8], at: usize) -> Option<(&'static str, usize)> {
    let rest = &bytes[at..];
    let ends = |len: usize| rest.get(len).is_none_or(|b| !is_word(*b));
    // an event id, a UUIDv7: 8-4-4-4-12
    let groups = [8, 4, 4, 4, 12];
    let mut len = 0;
    let mut uuid = true;
    for (index, size) in groups.iter().enumerate() {
        if index > 0 {
            uuid &= rest.get(len) == Some(&b'-');
            len += 1;
        }
        uuid &= rest
            .get(len..len + size)
            .is_some_and(|g| g.iter().all(|b| is_hex(*b)));
        len += size;
    }
    if uuid && ends(len) {
        return Some(("ID", len));
    }
    if let Some(len) = time_at(rest)
        && ends(len)
    {
        return Some(("TIME", len));
    }
    if let Some(digest) = rest.strip_prefix(b"local-") {
        let hex = digest.iter().take_while(|b| is_hex(**b)).count();
        if hex == 12 && ends(6 + hex) {
            return Some(("WORKSPACE", 6 + hex));
        }
    }
    let hex = rest.iter().take_while(|b| is_hex(**b)).count();
    if ends(hex) {
        match hex {
            40 | 12 => return Some(("COMMIT", hex)),
            16 => return Some(("WORKSPACE", hex)),
            8 => return Some(("MACHINE", hex)),
            _ => {}
        }
    }
    // a minted key: the prefix, then four or more base-36 digits
    if let Some(digest) = rest.strip_prefix(b"ex-") {
        let run = digest
            .iter()
            .take_while(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
            .count();
        if run >= 4 && ends(3 + run) {
            return Some(("KEY", 3 + run));
        }
    }
    None
}

/// The length of an ISO date or time at the start of `rest`: `YYYY-MM-DD`, then perhaps `THH:MM`,
/// `:SS`, a fraction and `Z`.
fn time_at(rest: &[u8]) -> Option<usize> {
    let date = b"dddd-dd-dd";
    let fits = |at: usize, wanted: u8| {
        rest.get(at).is_some_and(|b| {
            if wanted == b'd' {
                b.is_ascii_digit()
            } else {
                *b == wanted
            }
        })
    };
    if !(0..date.len()).all(|at| fits(at, date[at])) {
        return None;
    }
    let clock = b"Tdd:dd";
    if !(0..clock.len()).all(|at| fits(date.len() + at, clock[at])) {
        return Some(date.len());
    }
    let mut len = date.len() + clock.len();
    if fits(len, b':') && fits(len + 1, b'd') && fits(len + 2, b'd') {
        len += 3;
    }
    if fits(len, b'.') {
        len += 1;
        while fits(len, b'd') {
            len += 1;
        }
    }
    if fits(len, b'Z') {
        len += 1;
    }
    Some(len)
}

#[test]
fn varying_values_become_numbered_placeholders() {
    let mut normal = Normaliser::new(vec!["/tmp/box".to_owned()]);
    let text = "ex-3k9x.1 by codex:t@1a2b3c4d at 2026-10-10T12:34:56.789Z in /tmp/box/home \
                (0199b2c4-1d2e-7f00-8a1b-0123456789ab, ex-3k9x, ex-a.1, deadbeefdeadbeef) \
                [2026-10-10T12:34] 0199b2c4-1d2e-7f00-8a1b-0123456789ac created 2026-01-01";
    assert_eq!(
        normal.apply(text),
        "<KEY1>.1 by codex:t@<MACHINE1> at <TIME> in <ROOT>/home (<ID1>, <KEY1>, ex-a.1, \
         <WORKSPACE1>) [<TIME>] <ID2> created <TIME>"
    );
}
