//! `DeepTreeReview4`, from the fourth independent review: `dep tree` over a deep valid chain.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::process::Command;
use std::sync::Arc;

use beadroll_core::json::Object;
use beadroll_store::{Settings, Workspace, append_events};
use beadroll_test_support::{Json, Run, json, parse_json};

/// One machine's journal as the test writes it: each event caused by the one before.
#[derive(Default)]
struct Chain {
    events: Vec<Json>,
    previous: Option<String>,
    serial: u128,
}

impl Chain {
    /// The next event id: a UUIDv7 whose time and random bits both count the events, spelt as
    /// `str(uuid.UUID(int=value))` spells it.
    fn next_id(&mut self) -> String {
        self.serial += 1;
        let value = ((1_790_000_000_000 + self.serial) << 80) | (7 << 76) | (2 << 62) | self.serial;
        let mut id = format!("{value:032x}");
        for at in [20, 16, 12, 8] {
            id.insert(at, '-');
        }
        id
    }

    /// Appends an event doing `op` to `issue` (to itself when `None`) with payload `data`; its id.
    fn emit(&mut self, op: &str, issue: Option<&str>, data: Json) -> String {
        let event_id = self.next_id();
        let deps: Vec<Json> = self
            .previous
            .iter()
            .map(|d| Json::from(d.as_str()))
            .collect();
        let issue = issue.unwrap_or(&event_id).to_owned();
        self.events.push(json!({
            "v": 1, "id": (event_id.as_str()), "replica": "tree", "deps": deps,
            "actor": {"agent": "test", "thread": "tree-thread"},
            "op": op, "issue": issue, "data": data
        }));
        self.previous = Some(event_id.clone());
        event_id
    }
}

/// Q08: dep tree over a valid 1,100-node chain must not recurse into a crash.
///
/// As in Python, the journal is written in-process, by the store's `append_events`, and bead runs
/// on the store `BEAD_WORKSPACE` names in a `BEAD_HOME` of the test's own. It runs in the sandbox,
/// outside any repository, with the environment a machine's runs have (the developer's harness,
/// store and repository variables left out), so that nothing of the checkout the tests run from
/// reaches it.
#[test]
fn q08_deep_valid_dependency_tree_does_not_overflow() {
    let sandbox = crate::sandbox();
    let mut env: BTreeMap<OsString, OsString> = std::env::vars_os()
        .filter(|(key, _)| {
            let key = key.as_encoded_bytes();
            !["CLAUDE", "CODEX", "BEAD_", "GIT_"]
                .iter()
                .any(|prefix| key.starts_with(prefix.as_bytes()))
        })
        .collect();
    for (key, value) in [
        ("BEAD_HOME", sandbox.root().join("home").into_os_string()),
        ("BEAD_WORKSPACE", "review4-deep-tree".into()),
        ("BEAD_AGENT", "test".into()),
        ("BEAD_THREAD", "tree-thread".into()),
    ] {
        env.insert(key.into(), value);
    }
    let settings = Settings::from_process(
        env.clone(),
        Some(sandbox.root().to_path_buf()),
        std::env::var_os("HOME").unwrap_or_default(),
        std::env::temp_dir(),
        &|_| None,
    )
    .unwrap();
    let id = "review4-deep-tree";
    let workspace = Workspace::new(Arc::new(settings), id, None, None, Object::new()).unwrap();
    let mut chain = Chain::default();
    chain.emit("init", None, json!({"prefix": "ex"}));
    let issue_ids: Vec<String> = (0..1100)
        .map(|index: u32| {
            let key = format!("ex-{index}");
            let fields = json!({"key": key, "fields": {"title": (index.to_string())}});
            chain.emit("create", None, fields)
        })
        .collect();
    for pair in issue_ids.windows(2) {
        let data = json!({"to": (pair[1].as_str()), "type": "blocks"});
        chain.emit("dep_add", Some(&pair[0]), data);
    }
    append_events(&workspace, "tree", &chain.events).unwrap();

    let output = Command::new(sandbox.bead())
        .args(["dep", "tree", "ex-0", "--json"])
        .current_dir(sandbox.root())
        .env_clear()
        .envs(&env)
        .output()
        .unwrap();
    let result = Run {
        status: output.status,
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    };
    assert_eq!(result.returncode(), 0, "{}", result.stderr);
    let tree = parse_json(&result.stdout).unwrap();
    assert_eq!(tree["tree"].as_array().unwrap().len(), 1100);
}
