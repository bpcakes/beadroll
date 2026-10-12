//! G50: `machines --name` never replaces an `aliases.json` it could not read with one holding only
//! the new name.

/// G50: `machines --name` treats an aliases.json it cannot read or parse as empty and replaces it
/// with a file holding only the new name: every other machine name is lost, with exit 0.
mod g50_machines_name_keeps_aliases_it_could_not_read {
    use std::fs::Permissions;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};

    use beadroll_core::json::{Format, write};
    use beadroll_test_support::{Json, Machine, Run, Sandbox, json, parse_json};

    /// `OTHERS`: the names of two other machines, which every test expects to survive.
    fn others() -> Json {
        json!({"abcd0001": "desktop", "abcd0002": "ci"})
    }

    /// The aliases as one-space-indented JSON with a final newline.
    fn well_formed() -> String {
        write(&others(), Format::INDENTED).expect("the names") + "\n"
    }

    /// Machine g50, its tracker initialised with the prefix `ex`, and the path
    /// of its `aliases.json`. Should a test fail with the file unreadable, the sandbox's drop makes
    /// it writable again, which is all its removal needs.
    fn setup(sandbox: &Sandbox) -> (Machine, PathBuf) {
        let machine = Machine::new(sandbox, "g50");
        machine.bead(["init", "--prefix", "ex"]);
        let aliases = machine.home().join("aliases.json");
        (machine, aliases)
    }

    /// `name_over`: `machines --name abcd0003=new` over an `aliases.json` holding `text` with the
    /// permissions `mode`: what it answered, and what the file holds after it, read back with mode
    /// 0600.
    fn name_over(machine: &Machine, aliases: &Path, text: &str, mode: u32) -> (Run, String) {
        std::fs::write(aliases, text).unwrap();
        std::fs::set_permissions(aliases, Permissions::from_mode(mode)).unwrap();
        let named = machine.run(["machines", "--name", "abcd0003=new", "--json"]);
        std::fs::set_permissions(aliases, Permissions::from_mode(0o600)).unwrap();
        let after = std::fs::read_to_string(aliases).unwrap();
        (named, after)
    }

    /// `check_kept`: `machines --name` over `text` either refuses, with a structured error and the
    /// file left exactly as it was, or goes through and keeps every name that was there.
    fn check_kept(machine: &Machine, aliases: &Path, label: &str, text: &str, mode: u32) {
        let (named, after) = name_over(machine, aliases, text, mode);
        assert!(!named.crashed(), "{label}: {}", named.stderr);
        if named.returncode() != 0 {
            // Refused: a structured error and the file left exactly as it was.
            let answer = parse_json(&named.stderr)
                .unwrap_or_else(|error| panic!("{label}: {error}: {}", named.stderr));
            let has_error = answer
                .as_object()
                .is_some_and(|members| members.contains_key("error"));
            assert!(has_error, "{label}: {}", named.stderr);
            assert_eq!(
                after, text,
                "{label}: refused, but aliases.json was rewritten anyway"
            );
        } else {
            // Accepted: every name that was there survives.
            let others = others();
            let names = others.as_object().expect("the names");
            for (ident, alias) in names {
                let alias = alias.as_str().expect("a name");
                assert!(
                    after.contains(&format!("\"{alias}\"")),
                    "{label}: {ident}={alias} lost; aliases.json is now {after:?}"
                );
            }
        }
    }

    /// G50: a hand edit that leaves a trailing comma (or a BOM) makes `machines --name` drop every
    /// other name; a well-formed file is merged.
    #[test]
    fn g50_hand_edited_file_with_a_trailing_comma_is_not_overwritten() {
        let sandbox = crate::sandbox();
        let (machine, aliases) = setup(&sandbox);
        let (named, after) = name_over(&machine, &aliases, &well_formed(), 0o600);
        assert_eq!(named.returncode(), 0, "{}", named.stderr);
        let merged = parse_json(&after).unwrap_or_else(|error| panic!("{error}: {after}"));
        let expected = json!({"abcd0001": "desktop", "abcd0002": "ci", "abcd0003": "new"});
        // dicts compare without regard to order, and so do the objects of `Json`
        assert_eq!(merged, expected);
        let cases = [
            (
                "trailing comma",
                "{\n \"abcd0001\": \"desktop\",\n \"abcd0002\": \"ci\",\n}\n".to_owned(),
            ),
            ("byte order mark", format!("\u{feff}{}", well_formed())),
        ];
        for (label, text) in cases {
            check_kept(&machine, &aliases, label, &text, 0o600);
        }
    }

    /// G50: an aliases.json that is not an object is replaced by one holding only the new name.
    #[test]
    fn g50_wrong_shape_is_not_overwritten() {
        let sandbox = crate::sandbox();
        let (machine, aliases) = setup(&sandbox);
        let text = "[[\"abcd0001\", \"desktop\"], [\"abcd0002\", \"ci\"]]\n";
        check_kept(&machine, &aliases, "list", text, 0o600);
    }

    /// G50: an aliases.json bead cannot read (EACCES) is replaced, since renaming needs only the
    /// directory to be writable. Skipped as root, who reads files of mode 0.
    #[test]
    fn g50_unreadable_file_is_not_overwritten() {
        if rustix::process::geteuid().is_root() {
            eprintln!("skipped: root reads mode-0 files");
            return;
        }
        let sandbox = crate::sandbox();
        let (machine, aliases) = setup(&sandbox);
        check_kept(&machine, &aliases, "EACCES", &well_formed(), 0);
    }
}
