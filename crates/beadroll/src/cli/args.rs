//! The arguments each command takes, built from what the command line parsed.

use std::ffi::{OsStr, OsString};

use beadroll_core::json::Int;
use beadroll_sync::NetworkMode;

use super::parse::Namespace;
use crate::commands;

/// `list`'s arguments.
pub(super) fn list_args(ns: &Namespace) -> commands::ListArgs {
    commands::ListArgs {
        status: ns.texts("status").to_vec(),
        all: ns.flag("all"),
        deferred: ns.flag("deferred"),
        title_contains: owned(ns, "title_contains"),
        id: ns.texts("id").to_vec(),
        parent: owned(ns, "parent"),
        filters: filters(ns),
    }
}

/// `create`'s arguments.
pub(super) fn create_args(ns: &Namespace) -> commands::CreateArgs {
    commands::CreateArgs {
        title_pos: owned(ns, "title_pos"),
        title: owned(ns, "title"),
        fields: content(ns),
        labels: ns.texts("labels").to_vec(),
        deps: ns.texts("deps").to_vec(),
        silent: ns.flag("silent"),
    }
}

/// The arguments `close`, `reopen` and `delete` share, the network and remote among them.
pub(super) fn transition_args(ns: &Namespace) -> commands::TransitionArgs {
    commands::TransitionArgs {
        ids: ns.texts("ids").to_vec(),
        reason: owned(ns, "reason"),
        recursive: ns.flag("recursive"),
        force: ns.flag("force"),
        network: network(ns),
        remote: ns.text("remote").map(OsStr::to_os_string),
    }
}

/// `update`'s arguments, with the command line as its receipt names it.
pub(super) fn update_args(ns: &Namespace) -> commands::UpdateArgs {
    commands::UpdateArgs {
        ids: ns.texts("ids").to_vec(),
        title: owned(ns, "title"),
        fields: content(ns),
        claim: ns.flag("claim"),
        force: ns.flag("force"),
        add_label: ns.texts("add_label").to_vec(),
        remove_label: ns.texts("remove_label").to_vec(),
        set_labels: owned(ns, "set_labels"),
        network: network(ns),
        remote: owned(ns, "remote"),
        asked: ns.asked(),
    }
}

/// Whether the command may reach its coordination remote: not with `--offline`.
pub(super) fn network(ns: &Namespace) -> NetworkMode {
    if ns.flag("offline") {
        NetworkMode::Offline
    } else {
        NetworkMode::Online
    }
}

/// A text argument, owned.
pub(super) fn owned(ns: &Namespace, dest: &str) -> Option<OsString> {
    ns.text(dest).map(OsStr::to_os_string)
}

/// The fields `create` and `update` both set.
pub(super) fn content(ns: &Namespace) -> commands::ContentArgs {
    commands::ContentArgs {
        description: owned(ns, "description"),
        description_file: owned(ns, "description_file"),
        acceptance_criteria: owned(ns, "acceptance_criteria"),
        notes: owned(ns, "notes"),
        design: owned(ns, "design"),
        priority: owned(ns, "priority"),
        issue_type: owned(ns, "type"),
        status: owned(ns, "status"),
        assignee: owned(ns, "assignee"),
        estimate: ns.int("estimate").cloned(),
        external_ref: owned(ns, "external_ref"),
        parent: owned(ns, "parent"),
    }
}

/// The filters `list` and `ready` share.
pub(super) fn filters(ns: &Namespace) -> commands::FilterArgs {
    commands::FilterArgs {
        issue_type: ns.texts("type").to_vec(),
        label: ns.texts("label").to_vec(),
        priority: owned(ns, "priority"),
        assignee: owned(ns, "assignee"),
        limit: ns.int("limit").cloned().unwrap_or_else(|| Int::from(0)),
    }
}
