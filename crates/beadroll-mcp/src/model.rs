//! Typed workspace, ready-list, and issue-detail models decoded from `bead --json` output and
//! serialized into MCP `structuredContent`.

use serde_json::{Map, Value, json};

use crate::bead::AppError;

/// Metadata about the checkout's beadroll workspace, from `bead info --json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceInfo {
    pub(crate) id: String,
    pub(crate) prefix: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) last_sync: Option<String>,
    pub(crate) bead_version: Option<String>,
    pub(crate) tracked_config_warning: Option<String>,
    pub(crate) checkout: String,
}

/// One row of the ready list, in beadroll's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReadyIssue {
    pub(crate) id: String,
    pub(crate) uid: String,
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) priority: Option<i64>,
    pub(crate) issue_type: String,
    pub(crate) assignee: Option<String>,
    pub(crate) labels: Vec<String>,
    pub(crate) branch: Option<String>,
    pub(crate) conflicted: Vec<String>,
    pub(crate) created_at: Option<String>,
    pub(crate) updated_at: Option<String>,
}

/// Complete response payload for `bead_ready`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReadyView {
    pub(crate) workspace: WorkspaceInfo,
    pub(crate) limit: usize,
    pub(crate) has_more: bool,
    pub(crate) issues: Vec<ReadyIssue>,
}

/// Confirmed or provisional ownership claim on an issue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaimInfo {
    pub(crate) owner: String,
    pub(crate) since: Option<String>,
    pub(crate) generation: Option<i64>,
    pub(crate) branch: Option<String>,
}

/// An unconfirmed ownership attempt awaiting the remote or a stale ignored ownership event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OwnershipNotice {
    pub(crate) op: String,
    pub(crate) by: String,
}

/// A claim attempt that lost an ownership race to another owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LostClaim {
    pub(crate) by: String,
    pub(crate) held_by: String,
}

/// Whether a closed issue's closing commit has been merged into the default branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MergedStatus {
    Merged,
    NotMerged,
    Unknown,
}

impl MergedStatus {
    fn to_json(self) -> Value {
        match self {
            Self::Merged => Value::Bool(true),
            Self::NotMerged => Value::Bool(false),
            Self::Unknown => Value::from("unknown"),
        }
    }
}

/// A dependency or dependent issue linked from an issue detail view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RelatedIssue {
    pub(crate) id: String,
    pub(crate) uid: Option<String>,
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) dependency_type: String,
}

/// An imported dependency awaiting a key that is not an issue yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnresolvedDep {
    pub(crate) id: String,
    pub(crate) dependency_type: String,
}

/// One comment on an issue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommentItem {
    pub(crate) id: String,
    pub(crate) author: String,
    pub(crate) text: String,
    pub(crate) created_at: Option<String>,
    pub(crate) branch: Option<String>,
}

/// Full details of one issue, from `bead show --json`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct IssueDetail {
    pub(crate) id: String,
    pub(crate) uid: String,
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) priority: Option<i64>,
    pub(crate) issue_type: String,
    pub(crate) description: Option<String>,
    pub(crate) acceptance_criteria: Option<String>,
    pub(crate) notes: Option<String>,
    pub(crate) design: Option<String>,
    pub(crate) external_ref: Option<String>,
    pub(crate) close_reason: Option<String>,
    pub(crate) assignee: Option<String>,
    pub(crate) labels: Vec<String>,
    pub(crate) parent: Option<String>,
    pub(crate) parent_uid: Option<String>,
    pub(crate) parents: Vec<String>,
    pub(crate) parent_uids: Vec<String>,
    pub(crate) blocked: bool,
    pub(crate) created_at: Option<String>,
    pub(crate) created_by: Option<String>,
    pub(crate) updated_at: Option<String>,
    pub(crate) closed_at: Option<String>,
    pub(crate) where_branch: Option<String>,
    pub(crate) where_commit: Option<String>,
    pub(crate) closed_on_branch: Option<String>,
    pub(crate) closed_on_commit: Option<String>,
    pub(crate) merged: Option<MergedStatus>,
    pub(crate) claim: Option<ClaimInfo>,
    pub(crate) provisional_claims: Vec<ClaimInfo>,
    pub(crate) unconfirmed: Vec<OwnershipNotice>,
    pub(crate) lost_claims: Vec<LostClaim>,
    pub(crate) stale: Vec<OwnershipNotice>,
    pub(crate) dependencies: Vec<RelatedIssue>,
    pub(crate) unresolved_dependencies: Vec<UnresolvedDep>,
    pub(crate) dependents: Vec<RelatedIssue>,
    pub(crate) comments: Vec<CommentItem>,
    pub(crate) conflicts: Option<Map<String, Value>>,
}

/// Complete response payload for `bead_show`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DetailView {
    pub(crate) workspace: WorkspaceInfo,
    pub(crate) issue: IssueDetail,
}

impl WorkspaceInfo {
    /// Decodes `bead info --json` output for `checkout`.
    pub(crate) fn from_info_json(value: &Value, checkout: String) -> Result<Self, AppError> {
        let obj = value
            .as_object()
            .ok_or_else(|| AppError::invalid_output("bead info", "expected a JSON object"))?;
        let id = non_empty_str(obj.get("workspace"))
            .ok_or_else(|| AppError::invalid_output("bead info", "missing workspace id"))?;
        Ok(Self {
            id,
            prefix: non_empty_str(obj.get("prefix")),
            name: non_empty_str(obj.get("name")),
            source: non_empty_str(obj.get("workspace_source")),
            last_sync: non_empty_str(obj.get("last_sync")),
            bead_version: non_empty_str(obj.get("version")),
            tracked_config_warning: non_empty_str(obj.get("tracked_config_warning")),
            checkout,
        })
    }

    /// Serializes workspace metadata for `structuredContent`.
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "prefix": self.prefix,
            "name": self.name,
            "source": self.source,
            "last_sync": self.last_sync,
            "bead_version": self.bead_version,
            "tracked_config_warning": self.tracked_config_warning,
            "checkout": self.checkout,
        })
    }

    /// Human-friendly label identifying this workspace (`name` or `prefix` or id).
    pub(crate) fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .or(self.prefix.as_deref())
            .unwrap_or(&self.id)
    }
}

impl ReadyIssue {
    /// Decodes one row of `bead ready --json`, rejecting older `bead` builds that omit `uid`.
    pub(crate) fn from_json(value: &Value, bead_version: Option<&str>) -> Result<Self, AppError> {
        let obj = value
            .as_object()
            .ok_or_else(|| AppError::invalid_output("bead ready", "expected issue object"))?;
        let id = non_empty_str(obj.get("id"))
            .ok_or_else(|| AppError::invalid_output("bead ready", "issue missing id"))?;
        let uid = non_empty_str(obj.get("uid"))
            .ok_or_else(|| AppError::upgrade_required(bead_version))?;
        Ok(Self {
            id,
            uid,
            title: string_or_empty(obj.get("title")),
            status: non_empty_str(obj.get("status")).unwrap_or_else(|| "open".to_owned()),
            priority: read_int(obj.get("priority")),
            issue_type: non_empty_str(obj.get("issue_type")).unwrap_or_else(|| "task".to_owned()),
            assignee: non_empty_str(obj.get("assignee")),
            labels: string_list(obj.get("labels")),
            branch: non_empty_str(obj.get("branch")),
            conflicted: string_list(obj.get("conflicted")),
            created_at: non_empty_str(obj.get("created_at")),
            updated_at: non_empty_str(obj.get("updated_at")),
        })
    }

    /// Serializes this row for `structuredContent`.
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "uid": self.uid,
            "title": self.title,
            "status": self.status,
            "priority": self.priority,
            "issue_type": self.issue_type,
            "assignee": self.assignee,
            "labels": self.labels,
            "branch": self.branch,
            "conflicted": self.conflicted,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
        })
    }
}

impl ReadyView {
    /// Serializes the ready-list view for `structuredContent`.
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "view": "ready",
            "workspace": self.workspace.to_json(),
            "limit": self.limit,
            "has_more": self.has_more,
            "issues": self.issues.iter().map(ReadyIssue::to_json).collect::<Vec<_>>(),
        })
    }
}

impl IssueDetail {
    /// Decodes the single issue object returned by `bead show --json`, rejecting older `bead`
    /// builds that omit `uid`.
    pub(crate) fn from_json(value: &Value, bead_version: Option<&str>) -> Result<Self, AppError> {
        let obj = value
            .as_object()
            .ok_or_else(|| AppError::invalid_output("bead show", "expected issue object"))?;
        let id = non_empty_str(obj.get("id"))
            .ok_or_else(|| AppError::invalid_output("bead show", "issue missing id"))?;
        let uid = non_empty_str(obj.get("uid"))
            .ok_or_else(|| AppError::upgrade_required(bead_version))?;
        let place = obj.get("where").and_then(Value::as_object);
        let closed_on = obj.get("closed_on").and_then(Value::as_object);
        Ok(Self {
            id,
            uid,
            title: string_or_empty(obj.get("title")),
            status: non_empty_str(obj.get("status")).unwrap_or_else(|| "open".to_owned()),
            priority: read_int(obj.get("priority")),
            issue_type: non_empty_str(obj.get("issue_type")).unwrap_or_else(|| "task".to_owned()),
            description: non_empty_str(obj.get("description")),
            acceptance_criteria: non_empty_str(obj.get("acceptance_criteria")),
            notes: non_empty_str(obj.get("notes")),
            design: non_empty_str(obj.get("design")),
            external_ref: non_empty_str(obj.get("external_ref")),
            close_reason: non_empty_str(obj.get("close_reason")),
            assignee: non_empty_str(obj.get("assignee")),
            labels: string_list(obj.get("labels")),
            parent: non_empty_str(obj.get("parent")),
            parent_uid: non_empty_str(obj.get("parent_uid")),
            parents: string_list(obj.get("parents")),
            parent_uids: string_list(obj.get("parent_uids")),
            blocked: obj.get("blocked").and_then(Value::as_bool).unwrap_or(false),
            created_at: non_empty_str(obj.get("created_at")),
            created_by: non_empty_str(obj.get("created_by")),
            updated_at: non_empty_str(obj.get("updated_at")),
            closed_at: non_empty_str(obj.get("closed_at")),
            where_branch: place.and_then(|p| non_empty_str(p.get("branch"))),
            where_commit: place.and_then(|p| non_empty_str(p.get("commit"))),
            closed_on_branch: closed_on.and_then(|p| non_empty_str(p.get("branch"))),
            closed_on_commit: closed_on.and_then(|p| non_empty_str(p.get("commit"))),
            merged: parse_merged(obj.get("merged")),
            claim: obj.get("claim").and_then(parse_claim),
            provisional_claims: array_items(obj.get("provisional_claims"))
                .filter_map(parse_claim)
                .collect(),
            unconfirmed: array_items(obj.get("unconfirmed"))
                .filter_map(parse_ownership_notice)
                .collect(),
            lost_claims: array_items(obj.get("lost_claims"))
                .filter_map(parse_lost_claim)
                .collect(),
            stale: array_items(obj.get("stale"))
                .filter_map(parse_ownership_notice)
                .collect(),
            dependencies: array_items(obj.get("dependencies"))
                .filter_map(parse_related)
                .collect(),
            unresolved_dependencies: array_items(obj.get("unresolved_dependencies"))
                .filter_map(parse_unresolved)
                .collect(),
            dependents: array_items(obj.get("dependents"))
                .filter_map(parse_related)
                .collect(),
            comments: array_items(obj.get("comments"))
                .filter_map(parse_comment)
                .collect(),
            conflicts: obj
                .get("conflicts")
                .and_then(Value::as_object)
                .filter(|map| !map.is_empty())
                .cloned(),
        })
    }

    /// Serializes full issue detail for `structuredContent`.
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "uid": self.uid,
            "title": self.title,
            "status": self.status,
            "priority": self.priority,
            "issue_type": self.issue_type,
            "description": self.description,
            "acceptance_criteria": self.acceptance_criteria,
            "notes": self.notes,
            "design": self.design,
            "external_ref": self.external_ref,
            "close_reason": self.close_reason,
            "assignee": self.assignee,
            "labels": self.labels,
            "parent": self.parent,
            "parent_uid": self.parent_uid,
            "parents": self.parents,
            "parent_uids": self.parent_uids,
            "blocked": self.blocked,
            "created_at": self.created_at,
            "created_by": self.created_by,
            "updated_at": self.updated_at,
            "closed_at": self.closed_at,
            "where_branch": self.where_branch,
            "where_commit": self.where_commit,
            "closed_on_branch": self.closed_on_branch,
            "closed_on_commit": self.closed_on_commit,
            "merged": self.merged.map(MergedStatus::to_json),
            "claim": self.claim.as_ref().map(ClaimInfo::to_json),
            "provisional_claims": self.provisional_claims.iter().map(ClaimInfo::to_json).collect::<Vec<_>>(),
            "unconfirmed": self.unconfirmed.iter().map(OwnershipNotice::to_json).collect::<Vec<_>>(),
            "lost_claims": self.lost_claims.iter().map(LostClaim::to_json).collect::<Vec<_>>(),
            "stale": self.stale.iter().map(OwnershipNotice::to_json).collect::<Vec<_>>(),
            "dependencies": self.dependencies.iter().map(RelatedIssue::to_json).collect::<Vec<_>>(),
            "unresolved_dependencies": self.unresolved_dependencies.iter().map(UnresolvedDep::to_json).collect::<Vec<_>>(),
            "dependents": self.dependents.iter().map(RelatedIssue::to_json).collect::<Vec<_>>(),
            "comments": self.comments.iter().map(CommentItem::to_json).collect::<Vec<_>>(),
            "conflicts": self.conflicts,
        })
    }
}

impl DetailView {
    /// Serializes the issue-detail view for `structuredContent`.
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "view": "detail",
            "workspace": self.workspace.to_json(),
            "issue": self.issue.to_json(),
        })
    }
}

impl ClaimInfo {
    fn to_json(&self) -> Value {
        json!({
            "owner": self.owner,
            "since": self.since,
            "generation": self.generation,
            "branch": self.branch,
        })
    }
}

impl OwnershipNotice {
    fn to_json(&self) -> Value {
        json!({
            "op": self.op,
            "by": self.by,
        })
    }
}

impl LostClaim {
    fn to_json(&self) -> Value {
        json!({
            "by": self.by,
            "held_by": self.held_by,
        })
    }
}

impl RelatedIssue {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "uid": self.uid,
            "title": self.title,
            "status": self.status,
            "dependency_type": self.dependency_type,
        })
    }
}

impl UnresolvedDep {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "dependency_type": self.dependency_type,
        })
    }
}

impl CommentItem {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "author": self.author,
            "text": self.text,
            "created_at": self.created_at,
            "branch": self.branch,
        })
    }
}

fn non_empty_str(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn string_or_empty(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_default()
}

fn read_int(value: Option<&Value>) -> Option<i64> {
    value.and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
    })
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    array_items(value)
        .filter_map(|item| non_empty_str(Some(item)))
        .collect()
}

fn array_items(value: Option<&Value>) -> impl Iterator<Item = &Value> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flat_map(|items| items.iter())
}

fn parse_merged(value: Option<&Value>) -> Option<MergedStatus> {
    match value {
        Some(Value::Bool(true)) => Some(MergedStatus::Merged),
        Some(Value::Bool(false)) => Some(MergedStatus::NotMerged),
        Some(Value::String(text)) if text == "unknown" => Some(MergedStatus::Unknown),
        _ => None,
    }
}

fn parse_claim(value: &Value) -> Option<ClaimInfo> {
    let obj = value.as_object()?;
    let owner = non_empty_str(obj.get("owner"))?;
    let branch = obj
        .get("where")
        .and_then(Value::as_object)
        .and_then(|place| non_empty_str(place.get("branch")));
    Some(ClaimInfo {
        owner,
        since: non_empty_str(obj.get("since")),
        generation: read_int(obj.get("generation")),
        branch,
    })
}

fn parse_ownership_notice(value: &Value) -> Option<OwnershipNotice> {
    let obj = value.as_object()?;
    let op = non_empty_str(obj.get("op"))?;
    let by = non_empty_str(obj.get("by")).unwrap_or_else(|| "unknown".to_owned());
    Some(OwnershipNotice { op, by })
}

fn parse_lost_claim(value: &Value) -> Option<LostClaim> {
    let obj = value.as_object()?;
    let by = non_empty_str(obj.get("by"))?;
    let held_by = non_empty_str(obj.get("held_by")).unwrap_or_else(|| "unknown".to_owned());
    Some(LostClaim { by, held_by })
}

fn parse_related(value: &Value) -> Option<RelatedIssue> {
    let obj = value.as_object()?;
    let id = non_empty_str(obj.get("id"))?;
    Some(RelatedIssue {
        id,
        uid: non_empty_str(obj.get("uid")),
        title: string_or_empty(obj.get("title")),
        status: non_empty_str(obj.get("status")).unwrap_or_else(|| "unknown".to_owned()),
        dependency_type: non_empty_str(obj.get("dependency_type"))
            .unwrap_or_else(|| "blocks".to_owned()),
    })
}

fn parse_unresolved(value: &Value) -> Option<UnresolvedDep> {
    let obj = value.as_object()?;
    let id = non_empty_str(obj.get("id"))?;
    Some(UnresolvedDep {
        id,
        dependency_type: non_empty_str(obj.get("dependency_type"))
            .unwrap_or_else(|| "blocks".to_owned()),
    })
}

fn parse_comment(value: &Value) -> Option<CommentItem> {
    let obj = value.as_object()?;
    Some(CommentItem {
        id: non_empty_str(obj.get("id")).unwrap_or_default(),
        author: non_empty_str(obj.get("author")).unwrap_or_else(|| "unknown".to_owned()),
        text: string_or_empty(obj.get("text")),
        created_at: non_empty_str(obj.get("created_at")),
        branch: non_empty_str(obj.get("branch")),
    })
}
