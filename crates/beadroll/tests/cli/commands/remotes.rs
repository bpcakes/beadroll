//! F03, E02 and a verified property: a password or token in a remote's URL is neither stored nor
//! shown, nor left readable by others, whatever the URL's shape, and the shapes that were already
//! safe stay as they were.

use std::os::unix::fs::PermissionsExt as _;

use beadroll_test_support::{Machine, git, parse_json};

use super::fixtures::metas;

/// The contents of the first store's `meta.json`.
fn meta(machine: &Machine) -> String {
    let first = metas(machine).into_iter().next();
    std::fs::read_to_string(first.expect("a store with a meta.json")).unwrap()
}

/// F03: uppercase schemes, slash passwords and token userinfo are not stored or shown.
#[test]
fn credential_urls_are_not_stored() {
    let sandbox = crate::sandbox();
    let cases = [
        (
            "uppercase",
            "HTTPS://alice:supersecret@example.invalid/private.git",
        ),
        ("slash", "https://alice:sec/ret@example.invalid/private.git"),
        ("token", "https://supersecret@example.invalid/private.git"),
    ];
    for (name, url) in cases {
        let machine = Machine::new(&sandbox, name);
        let made = machine.run([
            "init",
            "--prefix",
            "ex",
            "--tracked",
            "--remote",
            url,
            "--json",
        ]);
        assert_eq!(made.returncode(), 0, "{name}: {}", made.stderr);
        let config = std::fs::read_to_string(machine.checkout().join(".beadroll.json")).unwrap();
        // an `info` that failed, or a store without its meta.json, would show no secret either
        let info = machine.run(["info", "--json"]);
        assert_eq!(info.returncode(), 0, "{name}: {}", info.stderr);
        let metas: Vec<String> = metas(&machine)
            .iter()
            .map(|path| std::fs::read_to_string(path).unwrap())
            .collect();
        assert!(!metas.is_empty(), "{name}: no store's meta.json to look in");
        let blob = format!("{config}{}{}{}", info.stdout, info.stderr, metas.join("\n"));
        for secret in ["supersecret", "sec/ret", "s%65cret"] {
            assert!(!blob.contains(secret), "{name}: {secret} in {blob}");
        }
    }
}

/// Verified (no finding): encoded passwords, ssh:// and scp-like URLs, and a named remote.
#[test]
fn remote_url_shapes_that_are_already_safe() {
    let sandbox = crate::sandbox();
    let encoded = Machine::new(&sandbox, "encoded");
    let made = encoded.run([
        "init",
        "--prefix",
        "ex",
        "--remote",
        "https://alice:s%65cret@example.invalid/private.git",
        "--json",
    ]);
    assert_eq!(made.returncode(), 0, "{}", made.stderr);
    let stored = meta(&encoded);
    assert!(!stored.contains("s%65cret"), "{stored}");
    // G06: over https the user part may be the token
    assert!(
        stored.contains("\"https://example.invalid/private.git\""),
        "{stored}"
    );

    let ssh = Machine::new(&sandbox, "ssh");
    let url = "ssh://alice:supersecret@example.invalid/private.git";
    let made = ssh.run(["init", "--prefix", "ex", "--remote", url]);
    assert_eq!(made.returncode(), 0, "{}", made.stderr);
    let stored = meta(&ssh);
    assert!(!stored.contains("supersecret"), "{stored}");
    // the password dropped, the user kept: what an ssh URL needs to reach the same host
    assert!(
        stored.contains("\"ssh://alice@example.invalid/private.git\""),
        "{stored}"
    );
    let plain = Machine::new(&sandbox, "ssh-user");
    let url = "ssh://alice@example.invalid/private.git";
    plain.run(["init", "--prefix", "ex", "--remote", url]);
    let stored = meta(&plain);
    assert!(stored.contains(url), "{stored}");
    let scp = Machine::new(&sandbox, "scp");
    let url = "git@example.invalid:org/private.git";
    scp.run(["init", "--prefix", "ex", "--remote", url]);
    let stored = meta(&scp);
    assert!(stored.contains(url), "{stored}");

    let named = Machine::new(&sandbox, "named");
    named.bead(["init", "--prefix", "ex"]);
    git(
        [
            "remote",
            "set-url",
            "origin",
            "https://alice:supersecret@example.invalid/private.git",
        ],
        Some(named.checkout()),
    );
    std::fs::write(
        named.checkout().join(".beadroll.json"),
        "{\"workspace\": \"named-ws\", \"remote\": \"origin\"}\n",
    )
    .unwrap();
    let info = named.run(["info", "--json"]);
    assert_eq!(info.returncode(), 0, "{}", info.stderr);
    assert!(!info.stdout.contains("supersecret"), "{}", info.stdout);
    assert_eq!(
        parse_json(&info.stdout).unwrap()["remote"].as_str(),
        // G06: over https the user part may be the token
        Some("https://example.invalid/private.git")
    );
}

/// E02: remote URL credentials must be redacted and protected at rest.
#[test]
fn remote_credentials_are_not_disclosed_or_world_readable() {
    let sandbox = crate::sandbox();
    let secret = "supersecret";
    let remote = format!("https://alice:{secret}@example.invalid/private.git");
    let machine = Machine::new(&sandbox, "e02-secret");
    let initialized = machine.run(["init", "--prefix", "ex", "--remote", &remote, "--json"]);
    assert_eq!(initialized.returncode(), 0, "{}", initialized.stderr);
    let info = machine.run(["info", "--json"]);
    assert_eq!(info.returncode(), 0, "{}", info.stderr);
    assert!(!info.stdout.contains(secret), "{}", info.stdout);
    let meta = machine.store().join("meta.json");
    let text = std::fs::read_to_string(&meta).unwrap();
    assert!(!text.contains(secret), "{text}");
    // `stat.S_IMODE(mode) & 0o077`: no permission for the group or for others
    let mode = std::fs::metadata(&meta).unwrap().permissions().mode();
    assert_eq!(mode & 0o7777 & 0o077, 0, "meta.json has mode {mode:o}");
}
