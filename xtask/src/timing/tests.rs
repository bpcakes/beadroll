//! What `cargo xtask timing` reads of its arguments and writes of what it measured.

use super::*;
use std::time::Duration;

#[test]
fn dates_are_civil_utc() {
    assert_eq!(utc(UNIX_EPOCH), "1970-01-01T00:00:00Z");
    assert_eq!(
        utc(UNIX_EPOCH + Duration::from_secs(951_782_400)),
        "2000-02-29T00:00:00Z"
    );
    assert_eq!(
        utc(UNIX_EPOCH + Duration::from_secs(1_791_558_245)),
        "2026-10-09T15:04:05Z"
    );
    assert_eq!(
        utc(UNIX_EPOCH + Duration::from_secs(4_102_444_799)),
        "2099-12-31T23:59:59Z"
    );
}

#[test]
fn a_record_is_one_json_object() {
    let record = Record {
        label: Some("replay \"10k\""),
        command: &[OsString::from("cargo"), OsString::from("bench")],
        status: ExitStatus::from_raw(0),
        seconds: 1.5,
        started: UNIX_EPOCH,
        commit: None,
        dirty: Some(false),
    };
    let json = record.to_json();
    assert!(json.starts_with(
        "{\"label\": \"replay \\\"10k\\\"\", \"command\": [\"cargo\", \"bench\"], \"status\": 0, \
         \"seconds\": 1.500, \"started\": \"1970-01-01T00:00:00Z\", \"commit\": null, \"dirty\": false"
    ), "{json}");
    assert!(!json.contains('\n'));
}

#[test]
fn the_command_follows_two_dashes() {
    let args = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
    let options = parse(args(&["--label", "x", "--", "sleep", "--label"])).unwrap();
    assert_eq!(options.label.as_deref(), Some("x"));
    assert_eq!(options.command, args(&["sleep", "--label"]));
    assert!(matches!(
        parse(args(&["--label", "x"])),
        Err(Failure::Usage(_))
    ));
    assert!(matches!(parse(args(&["sleep"])), Err(Failure::Usage(_))));
}

#[test]
fn sync_takes_a_count_a_shape_and_a_rotation() {
    let args = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
    let options = parse(args(&["--sync", "200", "--ownership", "--rotate", "4096"])).unwrap();
    assert_eq!(options.sync, Some(200));
    assert_eq!(options.rotate, Some(4096));
    assert_eq!(options.shape, history::Shape::Ownership);
    assert_eq!(parse(args(&["--sync", "10"])).unwrap().rotate, None);
    for wrong in [
        &["--sync", "ten"][..],
        &["--sync", "10", "--replay", "10"],
        &["--sync", "10", "--rotate", "0"],
        &["--replay", "10", "--rotate", "4096"],
        &["--rotate", "4096", "--", "true"],
    ] {
        assert!(
            matches!(parse(args(wrong)), Err(Failure::Usage(_))),
            "{wrong:?}"
        );
    }
}

#[test]
fn concurrent_goes_with_merge() {
    let args = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
    let options = parse(args(&["--merge", "500", "--concurrent", "--text"])).unwrap();
    assert!(options.concurrent);
    assert!(parse(args(&["--read", "500", "--concurrent"])).is_err());
}

#[test]
fn contend_takes_machines_writes_and_a_latency() {
    let args = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
    let options = parse(args(&[
        "--contend",
        "4",
        "--writes",
        "50",
        "--latency",
        "0",
    ]))
    .unwrap();
    assert_eq!(options.contend, Some(4));
    assert_eq!(options.writes, Some(50));
    assert_eq!(options.latency, Some(0));
    for alone in [
        &["--writes", "5"][..],
        &["--latency", "100"],
        &["--contend", "0"],
    ] {
        assert!(parse(args(alone)).is_err(), "{alone:?}");
    }
    assert!(parse(args(&["--contend", "2", "--read", "10"])).is_err());
}

#[test]
fn extend_takes_a_count_of_issues_and_is_a_measurement_of_its_own() {
    let args = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
    let options = parse(args(&["--extend", "8000"])).unwrap();
    assert_eq!(options.extend, Some(8000));
    // The count reaches the extension measurement and no other: an arm that set another
    // mode's field, or none, would leave this one empty.
    let modes = [
        options.replay,
        options.sync,
        options.read,
        options.merge,
        options.contend,
        options.import,
        options.plan,
        options.dangling,
        options.ready,
        options.details,
        options.renames,
        options.decode,
        options.parse,
    ];
    assert!(modes.iter().all(Option::is_none), "{modes:?}");
    for wrong in [
        &["--extend", "many"][..],
        &["--extend"],
        // counted among the measurements, so it is refused beside another
        &["--extend", "10", "--plan", "10"],
        &["--extend", "10", "--replay", "10"],
        &["--extend", "10", "--rotate", "4096"],
    ] {
        assert!(
            matches!(parse(args(wrong)), Err(Failure::Usage(_))),
            "{wrong:?}"
        );
    }
}

#[test]
fn dangling_takes_a_count_of_issues_and_is_a_measurement_of_its_own() {
    let args = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
    let options = parse(args(&["--dangling", "16000"])).unwrap();
    assert_eq!(options.dangling, Some(16000));
    // The count reaches this measurement and no other.
    let modes = [
        options.replay,
        options.sync,
        options.read,
        options.merge,
        options.contend,
        options.import,
        options.plan,
        options.extend,
        options.ready,
        options.details,
        options.renames,
        options.decode,
        options.parse,
    ];
    assert!(modes.iter().all(Option::is_none), "{modes:?}");
    for wrong in [
        &["--dangling", "many"][..],
        &["--dangling"],
        // counted among the measurements, so it is refused beside another
        &["--dangling", "10", "--extend", "10"],
        &["--dangling", "10", "--replay", "10"],
        &["--dangling", "10", "--rotate", "4096"],
    ] {
        assert!(
            matches!(parse(args(wrong)), Err(Failure::Usage(_))),
            "{wrong:?}"
        );
    }
}

#[test]
fn ready_takes_a_count_of_issues_and_is_a_measurement_of_its_own() {
    let args = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
    let options = parse(args(&["--ready", "4000"])).unwrap();
    assert_eq!(options.ready, Some(4000));
    // The count reaches this measurement and no other.
    let modes = [
        options.replay,
        options.sync,
        options.read,
        options.merge,
        options.contend,
        options.import,
        options.plan,
        options.extend,
        options.dangling,
        options.details,
        options.renames,
        options.decode,
        options.parse,
    ];
    assert!(modes.iter().all(Option::is_none), "{modes:?}");
    for wrong in [
        &["--ready", "many"][..],
        &["--ready"],
        // counted among the measurements, so it is refused beside another
        &["--ready", "10", "--dangling", "10"],
        &["--ready", "10", "--replay", "10"],
        &["--ready", "10", "--rotate", "4096"],
    ] {
        assert!(
            matches!(parse(args(wrong)), Err(Failure::Usage(_))),
            "{wrong:?}"
        );
    }
}

#[test]
fn details_takes_a_count_of_issues_and_is_a_measurement_of_its_own() {
    let args = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
    let options = parse(args(&["--details", "8000"])).unwrap();
    assert_eq!(options.details, Some(8000));
    // The count reaches this measurement and no other.
    let modes = [
        options.replay,
        options.sync,
        options.read,
        options.merge,
        options.contend,
        options.import,
        options.plan,
        options.extend,
        options.dangling,
        options.ready,
        options.renames,
        options.decode,
        options.parse,
    ];
    assert!(modes.iter().all(Option::is_none), "{modes:?}");
    for wrong in [
        &["--details", "many"][..],
        &["--details"],
        // counted among the measurements, so it is refused beside another
        &["--details", "10", "--ready", "10"],
        &["--details", "10", "--replay", "10"],
        &["--details", "10", "--rotate", "4096"],
    ] {
        assert!(
            matches!(parse(args(wrong)), Err(Failure::Usage(_))),
            "{wrong:?}"
        );
    }
}

#[test]
fn renames_takes_a_count_of_copies_and_is_a_measurement_of_its_own() {
    let args = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
    let options = parse(args(&["--renames", "8000"])).unwrap();
    assert_eq!(options.renames, Some(8000));
    // The count reaches this measurement and no other.
    let modes = [
        options.replay,
        options.sync,
        options.read,
        options.merge,
        options.contend,
        options.import,
        options.plan,
        options.extend,
        options.dangling,
        options.ready,
        options.details,
        options.decode,
        options.parse,
    ];
    assert!(modes.iter().all(Option::is_none), "{modes:?}");
    for wrong in [
        &["--renames", "many"][..],
        &["--renames"],
        // counted among the measurements, so it is refused beside another
        &["--renames", "10", "--details", "10"],
        &["--renames", "10", "--replay", "10"],
        &["--renames", "10", "--rotate", "4096"],
    ] {
        assert!(
            matches!(parse(args(wrong)), Err(Failure::Usage(_))),
            "{wrong:?}"
        );
    }
}

#[test]
fn decode_takes_a_count_of_events_a_shape_and_is_a_measurement_of_its_own() {
    let args = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
    let options = parse(args(&["--decode", "200000", "--ownership"])).unwrap();
    assert_eq!(options.decode, Some(200_000));
    assert_eq!(options.shape, history::Shape::Ownership);
    // The count reaches this measurement and no other.
    let modes = [
        options.replay,
        options.sync,
        options.read,
        options.merge,
        options.contend,
        options.import,
        options.plan,
        options.extend,
        options.dangling,
        options.ready,
        options.details,
        options.renames,
        options.parse,
    ];
    assert!(modes.iter().all(Option::is_none), "{modes:?}");
    for wrong in [
        &["--decode", "many"][..],
        &["--decode"],
        // counted among the measurements, so it is refused beside another
        &["--decode", "10", "--read", "10"],
        &["--decode", "10", "--replay", "10"],
        &["--decode", "10", "--rotate", "4096"],
    ] {
        assert!(
            matches!(parse(args(wrong)), Err(Failure::Usage(_))),
            "{wrong:?}"
        );
    }
}

#[test]
fn parse_takes_a_count_of_files_a_shape_and_is_a_measurement_of_its_own() {
    let args = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
    let options = parse(args(&["--parse", "92000", "--ownership"])).unwrap();
    assert_eq!(options.parse, Some(92_000));
    assert_eq!(options.shape, history::Shape::Ownership);
    // The count reaches this measurement and no other.
    let modes = [
        options.replay,
        options.sync,
        options.read,
        options.merge,
        options.contend,
        options.import,
        options.plan,
        options.extend,
        options.dangling,
        options.ready,
        options.details,
        options.renames,
        options.decode,
    ];
    assert!(modes.iter().all(Option::is_none), "{modes:?}");
    for wrong in [
        &["--parse", "many"][..],
        &["--parse"],
        // counted among the measurements, so it is refused beside another
        &["--parse", "10", "--decode", "10"],
        &["--parse", "10", "--replay", "10"],
        &["--parse", "10", "--rotate", "4096"],
    ] {
        assert!(
            matches!(parse(args(wrong)), Err(Failure::Usage(_))),
            "{wrong:?}"
        );
    }
}

#[test]
fn ownership_shapes_a_replay_and_load_names_a_directory() {
    let args = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
    let options = parse(args(&["--replay", "10", "--ownership"])).unwrap();
    assert_eq!(options.replay, Some(10));
    assert_eq!(options.shape, history::Shape::Ownership);
    assert_eq!(
        parse(args(&["--replay", "10"])).unwrap().shape,
        history::Shape::Plain
    );
    let options = parse(args(&["--load", "dir", "--no-replay"])).unwrap();
    assert_eq!(options.load, Some(PathBuf::from("dir")));
    assert!(options.no_replay);
    for wrong in [
        &["--ownership", "--", "true"][..],
        &["--replay", "10", "--no-replay"],
        &["--load"],
    ] {
        assert!(
            matches!(parse(args(wrong)), Err(Failure::Usage(_))),
            "{wrong:?}"
        );
    }
}
