//! Journal files named by their paths in the store.

mod relative_names {
    use std::borrow::Cow;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;
    use std::path::{Path, PathBuf};

    use beadroll_core::json::Object;
    use beadroll_store::{Workspace, relative, relative_name};

    /// `relative_name` borrows the name of a path the store's listings make and says what
    /// `relative` says of every path (roll-lqjj.47: it takes the 92,000 attempt files of a command
    /// apart by their bytes rather than their components), the ones `Path::strip_prefix` cleans up
    /// and the ones it cannot give a name among them.
    #[test]
    fn a_name_is_the_one_relative_gives_however_the_path_is_spelled() {
        let home = tempfile::tempdir().unwrap();
        let settings = crate::settings(home.path(), &[]);
        let ws = Workspace::new(settings, "names", None, None, Object::new()).unwrap();
        let dir = ws.dir.clone();
        let text = |path: &Path| path.to_string_lossy().into_owned();
        // the name of a path as `relative` has always given it
        let cleaned = |path: &Path| text(path.strip_prefix(&ws.dir).unwrap_or(path));

        let listed = dir.join("events/m/claims/a.json");
        assert!(matches!(
            relative_name(&ws, &listed),
            Cow::Borrowed("events/m/claims/a.json")
        ));
        assert_eq!(relative(&ws, &listed), "events/m/claims/a.json");

        let sibling = PathBuf::from(format!("{}2/events/x.json", text(&dir)));
        let trailing = PathBuf::from(format!("{}/", text(&dir)));
        let doubled = PathBuf::from(format!("{}//events/x.json", text(&dir)));
        let dotted = PathBuf::from(format!("{}/./events/x.json", text(&dir)));
        let inner = PathBuf::from(format!("{}/events//m/./x.json", text(&dir)));
        let odd = dir.join(OsStr::from_bytes(b"events/m/\xff.json"));
        // a name that ends in a separator or a `/.`: `strip_prefix` drops it
        let ending = PathBuf::from(format!("{}/events/m/a.json/", text(&dir)));
        let ending_in_dot = PathBuf::from(format!("{}/events/m/a.json/.", text(&dir)));
        let paths = [
            listed,
            dir.clone(),
            trailing,
            doubled,
            dotted,
            inner,
            odd,
            ending,
            ending_in_dot,
            sibling,
            dir.parent().unwrap().join("elsewhere/x.json"),
            PathBuf::from("events/x.json"),
            PathBuf::new(),
        ];
        for path in &paths {
            assert_eq!(relative_name(&ws, path), cleaned(path), "{path:?}");
            assert_eq!(relative(&ws, path), cleaned(path), "{path:?}");
        }

        // and every spelling of up to five pieces after the directory, a piece being nothing, a
        // dot, two dots or a name, joined by a separator or, as a sibling's name is, by nothing
        let pieces = ["", ".", "..", "a", "b.json"];
        let mut layer: Vec<Vec<&str>> = vec![Vec::new()];
        let mut spellings = layer.clone();
        for _ in 0..5 {
            layer = (layer.iter())
                .flat_map(|spelling| {
                    pieces.iter().map(move |piece| {
                        let mut longer = spelling.clone();
                        longer.push(piece);
                        longer
                    })
                })
                .collect();
            spellings.extend(layer.iter().cloned());
        }
        for spelling in &spellings {
            for joiner in ["/", ""] {
                let path = PathBuf::from(format!("{}{joiner}{}", text(&dir), spelling.join("/")));
                assert_eq!(relative_name(&ws, &path), cleaned(&path), "{path:?}");
            }
        }
    }
}
