//! The judge (Python's `Run.judge`): a command's answer must match what the remote holds right
//! after it. A refusal may also rest on the acting machine's own unpublished writes, which it
//! legitimately sees; a follow-up run beside another may rest on the remote before both.

use beadroll_test_support::Json;

use super::UNCERTAIN;
use super::call::{Rc, Step};
use super::run::{Earlier, OBS, Remote, Run};

/// The refusals a command may answer with, when the remote agrees.
const REFUSALS: [&str; 4] = [
    "NOT_OWNER",
    "ALREADY_CLAIMED",
    "CLAIMED_BY_OTHER",
    "VALIDATION",
];

/// What the judge knows of a step: the owner label of whoever ran it, the issue it named, that
/// issue on the remote and as the acting machine sees it.
struct Facts<'a> {
    step: &'a Step,
    me: String,
    /// `X` or `Z`, when the command names one of them.
    key: Option<char>,
    /// The issue as the remote holds it.
    issue: Option<&'a Json>,
    /// The remote's confirmed claim of it.
    held: Option<&'a Json>,
    /// The remote's status of it.
    status: Option<&'a str>,
    /// The issue as the acting machine sees it, with what the remote held before a concurrent
    /// follow-up (Python's `local`).
    seen: Json,
}

impl Facts<'_> {
    /// Whether the command line holds `word`.
    fn has(&self, word: &str) -> bool {
        self.step.argv.iter().any(|arg| arg == word)
    }

    /// The owner of the remote's claim, as Python prints `held and held['owner']`.
    fn holder(&self) -> String {
        self.held
            .map_or_else(|| "None".to_owned(), |held| text(&held["owner"]))
    }

    /// Whether the remote's claim is held by someone other than the one who ran the step.
    fn held_by_other(&self) -> bool {
        self.held
            .is_some_and(|held| held["owner"].as_str() != Some(&self.me))
    }
}

/// A member as Python's f-string prints it: the text of a string, `None` for nothing.
fn text(value: &Json) -> String {
    match value {
        Json::Null => "None".to_owned(),
        Json::Str(text) => text.clone(),
        other => super::run::shown(other),
    }
}

/// Whether a status says the issue is done: closed, or deleted.
fn done(status: Option<&str>) -> bool {
    matches!(status, Some("closed" | "tombstone"))
}

impl Run {
    /// Judges `step`, run by `who` (machine, thread), against `remote`, what the observer read
    /// right after it. Exits 98, 99 and -9 (the fault itself) and the uncertain codes are excused,
    /// and nothing is judged when the remote could not be read. `earlier`: the step ran beside
    /// another, and its answer may rest on the remote before both.
    pub(super) fn judge(
        &self,
        step: &Step,
        remote: &Remote,
        who: (&str, &str),
        earlier: Option<&Earlier>,
    ) {
        let uncertain = step
            .code
            .as_deref()
            .is_some_and(|code| UNCERTAIN.contains(&code));
        if matches!(step.rc, Rc::Exit(98 | 99 | -9)) || uncertain || remote.is_empty() {
            return;
        }
        let facts = self.facts(step, remote, who, earlier);
        let verb = step.argv[0].as_str();
        let claims =
            matches!(verb, "claim" | "takeover") || (verb == "update" && facts.has("--claim"));
        let (succeeded, code) = (step.rc.is(0), step.code.as_deref());
        if claims && succeeded {
            self.judge_claimed(&facts);
        } else if claims && code == Some("ALREADY_CLAIMED") {
            let mine = facts.held.is_none() || !facts.held_by_other();
            let local = &facts.seen["claim"];
            let local_other = local.truthy() && local["owner"].as_str() != Some(&facts.me);
            if mine && !local_other {
                let why = format!(
                    "refused as already claimed but the remote owner is {}",
                    facts.holder()
                );
                self.bad(&facts, &why);
            }
        } else if claims && code == Some("VALIDATION") {
            if !done(facts.status) && !done(facts.seen["status"].as_str()) {
                let status = facts.status.unwrap_or("None");
                self.bad(
                    &facts,
                    &format!("refused as done but the remote status is {status}"),
                );
            }
        } else if verb == "release" && (succeeded || code == Some("NOT_OWNER")) {
            if facts.held.is_some() && !facts.held_by_other() {
                self.bad(&facts, "the remote still has this actor as owner");
            }
        } else if matches!(verb, "close" | "delete") && succeeded {
            self.judge_ended(&facts, verb);
        } else if verb == "sync" && succeeded {
            self.judge_synced(&facts, earlier);
        } else if !succeeded && !code.is_some_and(|code| REFUSALS.contains(&code)) {
            self.bad(&facts, "unexpected failure");
        }
    }

    /// What the judge knows of `step`.
    fn facts<'a>(
        &self,
        step: &'a Step,
        remote: &'a Remote,
        who: (&str, &str),
        earlier: Option<&Earlier>,
    ) -> Facts<'a> {
        let me = self.label(who.0, who.1);
        let names = |letter| step.argv.iter().any(|arg| arg == self.keys.of(letter));
        let key = ['X', 'Z'].into_iter().find(|letter| names(*letter));
        let mut seen = Json::Object(Default::default());
        if let Some(key) = key {
            let machine = self.machine(&step.machine);
            let shown = ["show", self.keys.of(key)];
            seen = self
                .call(machine, &shown, &step.thread, None, "view of actor")
                .first();
        }
        let issue = key.and_then(|key| remote.get(&key));
        let held = issue
            .map(|issue| &issue["claim"])
            .filter(|claim| claim.truthy());
        let status = issue.and_then(|issue| issue["status"].as_str());
        let mut facts = Facts {
            step,
            me,
            key,
            issue,
            held,
            status,
            seen,
        };
        if let Some(earlier) = earlier {
            facts.seen = before_both(&facts, earlier);
        }
        facts
    }

    /// A claim (or a takeover, or an update with `--claim`) reported done: the remote must hold
    /// this actor's claim, or the issue closed when the update closed it, with everything the
    /// update bundled.
    fn judge_claimed(&self, facts: &Facts<'_>) {
        if facts.has("--status") && facts.has("closed") {
            if facts.status != Some("closed") {
                let status = facts.status.unwrap_or("None");
                self.bad(
                    facts,
                    &format!("reported done but the remote has status {status}"),
                );
            }
        } else if facts.held.is_none() || facts.held_by_other() {
            let why = format!(
                "reported claimed but the remote owner is {}",
                facts.holder()
            );
            self.bad(facts, &why);
        }
        let issue = facts.issue.unwrap_or(&Json::Null);
        if facts.has("--title") && issue["title"].as_str() != Some("edited") {
            self.bad(
                facts,
                "reported done but the remote lacks the bundled title",
            );
        }
        if facts.has("--parent") && issue["parent"].as_str() != Some(self.keys.of('P')) {
            let parent = text(&issue["parent"]);
            self.bad(
                facts,
                &format!("reported done but the remote parent is {parent}"),
            );
        }
    }

    /// A close or a delete reported done, with no warning in its JSON: the remote must have it.
    fn judge_ended(&self, facts: &Facts<'_>, verb: &str) {
        let want = if verb == "close" {
            "closed"
        } else {
            "tombstone"
        };
        if facts.status != Some(want) {
            let step = facts.step;
            let (argv, status) = (step.argv.join(" "), facts.status.unwrap_or("None"));
            let local = text(&facts.seen["status"]);
            self.violation(format!(
                "UNPUBLISHED {} {argv} -> rc 0 (json carries no warning): remote status {status}, \
                 local status {local}",
                step.step
            ));
        }
    }

    /// A sync reported success: this machine's export must be the remote's (or, beside another
    /// command, the remote's before both).
    fn judge_synced(&self, facts: &Facts<'_>, earlier: Option<&Earlier>) {
        let (step, machine) = (facts.step, self.machine(&facts.step.machine));
        let mine = self.call(
            machine,
            &["export"],
            &step.thread,
            None,
            "export after sync",
        );
        let theirs = self.call(&self.obs, &["export"], OBS, None, "observer export");
        let before = earlier.is_some_and(|earlier| mine.raw == earlier.export);
        if mine.raw != theirs.raw && !before {
            let why = "sync reported success but this machine's export differs from the remote's";
            self.bad(facts, why);
        }
    }

    /// Records the step's answer as inconsistent with the remote, and why.
    fn bad(&self, facts: &Facts<'_>, why: &str) {
        let step = facts.step;
        let code = step.code.as_deref().unwrap_or("");
        let argv = step.argv.join(" ");
        self.violation(format!(
            "{} {argv} -> rc {} {code}: {why}",
            step.step, step.rc
        ));
    }
}

/// The issue as the actor sees it, for a step run beside another: a claim someone else held on the
/// remote before both, which this actor does not hold now, counts as seen; so does an issue that
/// was already closed or deleted.
fn before_both(facts: &Facts<'_>, earlier: &Earlier) -> Json {
    let mut seen = facts.seen.clone();
    let before = facts.key.and_then(|key| earlier.remote.get(&key));
    let Some(before) = before else {
        return seen;
    };
    let Json::Object(members) = &mut seen else {
        return seen;
    };
    let claim = &before["claim"];
    let others = claim.truthy() && claim["owner"].as_str() != Some(&facts.me);
    if others && !facts.held_by_other() {
        let own = members.get("claim").filter(|own| own.truthy()).cloned();
        members.insert("claim".to_owned(), own.unwrap_or_else(|| claim.clone()));
    }
    let status = &before["status"];
    if done(status.as_str()) {
        members.insert("status".to_owned(), status.clone());
    }
    seen
}
