//! `bead recap` (roll-dih8.2): what is new in every tracker on the machine since a recap last marked
//! activity seen, whatever the clocks said, with each event's provenance, in three layouts. The
//! tests run bead through pipes, where nothing is marked seen unless `--advance` asks; the
//! terminal's rule is `recap::delivery`'s.

mod delivery;
mod stores;

use beadroll_core::json::{Format, write};
use beadroll_store::{Clock, uuid7};
use beadroll_test_support::{Json, Machine, code, git, json, parse_json};

use super::bp_test::segments;

pub(crate) fn create(machine: &Machine, title: &str) -> String {
    machine.bead(["create", title]).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Every event an answer shows, tracker after tracker.
pub(crate) fn events(answer: &Json) -> Vec<&Json> {
    (answer["workspaces"].as_array().unwrap().iter())
        .flat_map(|ws| ws["events"].as_array().into_iter().flatten())
        .collect()
}

/// What `member` of each event an answer shows says.
pub(crate) fn each<'a>(answer: &'a Json, member: &str) -> Vec<&'a str> {
    (events(answer).into_iter())
        .map(|event| event[member].as_str().unwrap_or("-"))
        .collect()
}

/// A recap that marks what it shows as seen, as one on a terminal does.
pub(crate) fn recap(machine: &Machine) -> Json {
    machine.bead(["recap", "--advance"]).unwrap()
}

/// The event `op` on a new issue, from machine `feedface`, built on all `machine` holds: an
/// operation a newer bead could write and this one cannot apply.
fn newer_event(machine: &Machine, op: &str) {
    let info = machine.bead(["info"]).unwrap();
    let id = uuid7(&Clock::system());
    let event = json!({
        "v": 1, "id": (id.as_str()), "replica": "feedface", "deps": (info["frontier"].clone()),
        "actor": {"agent": "newer"}, "op": op, "issue": (id.as_str()), "data": {}
    });
    let folder = machine.store().join("events/feedface");
    std::fs::create_dir_all(&folder).unwrap();
    let line = write(&event, Format::COMPACT).unwrap();
    std::fs::write(folder.join("000001.jsonl"), line + "\n").unwrap();
}

#[test]
fn the_first_recap_shows_the_last_day_and_counts_what_came_before() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    // each event's own time, as its writer's clock gave it: an hour either side of a day ago
    let hours_ago = |hours: u32| format!("clock@always=offset(-{hours}h)");
    a.cmd(["init", "--prefix", "ex"])
        .env("BEAD_FAILPOINTS", hours_ago(26))
        .json();
    for (title, hours) in [
        ("A day and an hour ago", 25),
        ("Twenty-three hours ago", 23),
    ] {
        let old = a
            .cmd(["create", title])
            .env("BEAD_FAILPOINTS", hours_ago(hours));
        old.json();
    }
    create(&a, "Today's");
    let shown = a.run(["recap"]);
    assert!(
        shown
            .stdout
            .contains("1 earlier event not shown (--all shows it)"),
        "{}",
        shown.stdout
    );
    let first = recap(&a);
    assert_eq!(each(&first, "title"), ["Twenty-three hours ago", "Today's"]);
    let tracker = &first["workspaces"][0];
    assert_eq!(tracker["earlier"].as_i64(), Some(1));
    assert!(tracker["since"].as_str().is_some_and(|s| s.ends_with('Z')));
    // what the first recap covered is seen, the event it left out included
    create(&a, "Tomorrow's");
    let later = recap(&a);
    assert_eq!(each(&later, "title"), ["Tomorrow's"]);
    assert_eq!(later["workspaces"][0]["since"], Json::Null);
    assert!(events(&recap(&a)).is_empty());
    let every = a.bead(["recap", "--all"]).unwrap();
    let titles = [
        "A day and an hour ago",
        "Twenty-three hours ago",
        "Today's",
        "Tomorrow's",
    ];
    assert_eq!(each(&every, "title"), titles);
}

#[test]
fn each_event_is_its_log_row_with_title_summary_machine_and_commit() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    git(["checkout", "-b", "feature/recap"], Some(a.checkout()));
    let key = create(&a, "Original");
    a.bead(["update", &key, "--title", "Renamed", "-p", "1"]);
    a.cmd(["comments", "add", &key, "The reason", "--actor", "Alex"])
        .agent("claude")
        .thread("whole-thread-id-123456789")
        .json();
    a.bead(["claim", &key]);
    a.bead(["close", &key, "-r", "Shipped"]);
    let machine = a.bead(["info"]).unwrap()["machine"]
        .as_str()
        .unwrap()
        .to_owned();
    a.bead(["machines", "--name", &format!("{machine}=laptop")]);
    let commit = git(["rev-parse", "HEAD"], Some(a.checkout()));
    // --json marks nothing, and leaves nothing behind
    let preview = a.bead(["recap"]).unwrap();
    assert_eq!(preview["advance"], Json::Bool(false));
    assert!(!a.home().join(".recap.json").exists());
    assert!(!a.home().join(".recap.lock").exists());
    let ops = ["create", "set", "comment", "claim", "close"];
    assert_eq!(each(&preview, "op"), ops);
    let log = a.bead(["log", "--limit", "0"]).unwrap();
    let rows = events(&preview);
    assert_eq!(rows.len(), log.as_array().unwrap().len());
    for (row, logged) in rows.iter().zip(log.as_array().unwrap()) {
        for (member, value) in logged.as_object().unwrap() {
            assert_eq!(&row[member.as_str()], value, "{member}");
        }
        assert_eq!(row["machine"].as_str(), Some("laptop"));
        assert_eq!(row["machine_id"].as_str(), Some(machine.as_str()));
        assert_eq!(row["branch"].as_str(), Some("feature/recap"));
        assert_eq!(row["commit"].as_str(), Some(commit.trim()));
        assert_eq!(row["title"].as_str(), Some("Renamed"));
    }
    assert_eq!(rows[2]["agent"].as_str(), Some("claude"));
    assert_eq!(
        rows[2]["thread"].as_str(),
        Some("whole-thread-id-123456789")
    );
    assert_eq!(rows[2]["name"].as_str(), Some("Alex"));
    let summaries = each(&preview, "summary");
    assert!(
        summaries[1].contains("title=\"Renamed\""),
        "{}",
        summaries[1]
    );
    assert_eq!(
        &summaries[2..],
        ["comment: The reason", "claim", "close: Shipped"]
    );
}

#[test]
fn the_three_layouts_show_the_same_activity() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = create(&a, "Fix the flaky sync test");
    a.bead(["claim", &key]);
    a.bead(["comments", "add", &key, "Reproduced on macOS"]);
    a.bead(["update", &key, "--notes", "see the fetch ref"]);
    a.bead(["close", &key, "-r", "fixed"]);
    let other = create(&a, "Untouched since");
    let shown = |layout: &str| {
        let run = a.run(["recap", "--all", "--format", layout, "--color", "never"]);
        assert_eq!(run.returncode(), 0, "{}", run.stderr);
        run.stdout
    };
    // events, the default: one line per event, as log prints it, with the title after the issue
    let events = shown("events");
    assert_eq!(a.run(["recap", "--all", "--color", "never"]).stdout, events);
    let lines: Vec<&str> = events.lines().skip(1).collect();
    assert_eq!(lines.len(), 6, "{events}");
    let log = a.run(["log", "--limit", "0", "--color", "never"]).stdout;
    for (line, logged) in lines.iter().zip(log.lines()) {
        let (before, branch) = logged.rsplit_once("  (").unwrap_or((logged, ""));
        assert!(
            line.starts_with(&format!("  {before}  ")),
            "{line}\n{logged}"
        );
        assert!(line.ends_with(branch), "{line}\n{logged}");
    }
    assert!(lines[0].contains("Fix the flaky sync test"), "{events}");
    // issues: one line per issue, in the order of its last event
    let issues = shown("issues");
    let lines: Vec<&str> = issues.lines().skip(1).collect();
    assert_eq!(lines.len(), 2, "{issues}");
    let happened = "created, claimed, closed, 1 comment, 1 edit";
    assert!(lines[0].contains(&format!("✓ {key}")), "{issues}");
    assert!(lines[0].contains(happened), "{issues}");
    assert!(lines[1].contains(&format!("○ {other}  Untouched since  created")));
    // full: each event's summary, machine, whole thread and commit
    let full = shown("full");
    assert!(
        full.contains("    comment: Reproduced on macOS\n"),
        "{full}"
    );
    assert!(full.contains("thread=a-thread-0001 name=-"), "{full}");
    let commit = git(["rev-parse", "HEAD"], Some(a.checkout()));
    assert!(
        full.contains(&format!("commit={}", commit.trim())),
        "{full}"
    );
    // --format shapes human output only: --json is the same under every layout
    let plain = a.bead(["recap", "--all"]).unwrap();
    for layout in ["events", "issues", "full"] {
        let json = a.bead(["recap", "--all", "--format", layout]).unwrap();
        assert_eq!(json, plain, "--format {layout}");
    }
    let wrong = a.run(["recap", "--format", "table", "--json"]);
    assert_eq!((wrong.returncode(), code(&wrong)), (2, "USAGE".to_owned()));
}

#[test]
fn an_issue_row_names_who_wrote_its_last_event_and_when() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    // created two hours ago by the machine's agent, and commented on last by Alex, now
    let created = a
        .cmd(["create", "Reviewed"])
        .env("BEAD_FAILPOINTS", "clock@always=offset(-2h)")
        .json()
        .unwrap();
    let key = created["id"].as_str().unwrap();
    a.cmd(["comments", "add", key, "Looks right", "--actor", "Alex"])
        .json();
    let answer = a.bead(["recap"]).unwrap();
    let rows = events(&answer);
    assert_eq!(each(&answer, "op"), ["create", "comment"]);
    let at = |row: &Json| row["at"].as_str().unwrap()[..19].to_owned();
    let issues = a.run(["recap", "--format", "issues", "--color", "never"]);
    let line = (issues.stdout.lines())
        .find(|line| line.contains(key))
        .unwrap();
    let machine = rows[1]["machine"].as_str().unwrap();
    let last = format!("  {machine} Alex  {}", at(rows[1]));
    assert!(line.ends_with(&last), "{line}\n{last}");
    assert!(!line.contains(&at(rows[0])), "{line}");
}

#[test]
fn issue_rows_follow_the_order_of_each_ones_last_event() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    // created first and written to last: by its first event it would come first
    let busy = create(&a, "Written to last");
    let quiet = create(&a, "Created after it");
    a.bead(["comments", "add", &busy, "Still going"]);
    let shown = a.run(["recap", "--all", "--format", "issues", "--color", "never"]);
    assert_eq!(shown.returncode(), 0, "{}", shown.stderr);
    let rows: Vec<&str> = shown.stdout.lines().skip(1).collect();
    assert_eq!(rows.len(), 2, "{}", shown.stdout);
    let quiet = format!("○ {quiet}  Created after it  created");
    assert!(rows[0].contains(&quiet), "{}", shown.stdout);
    let busy = format!("○ {busy}  Written to last  created, 1 comment");
    assert!(rows[1].contains(&busy), "{}", shown.stdout);
}

#[test]
fn an_event_from_a_machine_whose_clock_is_behind_shows_when_it_arrives() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    let b = crate::rust_only(&sandbox, "b");
    a.bead(["init", "--prefix", "ex"]);
    let key = create(&a, "Shared");
    b.bead(["sync"]);
    b.bead(["comments", "add", &key, "Seen first"]);
    a.bead(["sync"]);
    let first = recap(&a);
    let previous = events(&first).last().unwrap()["at"]
        .as_str()
        .unwrap()
        .to_owned();
    b.cmd(["comments", "add", &key, "Arrived later", "--offline"])
        .env("BEAD_FAILPOINTS", "clock@always=offset(-7d)")
        .json();
    assert!(events(&recap(&a)).is_empty(), "recap itself never syncs");
    b.bead(["sync"]);
    a.bead(["sync"]);
    let later = recap(&a);
    assert_eq!(each(&later, "summary"), ["comment: Arrived later"]);
    let row = events(&later)[0];
    assert!(row["at"].as_str().unwrap() < previous.as_str());
    assert_eq!(row["thread"].as_str(), Some("b-thread-0001"));
    assert!(events(&recap(&a)).is_empty());
    let all = a.bead(["recap", "--all"]).unwrap();
    let log = a.bead(["log", "--limit", "0"]).unwrap();
    let logged: Vec<&Json> = (log.as_array().unwrap().iter())
        .map(|row| &row["event"])
        .collect();
    assert_eq!(
        events(&all)
            .iter()
            .map(|row| &row["event"])
            .collect::<Vec<_>>(),
        logged
    );
}

#[test]
fn an_ownership_attempt_shows_once_the_remote_has_accepted_it() {
    let sandbox = crate::sandbox();
    let a = crate::rust_only(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let key = create(&a, "Claim me");
    recap(&a);
    let stopped = a
        .cmd(["claim", &key])
        .env("BEAD_TEST_CRASH_AFTER_PUSH", "1")
        .run();
    assert_eq!(stopped.returncode(), 99, "{}", stopped.stderr);
    assert!(events(&recap(&a)).is_empty());
    a.bead(["sync"]);
    assert_eq!(each(&recap(&a), "op"), ["claim"]);
}

#[test]
fn events_waiting_for_a_cause_show_once_it_arrives() {
    let sandbox = crate::sandbox();
    let mut a = Machine::new(&sandbox, "a");
    a.extra.insert("BEAD_ROTATE_BYTES".into(), "1".into());
    a.bead(["init", "--prefix", "ex"]);
    let key = create(&a, "Waiting");
    recap(&a);
    a.bead(["comments", "add", &key, "Cause", "--offline"]);
    let mut written = segments(&a.store());
    written.sort();
    let cause = written.pop().unwrap();
    a.bead(["comments", "add", &key, "Effect", "--offline"]);
    let held = cause.with_extension("held");
    std::fs::rename(&cause, &held).unwrap();
    let waiting = recap(&a);
    assert!(events(&waiting).is_empty());
    assert_eq!(waiting["workspaces"][0]["pending"].as_i64(), Some(1));
    std::fs::rename(held, cause).unwrap();
    let shown = each(&recap(&a), "summary").join("; ");
    assert_eq!(shown, "comment: Cause; comment: Effect");
}

#[test]
fn an_event_this_bead_cannot_apply_is_reported_and_the_cursor_still_moves() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    create(&a, "Before");
    recap(&a);
    newer_event(&a, "archive");
    let shown = recap(&a);
    assert_eq!(each(&shown, "op"), ["archive"]);
    let problems = shown["workspaces"][0]["integrity"].as_array().unwrap();
    assert_eq!(problems.len(), 1, "{shown:?}");
    create(&a, "After");
    let later = recap(&a);
    assert_eq!(each(&later, "title"), ["After"]);
    let told = a.run(["recap"]).stdout;
    assert!(told.contains("  1 integrity problem:\n"), "{told}");
    let listed = format!(
        "    {}: unknown operation 'archive'\n",
        problems[0]["issue"].as_str().unwrap()
    );
    assert!(told.contains(&listed), "{told}");
    assert!(told.contains("No new activity"), "{told}");
    // among the issues it has a row too: no status or title the state could give, the id it names
    let issues = a.run(["recap", "--all", "--format", "issues", "--color", "never"]);
    let row = format!(
        "  ? {}    1 other event  ",
        problems[0]["issue"].as_str().unwrap()
    );
    assert!(issues.stdout.contains(&row), "{}", issues.stdout);
}

#[test]
fn hostile_text_reaches_no_layout_raw_and_json_keeps_it() {
    let sandbox = crate::sandbox();
    let a = Machine::new(&sandbox, "a");
    a.bead(["init", "--prefix", "ex"]);
    let title = "題名\u{1b}[31m\nforged\u{202e}";
    let key = create(&a, title);
    a.bead(["comments", "add", &key, "text\r\u{1b}]8;;bad\u{7}"]);
    for layout in ["events", "issues", "full"] {
        let shown = a
            .cmd(["recap", "--format", layout])
            .env("COLUMNS", "20")
            .env("NO_COLOR", "1")
            .run();
        assert_eq!(shown.returncode(), 0, "{}", shown.stderr);
        assert!(
            !shown.stdout.contains(['\u{1b}', '\r', '\u{7}']),
            "{layout}"
        );
        assert!(
            shown.stdout.contains("題名\\x1b[31m\\x0aforged\\u202e"),
            "{layout}"
        );
    }
    let json = a.run(["recap", "--json", "--color", "always"]);
    assert!(json.stderr.is_empty());
    assert!(!json.stdout.contains('\u{1b}'));
    let answer = parse_json(&json.stdout).unwrap();
    assert_eq!(each(&answer, "title")[0], title);
}
