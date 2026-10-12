//! Painting: escaped first, coloured only when asked, and colour decided as the conventions say.

use std::collections::BTreeMap;

use super::*;

#[test]
fn a_part_is_escaped_then_painted() {
    let line = Line::new()
        .with(Tone::Id, "ex-1")
        .with(Tone::Plain, " ")
        .with(Tone::Strong, "esc\u{1b}[31mRED");
    assert_eq!(render(&line, false), "ex-1 esc\\x1b[31mRED");
    assert_eq!(
        render(&line, true),
        "\u{1b}[36mex-1\u{1b}[0m \u{1b}[1mesc\\x1b[31mRED\u{1b}[0m"
    );
}

#[test]
fn a_plain_line_with_a_check_mark_has_the_mark_painted() {
    let line = Line::from("✓ Closed ex-1: done");
    assert_eq!(render(&line, false), "✓ Closed ex-1: done");
    assert_eq!(
        render(&line, true),
        "\u{1b}[32m✓\u{1b}[0m Closed ex-1: done"
    );
}

#[test]
fn colour_follows_the_flag_then_the_conventions() {
    let with = |pairs: &[(&str, &str)]| {
        let vars: BTreeMap<String, OsString> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), OsString::from(value)))
            .collect();
        move |key: &str| vars.get(key).cloned()
    };
    let shows_on = |when, pairs: &[(&str, &str)]| colors(when, &with(pairs));
    let all = Colors {
        stdout: true,
        stderr: true,
    };
    assert_eq!(shows_on(When::Always, &[("NO_COLOR", "1")]), all);
    assert_eq!(
        shows_on(When::Never, &[("CLICOLOR_FORCE", "1")]),
        Colors::default()
    );
    assert_eq!(shows_on(When::Auto, &[("CLICOLOR_FORCE", "1")]), all);
    assert_eq!(
        shows_on(When::Auto, &[("CLICOLOR_FORCE", "1"), ("NO_COLOR", "1")]),
        Colors::default()
    );
    // an empty NO_COLOR says nothing
    assert_eq!(
        shows_on(When::Auto, &[("NO_COLOR", ""), ("CLICOLOR_FORCE", "1")]),
        all
    );
    assert!(!shows_on(When::Auto, &[("CLICOLOR_FORCE", "0")]).stdout);
    assert_eq!(When::named(OsStr::new("always")), Some(When::Always));
    assert_eq!(When::named(OsStr::new("yes")), None);
}
