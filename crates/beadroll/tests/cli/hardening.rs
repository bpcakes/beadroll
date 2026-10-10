//! `tests/test_hardening.py`'s black-box tests, ported under the inventory's naming rule.

mod g56_closed_stdin_is_a_structured_error {
    use std::process::Command;

    use beadroll_test_support::{Machine, parse_json};

    /// `arg` quoted for the shell.
    fn quoted(arg: &str) -> String {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }

    /// G56: `-` names standard input; with descriptor 0 closed (Python's `sys.stdin is None`),
    /// `comments add -f -`, `update`/`create --description-file -` and `import -` answer with a
    /// structured error and write nothing, as they do when it is open but unreadable; an open
    /// standard input still works.
    #[test]
    fn closed_stdin_for_every_dash_reader_is_a_structured_error() {
        let sandbox = crate::sandbox();
        let m = Machine::new(&sandbox, "g56");
        m.bead(["init", "--prefix", "cs"]);
        let key = m.bead(["create", "seed"]).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let before = m.export();
        let cases: [(&str, &[&str]); 4] = [
            ("comments add -f -", &["comments", "add", &key, "-f", "-"]),
            (
                "update --description-file -",
                &["update", &key, "--description-file", "-"],
            ),
            (
                "create --description-file -",
                &["create", "other", "--description-file", "-"],
            ),
            ("import -", &["import", "-"]),
        ];
        let bead = m.executable().display().to_string();
        for (label, args) in cases {
            let mut words = vec![quoted(&bead)];
            words.extend(args.iter().map(|a| quoted(a)));
            words.push("--json".to_owned());
            let script = format!("{} <&-", words.join(" "));
            let out = Command::new("/bin/sh")
                .args(["-c", &script])
                .current_dir(m.checkout())
                .env_clear()
                .envs(m.env(None, "codex"))
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(
                !stderr.contains("Traceback") && !stderr.contains("panicked at"),
                "{label} with stdin closed crashed: {stderr}"
            );
            assert_eq!(out.status.code(), Some(1), "{label}: {stderr}");
            let error = parse_json(&stderr).unwrap();
            let code = error["error"]["code"].as_str().unwrap_or_default();
            assert!(["IO", "VALIDATION"].contains(&code), "{label}: {stderr}");
        }
        assert_eq!(m.export(), before);
        // an open stdin still works
        m.cmd(["update", &key, "--description-file", "-"])
            .stdin("from stdin")
            .json();
        assert_eq!(m.show(&key)["description"].as_str(), Some("from stdin"));
    }
}
