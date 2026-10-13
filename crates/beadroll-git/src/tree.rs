//! Names of ordinary blob-only trees, to compare a desired tree with a reachable one without
//! sending its unchanged entries to mktree. This computes a name, never establishes reachability.

use crate::hash::object_id;

/// The name `mktree` gives `100644 blob OID\tNAME` entries in the object format of `hex_len`
/// (40 or 64). Entries must be in strictly increasing filename order. Unusual names or object
/// names return `None`: let mktree interpret or refuse them instead.
pub fn blob_tree_id(entries: &[(&str, &str)], hex_len: usize) -> Option<String> {
    if !matches!(hex_len, 40 | 64) {
        return None;
    }
    let mut data = Vec::new();
    let mut previous = None;
    for &(name, oid) in entries {
        if name.is_empty()
            || name.contains(['\0', '\n', '\r', '\t', '/', '"'])
            || previous.is_some_and(|before| before >= name)
            || oid.len() != hex_len
        {
            return None;
        }
        data.extend_from_slice(b"100644 ");
        data.extend_from_slice(name.as_bytes());
        data.push(0);
        let digit = |b: u8| match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            _ => None,
        };
        for pair in oid.as_bytes().as_chunks::<2>().0 {
            data.push(digit(pair[0])? * 16 + digit(pair[1])?);
        }
        previous = Some(name);
    }
    object_id("tree", &data, hex_len)
}

#[cfg(test)]
mod tests {
    use super::blob_tree_id;

    #[test]
    fn ordinary_tree_names_match_mktree_in_both_object_formats() {
        for format in ["sha1", "sha256"] {
            let (_dir, env) = crate::testing::bare(format);
            let blob = env
                .git(["hash-object", "-w", "--stdin"])
                .input(b"contents".to_vec())
                .string()
                .unwrap();
            for names in [vec![], vec!["a.json", "a.jsonl", "z.json", "é.json"]] {
                let entries: Vec<(&str, &str)> =
                    names.iter().map(|name| (*name, blob.as_str())).collect();
                let input: String = entries
                    .iter()
                    .map(|(name, oid)| format!("100644 blob {oid}\t{name}\n"))
                    .collect();
                let tree = env
                    .git(["mktree"])
                    .input(input.into_bytes())
                    .string()
                    .unwrap();
                assert_eq!(blob_tree_id(&entries, blob.len()), Some(tree));
            }
            for name in ["", "\"quoted\"", "a/b", "a\nb", "a\tb", "a\0b"] {
                assert!(blob_tree_id(&[(name, &blob)], blob.len()).is_none());
            }
            assert!(blob_tree_id(&[("b", &blob), ("a", &blob)], blob.len()).is_none());
            assert!(blob_tree_id(&[("a", &blob), ("a", &blob)], blob.len()).is_none());
        }
    }
}
