//! Plain-text representations returned in MCP `content` blocks so text-only hosts and models
//! receive complete, readable output alongside the interactive MCP App.

use std::fmt::Write as _;

use crate::model::{DetailView, IssueDetail, ReadyView, WorkspaceInfo};

/// Formats the ready-issues list as plain text for non-UI clients and model context.
pub(crate) fn format_ready(view: &ReadyView) -> String {
    let mut out = String::new();
    write_workspace_header(&mut out, &view.workspace);
    if view.issues.is_empty() {
        out.push_str("No ready issues in this workspace.\n");
        return out;
    }
    let count = view.issues.len();
    let suffix = if view.has_more {
        format!(" (showing first {count}; more available via `bead ready`)")
    } else {
        format!(" ({count})")
    };
    let _ = writeln!(out, "Ready issues{suffix}:");
    for issue in &view.issues {
        let priority = issue
            .priority
            .map_or_else(|| "P?".to_owned(), |p| format!("P{p}"));
        let _ = write!(
            out,
            "- {} [{priority}] [{}] {}",
            issue.id, issue.issue_type, issue.title
        );
        if !issue.labels.is_empty() {
            let _ = write!(out, " ({})", issue.labels.join(", "));
        }
        let _ = writeln!(out, " [uid: {}]", issue.uid);
    }
    out
}

/// Formats one issue's full details as plain text for non-UI clients and model context.
pub(crate) fn format_detail(view: &DetailView) -> String {
    let mut out = String::new();
    write_workspace_header(&mut out, &view.workspace);
    write_issue_summary(&mut out, &view.issue);
    write_issue_sections(&mut out, &view.issue);
    write_issue_relations(&mut out, &view.issue);
    out
}

fn write_workspace_header(out: &mut String, ws: &WorkspaceInfo) {
    let label = ws.display_name();
    match &ws.prefix {
        Some(prefix) if prefix != label => {
            let _ = writeln!(out, "Workspace: {label} (prefix {prefix}, id {})", ws.id);
        }
        _ => {
            let _ = writeln!(out, "Workspace: {label} (id {})", ws.id);
        }
    }
    match &ws.last_sync {
        Some(sync) => {
            let _ = writeln!(out, "Last sync: {sync} (local read only)");
        }
        None => {
            let _ = writeln!(out, "Last sync: never synced (local read only)");
        }
    }
}

fn write_issue_summary(out: &mut String, issue: &IssueDetail) {
    let priority = issue
        .priority
        .map_or_else(|| "P?".to_owned(), |p| format!("P{p}"));
    let _ = writeln!(
        out,
        "\n{} · {} [{priority} · {} · {}]",
        issue.id,
        issue.title,
        issue.status.to_uppercase(),
        issue.issue_type
    );
    let _ = writeln!(out, "UID: {}", issue.uid);
    if issue.blocked {
        out.push_str("Blocked: yes\n");
    }
    if let Some(parent) = &issue.parent {
        let _ = writeln!(out, "Parent: {parent}");
    }
    if !issue.labels.is_empty() {
        let _ = writeln!(out, "Labels: {}", issue.labels.join(", "));
    }
    if let Some(assignee) = &issue.assignee {
        let _ = writeln!(out, "Assignee: {assignee}");
    }
    if let Some(claim) = &issue.claim {
        let _ = write!(out, "Claimed by: {}", claim.owner);
        if let Some(since) = &claim.since {
            let _ = write!(out, " (since {since}");
            if let Some(branch) = &claim.branch {
                let _ = write!(out, ", branch {branch}");
            }
            out.push(')');
        }
        out.push('\n');
    }
    for provisional in &issue.provisional_claims {
        let _ = write!(out, "Provisional claim (offline): {}", provisional.owner);
        if let Some(since) = &provisional.since {
            let _ = write!(out, " (since {since})");
        }
        out.push('\n');
    }
}

fn write_issue_sections(out: &mut String, issue: &IssueDetail) {
    let sections = [
        ("Description", issue.description.as_deref()),
        ("Acceptance criteria", issue.acceptance_criteria.as_deref()),
        ("Notes", issue.notes.as_deref()),
        ("Design", issue.design.as_deref()),
        ("Close reason", issue.close_reason.as_deref()),
    ];
    for (heading, body) in sections {
        if let Some(text) = body {
            let _ = writeln!(out, "\n{heading}:\n{text}");
        }
    }
    if let Some(conflicts) = &issue.conflicts {
        let fields: Vec<&str> = conflicts.keys().map(String::as_str).collect();
        let _ = writeln!(out, "\nConflicts: {}", fields.join(", "));
    }
}

fn write_issue_relations(out: &mut String, issue: &IssueDetail) {
    if !issue.dependencies.is_empty() || !issue.unresolved_dependencies.is_empty() {
        out.push_str("\nDependencies:\n");
        for dep in &issue.dependencies {
            let _ = writeln!(
                out,
                "- ({}) {} [{}] {}",
                dep.dependency_type, dep.id, dep.status, dep.title
            );
        }
        for dep in &issue.unresolved_dependencies {
            let _ = writeln!(
                out,
                "- ({}) {} (not an issue yet)",
                dep.dependency_type, dep.id
            );
        }
    }
    if !issue.dependents.is_empty() {
        out.push_str("\nDependents:\n");
        for dep in &issue.dependents {
            let _ = writeln!(
                out,
                "- ({}) {} [{}] {}",
                dep.dependency_type, dep.id, dep.status, dep.title
            );
        }
    }
    if !issue.comments.is_empty() {
        let _ = writeln!(out, "\nComments ({}):", issue.comments.len());
        for comment in &issue.comments {
            let when = comment.created_at.as_deref().unwrap_or("?");
            let _ = writeln!(out, "- [{when}] {}: {}", comment.author, comment.text);
        }
    }
}
