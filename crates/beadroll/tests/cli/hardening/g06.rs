//! G06 (`docs/FINDINGS.md`): `https://TOKEN:x-oauth-basic@host` kept the token in
//! `.beadroll.json` and `meta.json` while claiming the password was dropped.

mod g06_https_token_in_the_user_position {
    use std::net::TcpListener;
    use std::path::PathBuf;

    use beadroll_test_support::{Json, Machine, git, parse_json};

    /// The class's `TOKEN`: a token in the shape of a GitHub one.
    const TOKEN: &str = "ghp_G06TESTTOKEN0123";

    /// The `meta.json` of each store in `machine`'s home, in directory order: what
    /// `home.glob("*/meta.json")` lists.
    fn metas(machine: &Machine) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(machine.home()) else {
            return Vec::new();
        };
        entries
            .map(|entry| entry.unwrap().path().join("meta.json"))
            .filter(|path| path.exists())
            .collect()
    }

    /// `value[key]`, which must be there: Python's `value[key]` raises when it is not.
    fn member<'v>(value: &'v Json, key: &str) -> &'v Json {
        value
            .get(key)
            .unwrap_or_else(|| panic!("no {key:?} in {value:?}"))
    }

    /// G06: https://TOKEN:x-oauth-basic@host and https://TOKEN:@host carry the token as the user
    /// part; bead must not write it to .beadroll.json or meta.json (it says the password was
    /// dropped), and the value it stores must be the value it uses.
    #[test]
    fn token_user_part_is_not_written_to_tracked_config_or_meta() {
        let sandbox = crate::sandbox();
        let shapes = [
            (
                "x-oauth-basic",
                format!("https://{TOKEN}:x-oauth-basic@example.invalid/org/repo.git"),
            ),
            (
                "empty-password",
                format!("https://{TOKEN}:@example.invalid/org/repo.git"),
            ),
        ];
        for (shape, url) in shapes {
            let tracked = Machine::new(&sandbox, &format!("{shape}-tracked"));
            let made = tracked.run([
                "init",
                "--prefix",
                "ex",
                "--tracked",
                "--remote",
                &url,
                "--json",
            ]);
            assert_eq!(made.returncode(), 0, "shape={shape}: {}", made.stderr);
            let config =
                std::fs::read_to_string(tracked.checkout().join(".beadroll.json")).unwrap();
            assert!(
                !config.contains(TOKEN),
                "shape={shape}: the token is in .beadroll.json, which bead says to commit"
            );
            let plain = Machine::new(&sandbox, &format!("{shape}-meta"));
            let made = plain.run(["init", "--prefix", "ex", "--remote", &url, "--json"]);
            assert_eq!(made.returncode(), 0, "shape={shape}: {}", made.stderr);
            let first = metas(&plain)
                .into_iter()
                .next()
                .expect("a store's meta.json");
            let stored = std::fs::read_to_string(&first).unwrap();
            let meta = parse_json(&stored).unwrap_or_else(|error| panic!("{error}: {stored}"));
            let remote = member(&meta, "remote").as_str().expect("a remote");
            assert!(
                !remote.contains(TOKEN),
                "shape={shape}: the token is stored in meta.json"
            );
            let info = plain.run(["info", "--json"]);
            assert_eq!(info.returncode(), 0, "shape={shape}: {}", info.stderr);
            assert!(
                !info.stdout.contains(TOKEN),
                "shape={shape}: {}",
                info.stdout
            );
            let reported = parse_json(&info.stdout).unwrap();
            assert_eq!(
                member(&meta, "remote"),
                member(&reported, "remote"),
                "shape={shape}: bead stores one remote and uses another"
            );
        }
    }

    /// G06: a checkout whose origin is http://TOKEN:x-oauth-basic@host must not have bead print
    /// the token in info or in a sync error (git itself redacts it), nor store it in meta.json.
    #[test]
    fn token_user_part_of_origin_is_not_shown() {
        let port = {
            let probe = TcpListener::bind("127.0.0.1:0").unwrap();
            probe.local_addr().unwrap().port()
        }; // closed again: connections are refused, nothing leaves the host
        let sandbox = crate::sandbox();
        let machine = Machine::new(&sandbox, "origin-token");
        let url = format!("http://{TOKEN}:x-oauth-basic@127.0.0.1:{port}/org/repo.git");
        git(
            ["remote", "set-url", "origin", &url],
            Some(machine.checkout()),
        );
        let made = machine.run(["init", "--prefix", "ex", "--json"]);
        assert_eq!(made.returncode(), 0, "{}", made.stderr);
        let info = machine.run(["info", "--json"]);
        assert_eq!(info.returncode(), 0, "{}", info.stderr);
        let synced = machine
            .cmd(["sync", "--json"])
            .env("GIT_TERMINAL_PROMPT", "0")
            .run();
        // nothing answers; only what bead prints matters
        assert_ne!(synced.returncode(), 0, "{}", synced.stdout);
        let metas: String = metas(&machine)
            .iter()
            .map(|path| std::fs::read_to_string(path).unwrap())
            .collect();
        let shown = format!("{}{}{}{metas}", info.stdout, synced.stdout, synced.stderr);
        assert!(!shown.contains(TOKEN), "{shown}");
    }
}
