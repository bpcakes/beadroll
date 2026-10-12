//! G39: the password of a remote in git's `<transport>::<address>` form (`https::https://user:pw@
//! host/...`) is dropped exactly as the plain URL's is: never stored, shown, committed or handed to
//! git.

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// The `Authorization` headers a recording server has seen, one per request, `None` for a request
/// that carried none.
type Seen = Arc<Mutex<Vec<Option<String>>>>;

/// A local HTTP/1.0 server that answers GET with 401 until credentials arrive, then 404. Returns
/// its port and recorded Authorization headers. Each response closes the connection; other
/// methods return 501 without being recorded.
fn recording_server() -> (u16, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Seen::default();
    let record = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Ok(reader) = stream.try_clone() else {
                continue;
            };
            let mut reader = BufReader::new(reader);
            let mut request = String::new();
            if reader.read_line(&mut request).unwrap_or(0) == 0 {
                continue;
            }
            let mut auth = None;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("authorization")
                {
                    auth = Some(value.trim().to_owned());
                }
            }
            let answer: &[u8] = if !request.starts_with("GET ") {
                b"HTTP/1.0 501 Unsupported method\r\nContent-Length: 0\r\n\r\n"
            } else if auth.is_some() {
                record.lock().unwrap().push(auth);
                b"HTTP/1.0 404 Not Found\r\nContent-Length: 0\r\n\r\n"
            } else {
                record.lock().unwrap().push(auth);
                b"HTTP/1.0 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"x\"\r\n\
                  Content-Length: 0\r\n\r\n"
            };
            let _ = stream.write_all(answer);
        }
    });
    (port, seen)
}

mod g39_password_in_a_transport_url {
    use std::path::PathBuf;

    use beadroll_test_support::{Json, Machine, Sandbox, parse_json};

    use super::recording_server;

    /// `PLAIN`: an https remote with a user and a password.
    const PLAIN: &str = "https://alice:supersecret@example.invalid/org/repo.git";

    /// `HELPER`: `PLAIN` in git's `<transport>::<address>` form, where git-remote-https gets the
    /// address as is.
    fn helper() -> String {
        format!("https::{PLAIN}")
    }

    /// The class's `setUp`: the sandbox, and an empty global git config, so that no credential
    /// helper or askpass comes from the developer's own config.
    struct Fixture {
        sandbox: Sandbox,
        gitconfig: PathBuf,
        count: usize,
    }

    impl Fixture {
        fn new() -> Fixture {
            let sandbox = crate::sandbox();
            let gitconfig = sandbox.root().join("empty-gitconfig");
            std::fs::write(&gitconfig, "").unwrap();
            Fixture {
                sandbox,
                gitconfig,
                count: 0,
            }
        }

        /// `initialised`: a new machine after `init --remote URL`, and what init printed.
        fn initialised(&mut self, url: &str, more: &[&str]) -> (Machine, String) {
            self.count += 1;
            let mut machine = Machine::new(&self.sandbox, &format!("m{}", self.count));
            machine
                .extra
                .insert("GIT_CONFIG_NOSYSTEM".into(), "1".into());
            machine
                .extra
                .insert("GIT_CONFIG_GLOBAL".into(), self.gitconfig.clone().into());
            machine.extra.insert("GIT_ASKPASS".into(), "".into());
            let args = ["init", "--prefix", "ex", "--remote", url];
            let made = machine.run(args.iter().chain(more));
            assert_eq!(made.returncode(), 0, "{}", made.stderr);
            let said = made.stdout + &made.stderr;
            (machine, said)
        }
    }

    /// The `remote` member of `document`, which must be text.
    fn remote(document: &Json) -> String {
        let remote = document["remote"].as_str();
        remote.expect("a remote, as text").to_owned()
    }

    /// The first `meta.json` in `machine`'s home: `next(machine.home.glob("*/meta.json"))`.
    fn meta(machine: &Machine) -> Json {
        let entries = std::fs::read_dir(machine.home()).unwrap();
        let path = entries
            .map(|entry| entry.unwrap().path().join("meta.json"))
            .find(|path| path.exists())
            .expect("a store's meta.json");
        parse_json(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    /// G39: the password of `https::https://user:pw@host/...` must be dropped exactly as for
    /// `https://user:pw@host/...`: not kept in meta.json, not shown by info, with the `https::`
    /// prefix kept.
    #[test]
    fn password_in_transport_url_is_not_stored_or_shown() {
        let mut fixture = Fixture::new();
        let mut stored = Vec::new();
        let mut shown = Vec::new();
        let mut said = Vec::new();
        for url in [helper(), PLAIN.to_owned()] {
            let (machine, printed) = fixture.initialised(&url, &[]);
            said.push(printed);
            stored.push(remote(&meta(&machine)));
            let info = machine.run(["info", "--json"]);
            assert_eq!(info.returncode(), 0, "{}", info.stderr);
            shown.push(remote(&parse_json(&info.stdout).unwrap()));
        }
        let [stored_helper, stored_plain] = &stored[..] else {
            panic!("two remotes stored")
        };
        let [shown_helper, shown_plain] = &shown[..] else {
            panic!("two remotes shown")
        };
        assert!(
            !format!("{stored_helper}{shown_helper}").contains("supersecret"),
            "the password was kept: {stored_helper}, {shown_helper}"
        );
        assert_eq!(*stored_helper, format!("https::{stored_plain}"));
        assert_eq!(*shown_helper, format!("https::{shown_plain}"));
        let dropped: Vec<bool> = said
            .iter()
            .map(|printed| printed.contains("password was dropped"))
            .collect();
        assert_eq!(dropped[0], dropped[1], "{said:?}");
    }

    /// G39: `init --tracked --remote https::https://user:pw@host/...` must not write the password
    /// into .beadroll.json, the file bead tells the user to commit.
    #[test]
    fn password_in_transport_url_is_not_committed() {
        let mut fixture = Fixture::new();
        let mut configs = Vec::new();
        for url in [helper(), PLAIN.to_owned()] {
            let (machine, _) = fixture.initialised(&url, &["--tracked", "--json"]);
            let config = machine.checkout().join(".beadroll.json");
            let config = parse_json(&std::fs::read_to_string(config).unwrap()).unwrap();
            configs.push(remote(&config));
        }
        let [with_helper, plain] = &configs[..] else {
            panic!("two configs")
        };
        assert!(
            !with_helper.contains("supersecret"),
            "the password is in .beadroll.json: {with_helper}"
        );
        assert_eq!(*with_helper, format!("https::{plain}"));
    }

    /// G39: bead must not hand git the password of `http::http://user:pw@host/...` (git sends it
    /// as Basic auth), nor print it when the sync fails.
    #[test]
    fn password_in_transport_url_is_not_handed_to_git() {
        let mut fixture = Fixture::new();
        let (port, seen) = recording_server();
        let url = format!("http::http://alice:secret-g39@127.0.0.1:{port}/org/repo.git");
        let (machine, _) = fixture.initialised(&url, &["--json"]);
        let synced = machine.run(["sync"]);
        // the server never accepts; only what it was sent matters
        assert_ne!(synced.returncode(), 0, "{}", synced.stdout);
        let seen = seen.lock().unwrap().clone();
        assert!(!seen.is_empty(), "git never reached the recording server");
        let sent: Vec<&String> = seen.iter().flatten().collect();
        assert!(
            sent.is_empty(),
            "git was handed the password and sent it to the server: {sent:?}"
        );
        let printed = synced.stdout + &synced.stderr;
        assert!(!printed.contains("secret-g39"), "{printed}");
    }
}
