//! G17: the first sync of a new tracker and a claim work under a translated git: bead runs its
//! private repository's git in the C locale (`docs/FINDINGS.md`).

use std::process::Command;

use beadroll_test_support::Sandbox;

/// `_g17_translated_locale`: a locale in which this git prints a translated "couldn't find remote
/// ref", or `None`. git answers for itself: a fetch of a ref the sandbox's remote does not have,
/// with no user or system config, under each locale in turn.
fn translated_locale(sandbox: &Sandbox) -> Option<&'static str> {
    ["de_DE.UTF-8", "fr_FR.UTF-8"].into_iter().find(|locale| {
        let inherited =
            std::env::vars_os().filter(|(k, _)| !k.as_encoded_bytes().starts_with(b"GIT_"));
        let probe = Command::new("git")
            .arg("--git-dir")
            .arg(sandbox.origin())
            .arg("fetch")
            .arg(sandbox.origin())
            .arg("refs/beads/none")
            .current_dir(sandbox.root())
            .env_clear()
            .envs(inherited)
            .envs([
                ("LANG", *locale),
                ("LC_ALL", *locale),
                ("LANGUAGE", ""),
                ("GIT_CONFIG_NOSYSTEM", "1"),
                ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ])
            .output()
            .expect("git starts");
        let stderr = String::from_utf8_lossy(&probe.stderr);
        !probe.status.success() && !stderr.trim().is_empty() && !stderr.contains("find remote ref")
    })
}

mod g17_translated_git {
    use beadroll_test_support::{Json, Machine, parse_json};

    use super::translated_locale;

    /// G17: Link.fetch recognises a not-yet-published journal by git's English "couldn't find
    /// remote ref"; under a translated git (LANG/LC_ALL=de_DE.UTF-8 or fr_FR.UTF-8) the first sync
    /// of every new tracker fails with REMOTE_UNREACHABLE, and so does every claim, until some
    /// other machine has published. Skipped, as the Python test is, where this git has no German
    /// or French messages.
    #[test]
    fn first_sync_and_claim_work_under_a_translated_git() {
        let sandbox = crate::sandbox();
        let Some(locale) = translated_locale(&sandbox) else {
            eprintln!("skipped: this git has no German or French messages");
            return;
        };
        let mut a = Machine::new(&sandbox, "a");
        for (name, value) in [("LANG", locale), ("LC_ALL", locale), ("LANGUAGE", "")] {
            a.extra.insert(name.into(), value.into());
        }
        // kept local, so the sync below is the push under this locale
        a.bead(["init", "--prefix", "lo", "--offline"]);
        let created = a.bead(["create", "x", "--offline"]).expect("the new issue");
        let key = created["id"].as_str().expect("the issue's id").to_owned();
        let first = a.run(["sync", "--json"]);
        assert_eq!(
            first.returncode(),
            0,
            "first sync under {locale}: {}",
            first.stderr
        );
        let synced = parse_json(&first.stdout).expect("sync prints JSON");
        let pushed = synced.get("pushed").and_then(Json::as_i64);
        let pushed = pushed.unwrap_or_else(|| panic!("no pushed count in {}", first.stdout));
        assert!(pushed > 0, "{}", first.stdout);
        let claim = a.run(["claim", &key, "--json"]);
        assert_eq!(
            claim.returncode(),
            0,
            "claim under {locale}: {}",
            claim.stderr
        );
    }
}
