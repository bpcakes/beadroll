//! The docs: every push builds them with warnings denied, so a link that names nothing fails, and
//! the workspace's rustdoc lints say which links are meant: a public item's doc may name the private
//! one it depends on, for these crates are documented for those who maintain them.

use super::{job, read, step_with, workflow};

#[test]
fn every_push_builds_the_docs_with_warnings_denied() {
    let every_push = workflow(".github/workflows/rust.yml");
    let (_, clippy) = job(&every_push, "clippy");
    let at = step_with(
        &clippy,
        &["run: cargo doc", "--workspace", "--no-deps", "--locked"],
    );
    let built = clippy[at].join("\n");
    assert!(built.contains("RUSTDOCFLAGS: -D warnings"), "{built}");
}

#[test]
fn the_workspace_denies_broken_doc_links_and_allows_private_ones() {
    let manifest = read("Cargo.toml");
    let table: Vec<&str> = (manifest.lines())
        .skip_while(|line| *line != "[workspace.lints.rustdoc]")
        .skip(1)
        .take_while(|line| !line.starts_with('['))
        .map(|line| line.split('#').next().unwrap_or(line).trim())
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(
        table,
        [
            "broken_intra_doc_links = \"deny\"",
            "private_intra_doc_links = \"allow\""
        ]
    );
}
