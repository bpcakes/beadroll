//! Failed cache writes cannot acknowledge prepared locations; the same state can retry.

use super::*;
use crate::projection::{extend_projection, save, texts_path};
use beadroll_core::projection::{Extended, Record};
use beadroll_core::state::Texts;

fn comment() -> Value {
    let mut data = Object::new();
    data.insert(
        "text".into(),
        Value::from("written after the cached snapshot"),
    );
    event(5, &[4], "comment", 2, data)
}

#[derive(Clone, Copy)]
enum Preparation {
    Append,
    Rewrite,
}

#[test]
fn failed_projection_writes_leave_new_texts_unacknowledged_for_retry() {
    for preparation in [Preparation::Append, Preparation::Rewrite] {
        let point = "atomic-write:/projection";
        let home = tempfile::tempdir().unwrap();
        let ws = store(home.path(), Some("this build"));
        begun(&ws);
        assert!(replays(&ws));
        let before = std::fs::read(texts_path(&ws)).unwrap();
        let mut projected = projection(&ws).unwrap();
        let update = comment();
        let Extended::Applied(next) = extend_projection(
            &ws,
            projected,
            vec![Record {
                value: update.clone(),
                replica: HERE.into(),
                origin: "test".into(),
                sequenced: true,
            }],
        )
        .unwrap() else {
            panic!("sequential extension")
        };
        projected = *next;
        append_events(&ws, HERE, &[update]).unwrap();
        // Keep the identities of the exact journal used by this projection's writeback.
        let mut made = file::read(&projection_path(&ws)).unwrap().made;
        let bytes = std::fs::read(segment(&ws)).unwrap();
        for held in &mut made.files {
            if held.segment {
                held.key = FileKey::of(&segment(&ws)).unwrap();
                held.sum = Some(content_sum(&bytes));
            }
        }
        if matches!(preparation, Preparation::Rewrite) {
            projected = beadroll_core::projection::project(load_journal(&ws).unwrap()).unwrap();
        }
        let generation = projected.state().texts_file();
        let failure = failing(
            home.path(),
            Some("this build"),
            Some(&format!(
                "atomic-write:/projection-texts@always=sleep(0);{point}@always=errno(ENOSPC)"
            )),
        );
        save(&failure, made.clone(), &mut projected);
        assert_eq!(failure.settings.failpoints.hits(point), 1);
        let uid = EventId::parse(&id(2)).unwrap();
        assert!(matches!(
            projected.state().issue(uid).unwrap().texts(),
            Texts::Here
        ));
        assert_eq!(projected.state().texts_file(), generation);
        if matches!(preparation, Preparation::Append) {
            assert!(std::fs::read(texts_path(&ws)).unwrap().starts_with(&before));
        } else {
            assert_eq!(
                failure
                    .settings
                    .failpoints
                    .hits("atomic-write:/projection-texts"),
                1
            );
        }

        save(&ws, made, &mut projected);
        assert!(matches!(
            projected.state().issue(uid).unwrap().texts(),
            Texts::Read(_)
        ));
        assert!(replays(&ws));
        assert_eq!(
            view(&ws).unwrap().hydrated().unwrap().export_records(),
            replay(load_journal(&ws).unwrap())
                .unwrap()
                .hydrated()
                .unwrap()
                .export_records()
        );
    }
}

#[test]
fn a_missing_or_replaced_text_file_declines_the_prepared_batch() {
    for replacement in [None, Some(b"another generation".as_slice())] {
        let home = tempfile::tempdir().unwrap();
        let ws = store(home.path(), Some("this build"));
        begun(&ws);
        assert!(replays(&ws));
        let projected = projection(&ws).unwrap();
        match replacement {
            None => std::fs::remove_file(texts_path(&ws)).unwrap(),
            Some(bytes) => std::fs::write(texts_path(&ws), bytes).unwrap(),
        }
        let update = comment();
        assert!(matches!(
            extend_projection(
                &ws,
                projected,
                vec![Record {
                    value: update.clone(),
                    replica: HERE.into(),
                    origin: "test".into(),
                    sequenced: true,
                }]
            )
            .unwrap(),
            Extended::Replay
        ));
        append_events(&ws, HERE, &[update]).unwrap();
        assert!(replays(&ws));
    }
}
