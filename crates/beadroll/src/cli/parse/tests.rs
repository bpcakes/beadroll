//! What clap makes of command lines: the values commands read, their defaults, and the errors.

use std::os::unix::ffi::OsStringExt as _;

use super::*;
use crate::cli::spec::COMMANDS;

fn args(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

fn parsed(list: &[&str]) -> Namespace {
    match parse(&args(list)) {
        Parsed::Command(namespace) => namespace,
        other => panic!("{list:?}: {other:?}"),
    }
}

fn usage(list: &[&str]) -> String {
    match parse(&args(list)) {
        Parsed::Usage(usage) => String::from_utf8_lossy(&usage.message).into_owned(),
        other => panic!("{list:?}: {other:?}"),
    }
}

fn text(namespace: &Namespace, dest: &str) -> Option<String> {
    namespace
        .text(dest)
        .map(|t| t.to_string_lossy().into_owned())
}

fn texts(namespace: &Namespace, dest: &str) -> Vec<String> {
    namespace
        .texts(dest)
        .iter()
        .map(|t| t.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn the_table_builds_a_command_line_clap_accepts() {
    command(None).debug_assert();
    command(Some(40)).debug_assert();
    for spec in &COMMANDS {
        for arg in spec.args {
            let list = matches!(arg.nargs, Nargs::ZeroOrMore | Nargs::OneOrMore)
                || arg.action == Action::Append;
            assert!(!list || arg.kind == Kind::Text, "{}", arg.dest);
        }
    }
}

#[test]
fn an_unambiguous_prefix_names_its_option() {
    let ns = parsed(&[
        "create",
        "--description=",
        "--tit=t",
        "x",
        "--acceptance-c",
        "a",
    ]);
    assert_eq!(ns.spec.name, "create");
    assert_eq!(text(&ns, "description"), Some(String::new()));
    assert_eq!(text(&ns, "title"), Some("t".into()));
    assert_eq!(text(&ns, "title_pos"), Some("x".into()));
    assert_eq!(text(&ns, "acceptance_criteria"), Some("a".into()));
    assert_eq!(
        texts(&parsed(&["list", "--stat", "closed"]), "status"),
        ["closed"]
    );
    assert!(usage(&["create", "--desc", "x"]).contains("--desc"));
    assert_eq!(
        text(&parsed(&["create", "--", "--desc"]), "title_pos"),
        Some("--desc".into())
    );
}

#[test]
fn the_last_value_wins_lists_keep_every_value_and_a_scope_is_given_once() {
    let ns = parsed(&[
        "create",
        "x",
        "--title",
        "a",
        "--title",
        "b",
        "-l",
        "p",
        "-lq",
        "--label=r",
    ]);
    assert_eq!(text(&ns, "title"), Some("b".into()));
    assert_eq!(texts(&ns, "labels"), ["p", "q", "r"]);
    assert!(parsed(&["create", "x"]).texts("labels").is_empty());
    assert_eq!(
        usage(&["list", "--parent", "a", "--parent", "b"]),
        "the argument '--parent <ID>' cannot be used multiple times"
    );
    assert!(usage(&["ready", "x", "--epic", "y"]).contains("cannot be used with"));
    assert!(usage(&["ready", "--epic", "a", "--parent", "b"]).contains("cannot be used with"));
    let ns = parsed(&["ready", "--epic", "a"]);
    assert_eq!(text(&ns, "epic"), Some("a".into()));
    assert_eq!(ns.text("root"), None);
}

#[test]
fn positionals_and_options_mix() {
    assert_eq!(
        texts(&parsed(&["update", "A", "--status", "open", "B"]), "ids"),
        ["A", "B"]
    );
    assert!(usage(&["create", "a", "b"]).contains("'b'"));
    let ns = parsed(&["dep", "add", "--type", "blocks", "A", "B"]);
    assert_eq!(texts(&ns, "words"), ["add", "A", "B"]);
    assert_eq!(text(&ns, "type"), Some("blocks".into()));
    let ns = parsed(&["comments", "--", "add", "X", "-m"]);
    assert_eq!(texts(&ns, "words"), ["add", "X", "-m"]);
    assert!(parsed(&["dep"]).texts("words").is_empty());
}

#[test]
fn an_options_value_may_start_with_a_dash_and_a_positionals_after_a_double_dash() {
    assert_eq!(
        text(&parsed(&["create", "--", "- dash title"]), "title_pos"),
        Some("- dash title".into())
    );
    assert!(usage(&["create", "- dash title"]).contains("'--"));
    let ns = parsed(&["comments", "add", "X", "-m", "- note", "--offline"]);
    assert_eq!(texts(&ns, "words"), ["add", "X"]);
    assert_eq!(text(&ns, "message"), Some("- note".into()));
    assert!(ns.flag("offline"));
    assert_eq!(
        text(&parsed(&["create", "-p", "-1", "t"]), "priority"),
        Some("-1".into())
    );
    assert_eq!(
        text(&parsed(&["create", "-pfoo", "t"]), "priority"),
        Some("foo".into())
    );
    assert_eq!(
        text(&parsed(&["create", "--title", "--notes", "x"]), "title"),
        Some("--notes".into())
    );
    assert_eq!(
        parsed(&["log", "--limit", "-2"]).int("limit"),
        Some(&Int::from(-2))
    );
    // an issue id is not free text: a mistyped option is an error, with a suggestion
    let mistyped = usage(&["update", "A", "--stauts", "open"]);
    assert!(mistyped.contains("'--status'"), "{mistyped}");
}

#[test]
fn flags_take_no_value_and_short_ones_combine() {
    assert!(usage(&["create", "--silent=yes", "t"]).contains("'yes'"));
    let ns = parsed(&["list", "-as", "open"]);
    assert!(ns.flag("all"));
    assert_eq!(texts(&ns, "status"), ["open"]);
    assert!(parsed(&["list", "-asopen"]).flag("all"));
    assert!(parsed(&["init", "--tracked"]).flag("tracked"));
    assert!(!parsed(&["init"]).flag("tracked"));
}

#[test]
fn numbers_are_whole_numbers_and_defaults_apply() {
    let ns = parsed(&["list", "--limit", " 10 "]);
    assert_eq!(ns.int("limit"), Some(&Int::from(10)));
    assert_eq!(parsed(&["list"]).int("limit"), Some(&Int::from(50)));
    assert_eq!(parsed(&["ready"]).int("limit"), Some(&Int::from(0)));
    assert_eq!(parsed(&["log"]).int("limit"), Some(&Int::from(30)));
    assert_eq!(parsed(&["create", "t"]).int("estimate"), None);
    assert!(usage(&["list", "-a", "--limit", "x"]).contains("not a whole number"));
    let many = "1".repeat(40);
    assert!(usage(&["create", "--estimate", &many]).contains("not a whole number"));
    assert!(usage(&["list", "-a", "--limit", "1_0"]).contains("not a whole number"));
    assert_eq!(
        parsed(&["list", "--limit", " +7 "]).int("limit"),
        Some(&Int::from(7))
    );
}

#[test]
fn errors_name_what_is_missing_or_unknown_and_suggest() {
    assert!(usage(&["show"]).contains("<ID>"));
    assert!(usage(&["search"]).contains("<TEXT>"));
    let unknown = usage(&["lis"]);
    assert!(
        unknown.contains("'lis'") && unknown.contains("'list'"),
        "{unknown}"
    );
    let flag = usage(&["list", "--stauts", "open"]);
    assert!(flag.contains("'--status'"), "{flag}");
    assert!(!usage(&["--bogus"]).is_empty());
}

#[test]
fn help_is_answered_for_the_command_it_follows() {
    let page = |list: &[&str]| match parse(&args(list)) {
        Parsed::Help(Page::Command(spec)) => spec.name.to_owned(),
        Parsed::Help(Page::Top) => "bead".to_owned(),
        other => panic!("{list:?}: {other:?}"),
    };
    assert_eq!(page(&["create", "--help"]), "create");
    assert_eq!(page(&["show", "--he"]), "show");
    assert_eq!(page(&["list", "-h"]), "list");
    assert_eq!(page(&["-h", "create"]), "bead");
    assert_eq!(page(&["--he", "create"]), "bead");
}

#[test]
fn bytes_that_are_not_utf8_stay_as_given() {
    let raw = OsString::from_vec(vec![b'x', 0xff]);
    let ns = match parse(&[OsString::from("create"), raw.clone()]) {
        Parsed::Command(ns) => ns,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        ns.text("title_pos").map(OsStr::as_bytes),
        Some(raw.as_bytes())
    );
    assert!(matches!(
        parse(&["list".into(), "--limit".into(), raw]),
        Parsed::Usage(_)
    ));
}

#[test]
fn a_receipt_signs_every_destination_with_its_value_or_default() {
    assert_eq!(
        parsed(&["claim", "A", "B"]).asked(),
        "{\"command\": \"claim\", \"ids\": [\"A\", \"B\"], \"offline\": false, \"remote\": null}"
    );
    // close writes no receipt; its namespace shows a flag's default as well as a given value
    assert_eq!(
        parsed(&["close", "A", "-r", "done", "--force"]).asked(),
        "{\"command\": \"close\", \"force\": true, \"ids\": [\"A\"], \"offline\": false, \
         \"reason\": \"done\", \"recursive\": false, \"remote\": null}"
    );
}
