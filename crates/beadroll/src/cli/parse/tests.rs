//! Expected parser results and errors for command-line arguments.

use super::*;
use crate::cli::spec::{COMMANDS, Kind};

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
        Parsed::Usage(message) => String::from_utf8(message).unwrap(),
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
fn every_parser_has_one_positional_at_most() {
    // consume_positionals matches one positional's pattern at a time
    for command in &COMMANDS {
        let positionals = command.args.iter().filter(|a| a.is_positional()).count();
        assert!(positionals <= 1, "{}", command.name);
        for arg in command.args {
            let list = matches!(arg.nargs, Nargs::ZeroOrMore | Nargs::OneOrMore)
                || arg.action == Action::Append;
            assert!(!list || arg.kind == Kind::Text, "{}", arg.dest);
        }
    }
}

#[test]
fn prefixes_are_options_when_they_name_one_and_errors_when_they_name_several() {
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
        usage(&["create", "--desc", "x"]),
        "ambiguous option: --desc could match --description, --description-file"
    );
    assert_eq!(
        usage(&["create", "--acc=x", "t"]),
        "ambiguous option: --acc=x could match --acceptance-criteria, --acceptance"
    );
    assert_eq!(
        usage(&["create", "-l", "a", "--lab", "e", "t"]),
        "ambiguous option: --lab could match --labels, --label"
    );
    assert_eq!(
        text(&parsed(&["create", "--", "--desc"]), "title_pos"),
        Some("--desc".into())
    );
}

#[test]
fn positionals_are_taken_before_the_first_option_after_them() {
    assert_eq!(
        usage(&["update", "A", "--status", "open", "B"]),
        "unrecognized arguments: B"
    );
    assert_eq!(usage(&["create", "a", "b"]), "unrecognized arguments: b");
    assert_eq!(
        usage(&["create", "x", "--", "y"]),
        "unrecognized arguments: y"
    );
    assert_eq!(
        usage(&["dep", "add", "--type", "blocks", "A", "B"]),
        "unrecognized arguments: A B"
    );
    let ns = parsed(&["dep", "add", "A", "B", "--type", "blocks"]);
    assert_eq!(texts(&ns, "words"), ["add", "A", "B"]);
    let ns = parsed(&["comments", "--", "add", "X", "-m"]);
    assert_eq!(texts(&ns, "words"), ["add", "X", "-m"]);
    assert!(parsed(&["dep"]).texts("words").is_empty());
}

#[test]
fn values_with_a_dash_are_values_when_python_reads_them_so() {
    assert_eq!(
        text(&parsed(&["create", "- dash title"]), "title_pos"),
        Some("- dash title".into())
    );
    assert_eq!(
        text(
            &parsed(&["comments", "add", "X", "-m", "- item"]),
            "message"
        ),
        Some("- item".into())
    );
    assert_eq!(
        text(&parsed(&["create", "-p", "-1", "t"]), "priority"),
        Some("-1".into())
    );
    assert_eq!(
        text(&parsed(&["create", "-1x"]), "title_pos"),
        Some("-1x".into())
    );
    assert_eq!(
        text(&parsed(&["create", "-"]), "title_pos"),
        Some("-".into())
    );
    assert_eq!(
        text(&parsed(&["create", "-pfoo", "t"]), "priority"),
        Some("foo".into())
    );
    assert_eq!(
        text(&parsed(&["create", "-p=foo", "t"]), "priority"),
        Some("foo".into())
    );
    assert_eq!(
        text(&parsed(&["create", "--title=", "x"]), "title"),
        Some(String::new())
    );
    assert_eq!(
        usage(&["create", "--title", "--notes", "x"]),
        "argument --title: expected one argument"
    );
    assert_eq!(
        usage(&["list", "-s"]),
        "argument -s/--status: expected one argument"
    );
}

#[test]
fn flags_take_no_value_and_short_ones_combine() {
    assert_eq!(
        usage(&["create", "--silent=yes", "t"]),
        "argument --silent: ignored explicit argument 'yes'"
    );
    assert_eq!(
        usage(&["list", "-a=1"]),
        "argument -a/--all: ignored explicit argument '1'"
    );
    assert_eq!(
        usage(&["list", "--all=1"]),
        "argument -a/--all: ignored explicit argument '1'"
    );
    assert_eq!(usage(&["list", "-aX"]), "unrecognized arguments: -X");
    let ns = parsed(&["list", "-as", "open"]);
    assert!(ns.flag("all"));
    assert_eq!(texts(&ns, "status"), ["open"]);
    assert!(parsed(&["list", "-asopen"]).flag("all"));
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
        "argument --parent: given more than once"
    );
    assert_eq!(
        usage(&["ready", "--par", "a", "--parent", "b"]),
        "argument --parent: given more than once"
    );
    assert_eq!(
        usage(&["ready", "x", "--epic", "y"]),
        "argument --epic: not allowed with argument ID"
    );
    assert_eq!(
        usage(&["ready", "--epic", "y", "x"]),
        "argument ID: not allowed with argument --epic"
    );
    assert_eq!(
        usage(&["ready", "--epic", "a", "--parent", "b"]),
        "argument --parent: not allowed with argument --epic"
    );
    let ns = parsed(&["ready", "--epic", "a"]);
    assert_eq!(text(&ns, "epic"), Some("a".into()));
    assert_eq!(ns.text("root"), None);
}

#[test]
fn numbers_are_read_as_int_reads_them_and_defaults_apply() {
    let ns = parsed(&["list", "--limit", " 1_0 "]);
    assert_eq!(ns.int("limit"), Some(&Int::from(10)));
    assert_eq!(parsed(&["list"]).int("limit"), Some(&Int::from(50)));
    assert_eq!(parsed(&["ready"]).int("limit"), Some(&Int::from(0)));
    assert_eq!(parsed(&["log"]).int("limit"), Some(&Int::from(30)));
    assert_eq!(parsed(&["create", "t"]).int("estimate"), None);
    assert_eq!(
        usage(&["list", "-a", "--limit", "x"]),
        "argument --limit: invalid int value: 'x'"
    );
    let many = "1".repeat(4301);
    assert_eq!(
        usage(&["create", "--estimate", &many]),
        format!("argument --estimate: invalid int value: '{many}'")
    );
    assert!(parsed(&["init", "--tracked"]).flag("tracked"));
    assert!(!parsed(&["init"]).flag("tracked"));
}

#[test]
fn errors_come_in_the_order_argparse_meets_them() {
    assert_eq!(
        usage(&["show"]),
        "the following arguments are required: ids"
    );
    assert_eq!(
        usage(&["search"]),
        "the following arguments are required: query"
    );
    assert_eq!(
        usage(&["--bogus"]),
        "the following arguments are required: command"
    );
    assert_eq!(
        usage(&["--bogus", "create"]),
        "unrecognized arguments: --bogus"
    );
    assert_eq!(
        usage(&["--bogus", "show"]),
        "the following arguments are required: ids"
    );
    assert_eq!(
        usage(&["create", "--estimate", "x", "--help"]),
        "argument --estimate: invalid int value: 'x'"
    );
    assert!(usage(&["nonsense"]).starts_with(
        "argument command: invalid choice: 'nonsense' (choose from 'init', 'import', 'export', "
    ));
    assert!(usage(&["-=x"]).starts_with("ambiguous option: -=x could match -h, --help"));
}

#[test]
fn help_is_answered_when_it_is_reached() {
    let page = |list: &[&str]| match parse(&args(list)) {
        Parsed::Help(Page::Command(spec)) => spec.name.to_owned(),
        Parsed::Help(Page::Top) => "bead".to_owned(),
        other => panic!("{list:?}: {other:?}"),
    };
    assert_eq!(page(&["create", "--bogus", "--help"]), "create");
    assert_eq!(page(&["show", "--he"]), "show");
    assert_eq!(page(&["list", "-hx"]), "list");
    assert_eq!(page(&["-h", "create"]), "bead");
    assert_eq!(page(&["--he", "create"]), "bead");
    assert_eq!(page(&["--", "help", "-h"]), "help");
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
    let message = |argv: Vec<OsString>| match parse(&argv) {
        Parsed::Usage(message) => message,
        other => panic!("{other:?}"),
    };
    let limit = message(vec!["list".into(), "--limit".into(), raw.clone()]);
    assert_eq!(limit, b"argument --limit: invalid int value: 'x\\udcff'");
    // Keep the original bytes: human output substitutes `?`, while JSON escapes them.
    let extra = message(vec!["create".into(), "t".into(), raw]);
    assert_eq!(extra, b"unrecognized arguments: x\xff");
    let ambiguous = message(vec![
        "create".into(),
        OsString::from_vec(b"--de=\xff".to_vec()),
    ]);
    assert_eq!(
        ambiguous,
        b"ambiguous option: --de=\xff could match --description, --description-file, --design, \
          --deps"
    );
}
