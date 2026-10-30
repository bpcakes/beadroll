//! The registry: what parses, what is refused, and which hits fire.

use std::ffi::OsStr;

use super::*;
use crate::CrashHooks;

fn points(spec: &str) -> Result<Failpoints, Error> {
    Failpoints::from_env(
        Some(OsStr::new(spec)),
        CrashHooks {
            before_push: false,
            after_push: false,
        },
    )
}

fn refused(spec: &str) -> String {
    match points(spec) {
        Err(Error::Config(message)) => message,
        other => panic!("{spec:?} was not refused: {other:?}"),
    }
}

#[test]
fn a_spec_fires_at_its_first_hit_unless_told_otherwise() {
    let fp = points("backoff=skip").unwrap();
    assert_eq!(fp.hit("backoff"), Some(Fired::Skip));
    assert_eq!(fp.hit("backoff"), None);
    assert_eq!(fp.hit("push.before"), None);
    let fp = points("backoff@nth(3)=skip").unwrap();
    let hits: Vec<_> = (0..5).map(|_| fp.hit("backoff")).collect();
    assert_eq!(hits, [None, None, Some(Fired::Skip), None, None]);
    let fp = points("backoff@always=skip").unwrap();
    assert!((0..12).all(|_| fp.hit("backoff") == Some(Fired::Skip)));
    assert_eq!(fp.hits("backoff"), 12);
    assert_eq!(
        fp.hits("push.before"),
        0,
        "a point no spec names is not counted"
    );
}

#[test]
fn every_action_reads_as_its_point_takes_it() {
    let fp = points(
        " push.reply@always=fail-reply ; publish.result=error(REMOTE_UNREACHABLE);\
         atomic-write:pending/=errno(ENOSPC);append.mid=torn-sigkill;clock@always=offset(-48h);\
         push.before=sleep(0);ownership.after-confirm=error(IO)",
    )
    .unwrap();
    assert_eq!(
        fp.hit("ownership.after-confirm"),
        Some(Fired::Error(Code::Io))
    );
    assert_eq!(fp.hit("push.reply"), Some(Fired::FailReply));
    assert_eq!(
        fp.hit("publish.result"),
        Some(Fired::Error(Code::RemoteUnreachable))
    );
    assert_eq!(fp.hit("append.mid"), Some(Fired::TornSigkill));
    assert_eq!(
        fp.hit("clock"),
        Some(Fired::Offset(-48 * 3_600_000_000_000))
    );
    assert_eq!(fp.hit("push.before"), None);
    let written = Path::new("/s/ws/pending/x.json");
    assert_eq!(fp.hit_path("atomic-written", written), None);
    assert_eq!(
        fp.hit_path("atomic-write", Path::new("/s/ws/meta.json")),
        None
    );
    assert!(matches!(
        fp.hit_path("atomic-write", written),
        Some(Fired::Errno(_))
    ));
    assert_eq!(
        points("clock=at(4429000000000000000)")
            .unwrap()
            .hit("clock"),
        Some(Fired::At(4_429_000_000_000_000_000))
    );
}

#[test]
fn what_cannot_fire_as_written_is_refused() {
    for spec in [
        "backoff",
        "nonsense=skip",
        "backoff=explode",
        "backoff@nth(0)=skip",
        "backoff@sometimes=skip",
        "backoff=skip;backoff@always=skip",
        "push.before=skip",
        "clock=exit(1)",
        "atomic-write:=sigkill",
        "atomic-written:x=errno(ENOSPC)",
        "publish.result=error(NO_SUCH_CODE)",
        "push.before=error(IO)",
        "backoff=exit(nine)",
        "backoff=sleep(1",
        "clock=offset(5)",
    ] {
        assert!(refused(spec).starts_with("BEAD_FAILPOINTS: "), "{spec}");
    }
    assert!(points("").unwrap().hit("backoff").is_none());
}

#[test]
fn the_crash_hooks_are_failpoints() {
    let fp = Failpoints::from_env(
        None,
        CrashHooks {
            before_push: true,
            after_push: true,
        },
    )
    .unwrap();
    assert_eq!(fp.specs.len(), 2);
    assert_eq!(fp.specs[0].action, Action::Exit(98));
    assert_eq!(fp.specs[1].point, "ownership.after-publish-ok");
    let both = Failpoints::from_env(
        Some(OsStr::new("ownership.before-publish=sigkill")),
        CrashHooks {
            before_push: true,
            after_push: false,
        },
    );
    assert!(matches!(both, Err(Error::Config(_))));
}

#[test]
fn a_pause_waits_for_its_go_file() {
    let dir = tempfile::tempdir().unwrap();
    let at = dir.path().join("gate");
    let fp = points(&format!("push.before=pause-until({})", at.display())).unwrap();
    let go = at.with_extension("go");
    let ready = at.with_extension("ready");
    let opener = std::thread::spawn(move || {
        while !ready.exists() {
            std::thread::sleep(Duration::from_millis(1));
        }
        std::fs::write(go, b"").unwrap();
    });
    assert_eq!(fp.hit("push.before"), None);
    opener.join().unwrap();
}
