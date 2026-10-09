//! V7: `without_password` drops exactly the secrets git would send.

use std::ffi::OsStr;
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use beadroll_git::without_password;

fn wp(url: &str) -> (String, bool) {
    let (out, dropped) = without_password(OsStr::new(url));
    (out.into_string().unwrap(), dropped)
}

/// git with no user, system or global config, no proxies and no prompts: it sees only `root`.
fn git_env(root: &Path, more: &[(&str, &str)]) -> Vec<(String, String)> {
    let home = root.join("v7-home");
    std::fs::create_dir_all(&home).unwrap();
    let config = home.join("gitconfig");
    std::fs::write(&config, "").unwrap();
    let mut env: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| {
            !k.starts_with("GIT_")
                && ![
                    "http_proxy",
                    "https_proxy",
                    "all_proxy",
                    "no_proxy",
                    "ssh_askpass",
                ]
                .contains(&k.to_lowercase().as_str())
        })
        .collect();
    let home = home.display().to_string();
    let config = config.display().to_string();
    for (k, v) in [
        ("HOME", home.as_str()),
        ("XDG_CONFIG_HOME", home.as_str()),
        ("GIT_CONFIG_GLOBAL", config.as_str()),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("NO_PROXY", "*"),
        ("no_proxy", "*"),
    ]
    .iter()
    .chain(more)
    {
        env.retain(|(name, _)| name != k);
        env.push(((*k).to_owned(), (*v).to_owned()));
    }
    env
}

fn ls_remote(root: &Path, url: &str, more: &[(&str, &str)]) -> Output {
    Command::new("git")
        .args(["ls-remote", url])
        .current_dir(root)
        .env_clear()
        .envs(git_env(root, more))
        .output()
        .unwrap()
}

fn scratch() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("v7-urls-")
        .tempdir()
        .unwrap();
    let root = dir.path().canonicalize().unwrap();
    (dir, root)
}

/// The user:password a Basic Authorization header carries ('' when it carries none).
fn basic(header: &str) -> String {
    let Some(encoded) = header.strip_prefix("Basic ") else {
        return String::new();
    };
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bits: String = encoded
        .trim()
        .trim_end_matches('=')
        .bytes()
        .map(|c| {
            let at = alphabet.iter().position(|a| *a == c).unwrap();
            format!("{at:06b}")
        })
        .collect();
    let bytes: Vec<u8> = (0..bits.len().saturating_sub(7))
        .step_by(8)
        .map(|i| u8::from_str_radix(&bits[i..i + 8], 2).unwrap())
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// An HTTP server on 127.0.0.1 that answers every request 401 (Basic) and records its
/// Authorization header: its port and the headers seen.
fn auth_server() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut authorization = String::new();
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
                    authorization = value.trim().to_owned();
                }
            }
            record.lock().unwrap().push(authorization);
            let _ = stream.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"v7\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });
    (port, seen)
}

/// without_password drops the password of every shape git sends one for and leaves ssh users and
/// ports, IPv6 hosts, scp with a user, `ssh://git%40corp@host` and '@' inside local paths unchanged.
#[test]
fn git_confirmed_password_shapes_are_dropped() {
    let dropped = [
        (
            "https://alice:secret@host.example/org/repo.git",
            "https://host.example/org/repo.git",
        ),
        (
            "https://alice:p%40ss@host.example/org/repo.git",
            "https://host.example/org/repo.git",
        ),
        (
            "HTTPS://alice:secret@host.example/org/repo.git",
            "HTTPS://host.example/org/repo.git",
        ),
        (
            "https://:TOKEN@host.example/org/repo.git",
            "https://host.example/org/repo.git",
        ),
        (
            "https://TOKEN@host.example/org/repo.git",
            "https://host.example/org/repo.git",
        ),
        (
            "https://alice:secret@[::1]:8443/org/repo.git",
            "https://[::1]:8443/org/repo.git",
        ),
        (
            "ssh://git:secret@host.example/org/repo.git",
            "ssh://git@host.example/org/repo.git",
        ),
        (
            "persistent-https://alice:secret@host.example/org/repo",
            "persistent-https://alice@host.example/org/repo",
        ),
    ];
    for (url, expected) in dropped {
        assert_eq!(wp(url), (expected.into(), true), "{url}");
    }
    let unchanged = [
        "ssh://git@host.example:2222/org/repo.git",
        "ssh://git@[::1]:2222/org/repo.git",
        "git@host.example:org/repo.git",
        "ssh://git%40corp@host.example/org/repo.git",
        "https://[::1]:8443/org/repo.git",
        "/srv/git/a@b/repo.git",
        "file:///srv/git/a@b/repo.git",
    ];
    for url in unchanged {
        assert_eq!(wp(url), (url.into(), false), "{url}");
    }
    assert_eq!((dropped.len(), unchanged.len()), (8, 7));
}

/// Against a local 401 server recording Authorization, git sends the secret of each shape; with
/// the URL without_password returns, git sends no secret (and no user either over http).
#[test]
fn http_secrets_git_sends_are_the_ones_bead_drops() {
    let (_dir, root) = scratch();
    let (port, seen) = auth_server();
    let shapes = [
        ("http://alice:secret@{hp}/org/repo.git", "secret"),
        ("http://alice:p%40ss@{hp}/org/repo.git", "p@ss"),
        ("HTTP://alice:secret@{hp}/org/repo.git", "secret"),
        ("http://:TOKEN@{hp}/org/repo.git", "TOKEN"),
        ("http://TOKEN@{hp}/org/repo.git", "TOKEN"),
    ];
    for (template, secret) in shapes {
        let url = template.replace("{hp}", &format!("127.0.0.1:{port}"));
        seen.lock().unwrap().clear();
        ls_remote(&root, &url, &[]);
        let sent: Vec<String> = seen.lock().unwrap().iter().map(|h| basic(h)).collect();
        if sent.is_empty() {
            continue; // this git never contacted the server (git 2.43 rejects an upper-case scheme)
        }
        assert!(
            sent.iter().any(|auth| auth.contains(secret)),
            "git did not send {secret:?} from {url}: {sent:?}"
        );
        let (rewritten, was_dropped) = wp(&url);
        assert!(was_dropped);
        seen.lock().unwrap().clear();
        ls_remote(&root, &rewritten, &[]);
        let sent: Vec<String> = seen.lock().unwrap().iter().map(|h| basic(h)).collect();
        assert!(
            !seen.lock().unwrap().is_empty(),
            "git did not reach the server with bead's {rewritten}"
        );
        assert!(
            !sent.iter().any(|auth| auth.contains(secret)),
            "git still sends {secret:?} from {rewritten}"
        );
    }
}

/// Through a logging GIT_SSH_COMMAND, git hands ssh the logins bead keeps, and the password bead
/// drops never reaches ssh once dropped.
#[test]
fn ssh_logins_bead_keeps_are_what_git_hands_ssh() {
    let (_dir, root) = scratch();
    let log = root.join("v7-ssh.log");
    let script = root.join("v7-ssh-log");
    std::fs::write(
        &script,
        "#!/bin/sh\nfor a in \"$@\"; do printf \"%s\\n\" \"$a\" >> \"$V7_SSH_LOG\"; done\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let script = format!("'{}'", script.display());
    let log_path = log.display().to_string();
    let handed = |url: &str| -> Vec<String> {
        std::fs::write(&log, "").unwrap();
        ls_remote(
            &root,
            url,
            &[
                ("GIT_SSH_COMMAND", script.as_str()),
                ("V7_SSH_LOG", log_path.as_str()),
            ],
        );
        std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    };
    let args = handed("ssh://git:secret@host.example/org/repo.git");
    assert!(
        args.contains(&"git:secret@host.example".to_owned()),
        "git does not hand ssh the user:password login: {args:?}"
    );
    let rewritten = wp("ssh://git:secret@host.example/org/repo.git");
    assert_eq!(
        rewritten,
        ("ssh://git@host.example/org/repo.git".into(), true)
    );
    let args = handed(&rewritten.0);
    assert!(args.contains(&"git@host.example".to_owned()), "{args:?}");
    assert!(!args.iter().any(|a| a.contains("secret")), "{args:?}");
    let kept: [(&str, &[&str]); 4] = [
        (
            "ssh://git%40corp@host.example/org/repo.git",
            &["git@corp@host.example"],
        ),
        ("alice:secret@host.example:org/repo.git", &["alice"]),
        (
            "ssh://git@host.example:2222/org/repo.git",
            &["-p", "2222", "git@host.example"],
        ),
        ("git@host.example:org/repo.git", &["git@host.example"]),
    ];
    for (url, expected) in kept {
        let args = handed(url);
        for arg in expected {
            assert!(
                args.contains(&(*arg).to_owned()),
                "git handed ssh {args:?} for {url}"
            );
        }
        assert_eq!(wp(url), (url.into(), false), "{url}");
    }
}
