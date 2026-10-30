//! Shared protocol fixtures: in-process stores and actors, coordinated command bodies and
//! pause-until handshakes.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use beadroll_core::json::{Format, Object, Value, write};
use beadroll_store::{Actor, Settings, Workspace, pending_files, read_attempt};
use beadroll_sync::NetworkMode;
use beadroll_sync::{Error, Mode, Tx, coordinated};
use beadroll_test_support::{Json, Machine, Sandbox, built_bead, json};

/// A sandbox whose machines run `BEAD_UNDER_TEST`, else the `bead` Cargo built beside this test
/// binary ([`built_bead`]); a missing one fails the test, saying how to build it.
pub(crate) fn sandbox() -> Sandbox {
    Sandbox::new(built_bead())
}

/// The settings of `machine`'s runs, from its checkout, with `vars` set besides
/// (`BEAD_FAILPOINTS`, `BEAD_NET_TIMEOUT`).
pub(crate) fn settings(machine: &Machine, vars: &[(&str, &str)]) -> Arc<Settings> {
    let mut env: BTreeMap<OsString, OsString> = machine.env(None, "codex");
    for (name, value) in vars {
        env.insert((*name).into(), (*value).into());
    }
    let settings = Settings::from_process(
        env,
        Some(machine.checkout().to_path_buf()),
        std::env::var_os("HOME").unwrap_or_default(),
        std::env::temp_dir(),
        &|_| None,
    );
    Arc::new(settings.expect("the machine's settings"))
}

/// Opens the machine's store in-process with its environment plus vars.
pub(crate) fn workspace(machine: &Machine, origin: &Path, vars: &[(&str, &str)]) -> Workspace {
    let store = machine.store();
    let id = store
        .file_name()
        .and_then(OsStr::to_str)
        .expect("a store id");
    let origin = Some(origin.as_os_str().to_owned());
    Workspace::new(settings(machine, vars), id, None, origin, Object::new()).expect("the store")
}

/// The actor detected on the machine.
pub(crate) fn actor(machine: &Machine) -> Actor {
    let thread = format!("{}-thread-0001", machine.name());
    Actor::writable(json!({"agent": "codex", "thread": thread}))
}

/// `path` quoted for the shell.
pub(crate) fn quoted(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

/// What `text` holds as a string.
pub(crate) fn text(value: &Json) -> String {
    value.as_str().expect("a string").to_owned()
}

/// The ids of the events this machine has pending: its unsettled attempts.
pub(crate) fn pending_ids(ws: &Workspace) -> Vec<String> {
    pending_files(ws)
        .iter()
        .flat_map(|path| read_attempt(path, None))
        .map(|event| text(&event["id"]))
        .collect()
}

/// The texts of an issue's comments, as `show` gives them.
pub(crate) fn comments(issue: &Json) -> Vec<String> {
    let comments = issue["comments"].as_array().unwrap_or_default();
    comments.iter().map(|c| text(&c["text"])).collect()
}

/// The receipt signature for the complete parsed command namespace in asked.
fn signature(actor: &Actor, asked: Json, request: Json) -> String {
    let receipt = json!({"actor": (actor.value.clone()), "args": asked, "request": request});
    write(&receipt, Format::SORTED_ASCII).expect("a receipt")
}

/// Builds a claim for key with generation one greater than the issue's last.
pub(crate) fn claim(tx: &mut Tx<'_>, mode: Mode, key: &str) -> Result<Mode, Error> {
    let (st, mut tx) = tx.lend()?;
    let issue = st
        .resolve(key)
        .ok_or_else(|| Error::Validation(format!("Issue not found: {key}")))?;
    let mut data = Object::new();
    data.insert("mode".into(), Value::from(mode.as_str()));
    let generation = issue
        .claim_generation()
        .plus_one()
        .expect("a generation counts on");
    data.insert("generation".into(), Value::Int(generation));
    tx.emit("claim", Some(&issue.uid().to_string()), data, None)?;
    Ok(mode)
}

/// `bead claim KEY` by `actor`.
pub(crate) fn claim_command(ws: &Workspace, actor: &Actor, key: &str) -> Result<Mode, Error> {
    let asked = json!({"command": "claim", "ids": [key], "offline": false, "remote": null});
    let signature = signature(actor, asked, Json::Null);
    coordinated(
        ws,
        actor,
        NetworkMode::Online,
        None,
        &signature,
        |tx, mode| claim(tx, mode, key),
    )
}

/// A claiming title update, optionally adding a label. The actor's claim and edits land together
/// or not at all.
pub(crate) fn claiming_update(
    ws: &Workspace,
    actor: &Actor,
    key: &str,
    title: &str,
    label: Option<&str>,
) -> Result<Mode, Error> {
    let added = label.map_or(Json::Null, |label| json!([label]));
    let asked = json!({
        "acceptance_criteria": null, "add_label": added, "assignee": null, "claim": true,
        "command": "update", "description": null, "description_file": null, "design": null,
        "estimate": null, "external_ref": null, "force": false, "ids": [key], "notes": null,
        "offline": false, "parent": null, "priority": null, "remote": null, "remove_label": null,
        "set_labels": null, "status": null, "title": title, "type": null
    });
    let request = json!({"fields": {"title": title}, "status": null});
    let signature = signature(actor, asked, request);
    coordinated(
        ws,
        actor,
        NetworkMode::Online,
        None,
        &signature,
        |tx, mode| {
            let mode = claim(tx, mode, key)?;
            // every edit joins the attempt: it lands with the claim or not at all
            tx.conditional = true;
            let uid = tx
                .state()?
                .resolve(key)
                .map(|issue| issue.uid().to_string());
            let uid = uid.unwrap_or_default();
            let mut fields = Object::new();
            fields.insert("title".into(), Value::from(title));
            let mut data = Object::new();
            data.insert("fields".into(), Value::from(fields));
            tx.emit("set", Some(&uid), data, None)?;
            if let Some(label) = label {
                let mut data = Object::new();
                data.insert("labels".into(), json!([label]));
                tx.emit("label_add", Some(&uid), data, None)?;
            }
            Ok(mode)
        },
    )
}

/// `bead release KEY` by `actor`, who holds its claim.
pub(crate) fn release_command(ws: &Workspace, actor: &Actor, key: &str) -> Result<Mode, Error> {
    let asked = json!({
        "command": "release", "ids": [key], "offline": false, "reason": null, "remote": null
    });
    let signature = signature(actor, asked, Json::Null);
    coordinated(
        ws,
        actor,
        NetworkMode::Online,
        None,
        &signature,
        |tx, mode| {
            let st = tx.state()?.clone();
            let issue = st.resolve(key);
            let held = issue.and_then(|issue| {
                let claim = issue.claim()?.id.to_string();
                Some((issue.uid().to_string(), claim))
            });
            let (uid, claim) =
                held.ok_or_else(|| Error::Validation(format!("{key} is not claimed")))?;
            let mut data = Object::new();
            data.insert("claim".into(), Value::from(claim));
            tx.emit("release", Some(&uid), data, None)?;
            Ok(mode)
        },
    )
}

/// The handshake of a `pause-until` failpoint: the paused process touches `PATH.ready`, then waits
/// for `PATH.go`.
pub(crate) struct Gate {
    path: PathBuf,
}

impl Gate {
    /// A gate named `name` in the sandbox.
    pub(crate) fn new(sandbox: &Sandbox, name: &str) -> Gate {
        Gate {
            path: sandbox.root().join(name),
        }
    }

    /// The failpoint action that stops at this gate.
    pub(crate) fn action(&self) -> String {
        format!("pause-until({})", self.path.display())
    }

    fn with(&self, suffix: &str) -> PathBuf {
        let mut name = self.path.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    }

    /// Once the paused process has reached the gate, runs `step` and then lets the process go on,
    /// even if `step` fails; `None` if `done` is set first, the process having finished without
    /// reaching it.
    pub(crate) fn meet<T>(&self, done: &AtomicBool, step: impl FnOnce() -> T) -> Option<T> {
        let ready = self.with(".ready");
        let deadline = Instant::now() + Duration::from_secs(120);
        while !ready.exists() {
            if done.load(Ordering::SeqCst) || Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let _open = Open(self.with(".go"));
        Some(step())
    }
}

/// Touches its file when dropped: a gate opened, or a hook released, whatever the test came to.
pub(crate) struct Open(pub(crate) PathBuf);

impl Drop for Open {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, b"");
    }
}

/// Sets its flag when dropped.
struct Done<'a>(&'a AtomicBool);

impl Drop for Done<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Runs `main` in this thread and `helper` beside it, which meets `main` at its gates and learns
/// from the flag it is given that `main` has finished: what each came to.
pub(crate) fn alongside<M, H: Send>(
    main: impl FnOnce() -> M,
    helper: impl FnOnce(&AtomicBool) -> H + Send,
) -> (M, H) {
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let helper = scope.spawn(|| helper(&done));
        let out = {
            let _done = Done(&done);
            main()
        };
        let met = helper
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        (out, met)
    })
}
