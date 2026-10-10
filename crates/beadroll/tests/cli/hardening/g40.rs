//! G40: an '@' after a URL's authority is not user-info: bead keeps such a remote as given, and
//! drops only the password that does sit in the user-info.

mod g40_at_sign_after_the_authority {
    use std::path::PathBuf;

    use beadroll_test_support::{Json, Machine, git, parse_json};

    /// `next(machine.home.glob("*/meta.json"))`: the `meta.json` of the first store in `machine`'s
    /// home, in directory order.
    fn meta(machine: &Machine) -> PathBuf {
        let entries = std::fs::read_dir(machine.home()).expect("the machine's home");
        entries
            .map(|entry| entry.expect("a directory entry").path().join("meta.json"))
            .find(|path| path.exists())
            .expect("a meta.json in the machine's home")
    }

    /// `_g40_init`: `init --remote URL` on a new machine: what init printed (its standard output,
    /// then its standard error), meta.json's remote (`None` when it has none) and info's remote.
    fn init(machine: &Machine, url: &str) -> (String, Option<Json>, Json) {
        let made = machine.run(["init", "--prefix", "ex", "--remote", url]);
        assert_eq!(made.returncode(), 0, "{}", made.stderr);
        let info = machine.run(["info", "--json"]);
        assert_eq!(info.returncode(), 0, "{}", info.stderr);
        let meta = std::fs::read_to_string(meta(machine)).expect("meta.json");
        let meta = parse_json(&meta).unwrap_or_else(|error| panic!("meta.json: {error}: {meta}"));
        let shown = parse_json(&info.stdout)
            .unwrap_or_else(|error| panic!("info --json: {error}: {}", info.stdout));
        let used = shown.get("remote").cloned();
        let used = used.unwrap_or_else(|| panic!("info has no remote: {}", info.stdout));
        (
            made.stdout + &made.stderr,
            meta.get("remote").cloned(),
            used,
        )
    }

    /// G40: an '@' after the authority (in the path or query, after a port or a bracketed IPv6
    /// host) is not user-info: bead must store and use the URL as given, not a different host,
    /// and not claim it dropped a password.
    #[test]
    fn at_sign_in_path_query_or_port_url_is_not_user_info() {
        let sandbox = crate::sandbox();
        let urls = [
            "https://host.example/org/repo@v2.git",
            "ssh://host.example:2222/srv/a@b.git",
            "https://[::1]:8443/a@b",
            "file:///tmp/a:b@c/r.git",
            "https://host.example/r?x=a@b",
        ];
        for (number, url) in urls.into_iter().enumerate() {
            let machine = Machine::new(&sandbox, &format!("at{number}"));
            let (said, stored, used) = init(&machine, url);
            assert_eq!(
                stored,
                Some(Json::from(url)),
                "{url}: meta.json holds a different remote"
            );
            assert_eq!(
                used,
                Json::from(url),
                "{url}: bead would sync with a different remote"
            );
            assert!(!said.contains("password was dropped"), "{url}: {said}");
        }
    }

    /// G40: in https://u:p@h/r@x the user-info ends at the first '/': git sends u:p to host h for
    /// path /r@x, so bead must drop the password and keep host h and path /r@x.
    #[test]
    fn password_is_dropped_but_host_and_path_are_kept() {
        let sandbox = crate::sandbox();
        let machine = Machine::new(&sandbox, "both");
        let (said, stored, used) = init(&machine, "https://u:p4ss@h.example/r@x");
        for value in [stored, Some(used)] {
            let text = value.as_ref().and_then(Json::as_str);
            let text = text.unwrap_or_else(|| panic!("the remote is {value:?}"));
            assert!(!text.contains("p4ss"), "{text}");
            assert!(
                text.starts_with("https://") && text.ends_with("h.example/r@x"),
                "the host became {text:?}"
            );
        }
        assert!(said.contains("password was dropped"), "{said}");
    }

    /// G40: a file:// origin whose path holds ':' and '@' is the repository git fetches; bead must
    /// sync with it rather than with a path it made up by reading 'a:b' as user:password.
    #[test]
    fn origin_with_at_sign_in_its_path_syncs() {
        let sandbox = crate::sandbox();
        let tracker = sandbox.bare("a:b@c-tracker.git");
        let machine = Machine::new(&sandbox, "path");
        let url = format!("file://{}", tracker.to_str().expect("a UTF-8 path"));
        git(
            ["remote", "set-url", "origin", url.as_str()],
            Some(machine.checkout()),
        );
        // git itself reaches it
        git(["ls-remote", "origin"], Some(machine.checkout()));
        assert_eq!(machine.run(["init", "--prefix", "ex"]).returncode(), 0);
        machine.bead(["create", "through a path with an at sign"]);
        let info = machine.bead(["info"]).expect("info prints a record");
        assert_eq!(info.get("remote"), Some(&Json::from(url.as_str())));
        let synced = machine.run(["sync", "--json"]);
        assert_eq!(synced.returncode(), 0, "{}", synced.stderr);
        let refs = git(["for-each-ref", "refs/beads"], Some(&tracker));
        assert!(refs.contains("refs/beads/journal"), "{refs}");
    }
}
