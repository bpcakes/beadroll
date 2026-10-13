//! Queries over state: conflicts, blockers, descendants and subtrees, readiness and waits-for
//! cycles.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::Hash;

use crate::event::{DONE, Field};
use crate::id::EventId;
use crate::json::{self, Format, Object, Value};
use crate::state::{IdMap, Issue, State, Texts};
use crate::time::event_time;

/// At most this many cycles are reported.
pub const MAX_CYCLES: usize = 200;

/// Whether this dependency type blocks its issue until the target is done.
pub fn blocking(kind: &str) -> bool {
    kind == "blocks"
}

/// One value of a disputed field.
#[derive(Clone, Debug, PartialEq)]
pub struct Alternative {
    /// The value (for `status`, the status alone).
    pub value: Value,
    /// Who wrote it.
    pub by: String,
    /// When.
    pub at: String,
    /// The event that wrote it.
    pub event: EventId,
}

impl Alternative {
    /// As a JSON object, in field order.
    pub fn to_value(&self) -> Object {
        let mut out = Object::new();
        out.insert("value".into(), self.value.clone());
        out.insert("by".into(), Value::from(self.by.as_str()));
        out.insert("at".into(), Value::from(self.at.as_str()));
        out.insert("event".into(), Value::from(self.event.to_string()));
        out
    }
}

/// Which edges a cycle follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edges {
    /// `blocks` dependencies, and parent to child (br's rule, used by `bead dep cycles`).
    WaitsFor,
    /// `blocks` dependencies only.
    Blocking,
}

/// Which issues readiness considers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadyScope {
    /// Every issue.
    All,
    /// The issues beneath one (its descendants through parent-child edges, itself excluded).
    Subtree(EventId),
}

impl State {
    /// Fields two writers changed without seeing each other, with every alternative, in the order
    /// the issue's registers were made.
    pub fn conflicts(&self, issue: &Issue) -> Vec<(Field, Vec<Alternative>)> {
        let mut out = Vec::new();
        for (field, register) in issue.registers() {
            if split(*field, register) {
                let alternatives = register
                    .iter()
                    .map(|(event, value)| Alternative {
                        value: shown(*field, value),
                        by: self.who(*event),
                        at: event_time(event.millis()),
                        event: *event,
                    })
                    .collect();
                out.push((*field, alternatives));
            }
        }
        out
    }

    /// The fields of `issue` two writers set differently ([`State::conflicts`]), in the order its
    /// registers were made; for an issue whose texts were not read, its long fields as its texts
    /// were when written, after the others (roll-lqjj.6).
    pub fn disputed_fields(&self, issue: &Issue) -> Vec<Field> {
        let mut out: Vec<Field> = (issue.registers().iter())
            .filter(|(field, register)| split(**field, register))
            .map(|(field, _)| *field)
            .collect();
        if let Texts::There(stored) = issue.texts() {
            out.extend(stored.disputed.iter().copied());
        }
        out
    }

    /// Whether a field of `issue` is disputed.
    pub fn disputed(&self, issue: &Issue, field: Field) -> bool {
        self.disputed_fields(issue).contains(&field)
    }

    /// Finished for the purposes of unblocking others. A disputed status does not count.
    pub fn is_done(&self, issue: &Issue) -> bool {
        DONE.contains(&issue.status()) && !self.disputed(issue, Field::Status)
    }

    /// Whether `start` already waits, directly or not, for `goal`.
    pub fn reaches(&self, start: EventId, goal: EventId) -> bool {
        let (mut stack, mut seen) = (vec![start], HashSet::new());
        while let Some(uid) = stack.pop() {
            if uid == goal {
                return true;
            }
            if !self.issues.contains_key(&uid) || !seen.insert(uid) {
                continue;
            }
            stack.extend(self.waits_for(uid));
        }
        false
    }

    /// The issues `issue` names as parents, by sorted edge.
    pub fn parents_of(&self, issue: &Issue) -> Vec<&Issue> {
        let mut edges: Vec<&(String, String)> = issue.deps().keys().collect();
        edges.sort();
        edges
            .into_iter()
            .filter(|(_, kind)| kind == "parent-child")
            .filter_map(|(to, _)| self.issue_named(to))
            .collect()
    }

    /// Its first parent.
    pub fn parent_of(&self, issue: &Issue) -> Option<&Issue> {
        self.parents_of(issue).into_iter().next()
    }

    /// Every issue beneath `uid`: its children, theirs, and so on, through every parent link and
    /// whatever the issues in between are. Only parent-child edges count, and `uid` itself is never
    /// among them, even when a merged cycle leads back.
    pub fn descendants(&self, uid: EventId) -> BTreeSet<EventId> {
        let mut found = BTreeSet::new();
        let mut stack: Vec<EventId> = self.children(uid).to_vec();
        while let Some(child) = stack.pop() {
            if child != uid && found.insert(child) {
                stack.extend(self.children(child).iter().copied());
            }
        }
        found
    }

    /// Each of `roots` and every issue beneath it, as [`State::descendants`] finds them, each once
    /// and after every issue beneath it: children in the order replay made them, which is the
    /// order of their creates (never their ids', which share a millisecond in random order), roots
    /// in the order given. A merged cycle is followed until it leads back to an issue already
    /// listed. The walk keeps its own stack, so a deep tree cannot overflow the thread's.
    pub fn subtrees_leaf_up(&self, roots: &[EventId]) -> Vec<EventId> {
        // the children still to visit, the last made first so that the first made pops first
        let pending = |uid: EventId| {
            let mut children = self.children(uid).to_vec();
            children.reverse();
            children
        };
        let mut listed = Vec::new();
        let mut seen = HashSet::new();
        for &root in roots {
            if !seen.insert(root) {
                continue;
            }
            let mut stack = vec![(root, pending(root))];
            while let Some((_, children)) = stack.last_mut() {
                match children.pop() {
                    Some(child) => {
                        if seen.insert(child) {
                            stack.push((child, pending(child)));
                        }
                    }
                    None => listed.extend(stack.pop().map(|(uid, _)| uid)),
                }
            }
        }
        listed
    }

    /// The `blocks` targets of `issue` that are issues and not done, by sorted edge.
    pub fn direct_blockers(&self, issue: &Issue) -> Vec<&Issue> {
        let mut edges: Vec<&(String, String)> = issue.deps().keys().collect();
        edges.sort();
        edges
            .into_iter()
            .filter(|(_, kind)| blocking(kind))
            .filter_map(|(to, _)| self.issue_named(to))
            .filter(|target| !self.is_done(target))
            .collect()
    }

    /// br's readiness rule: prerequisites block directly; an epic (only an epic) waits for its open
    /// children; and an ancestor held by a prerequisite holds its descendants. Sorted by key.
    pub fn open_blockers(&self, issue: &Issue) -> Vec<&Issue> {
        let mut found: HashMap<EventId, &Issue> = self
            .direct_blockers(issue)
            .into_iter()
            .map(|b| (b.uid(), b))
            .collect();
        if issue.issue_type() == "epic" {
            for child in self.children(issue.uid()) {
                if let Some(child) = self.issue(*child).filter(|child| !self.is_done(child)) {
                    found.insert(child.uid(), child);
                }
            }
        }
        let mut seen = HashSet::from([issue.uid()]);
        let mut queue = self.parents_of(issue);
        while let Some(parent) = queue.pop() {
            if seen.contains(&parent.uid()) || self.is_done(parent) {
                continue;
            }
            seen.insert(parent.uid());
            found.extend(
                self.direct_blockers(parent)
                    .into_iter()
                    .map(|b| (b.uid(), b)),
            );
            queue.extend(self.parents_of(parent));
        }
        let mut blockers: Vec<&Issue> = found.into_values().collect();
        blockers.sort_by(|a, b| a.key().cmp(b.key()));
        blockers
    }

    /// Whether anything holds `issue`.
    pub fn is_blocked(&self, issue: &Issue) -> bool {
        !self.open_blockers(issue).is_empty()
    }

    /// What `bead ready` lists: open issues in scope that nobody holds, nothing blocks and whose
    /// status is not disputed, by priority, then creation time, then key. Readiness is judged on the
    /// whole tracker, so a blocker outside the scope still holds an issue inside it.
    pub fn ready(&self, scope: ReadyScope) -> Vec<&Issue> {
        let beneath = match scope {
            ReadyScope::All => None,
            ReadyScope::Subtree(root) => Some(self.descendants(root)),
        };
        let mut items: Vec<&Issue> = self
            .issues()
            .filter(|issue| issue.status() == "open")
            .filter(|issue| {
                beneath
                    .as_ref()
                    .is_none_or(|beneath| beneath.contains(&issue.uid()))
            })
            .filter(|issue| issue.claim().is_none() && !self.is_blocked(issue))
            .filter(|issue| !self.disputed(issue, Field::Status))
            .collect();
        sort_for_listing(&mut items);
        items
    }

    /// Every elementary cycle of the graph (Johnson's algorithm), each written from its smallest
    /// member's key, at most [`MAX_CYCLES`] of them.
    pub fn cycles(&self, edges: Edges) -> Vec<Vec<String>> {
        let follow = |uid: &EventId| -> Vec<EventId> {
            match edges {
                Edges::WaitsFor => self.waits_for(*uid),
                Edges::Blocking => {
                    let Some(issue) = self.issue(*uid) else {
                        return Vec::new();
                    };
                    let mut targets: Vec<&(String, String)> = issue.deps().keys().collect();
                    targets.sort();
                    targets
                        .into_iter()
                        .filter(|(_, kind)| blocking(kind))
                        .filter_map(|(to, _)| {
                            EventId::parse(to).filter(|to| self.issues.contains_key(to))
                        })
                        .collect()
                }
            }
        };
        let mut nodes: Vec<EventId> = self.issues.keys().copied().collect();
        nodes.sort_unstable();
        find_cycles(&nodes, &follow)
            .into_iter()
            .map(|cycle| {
                cycle
                    .iter()
                    .map(|uid| {
                        self.issue(*uid)
                            .map_or_else(|| uid.to_string(), |i| i.key().to_owned())
                    })
                    .collect()
            })
            .collect()
    }
}

/// Sorts as `bead list` and `bead ready` list: by priority (`true` is 1), creation time, then key.
pub fn sort_for_listing(items: &mut [&Issue]) {
    let priority = |issue: &Issue| issue.priority();
    let created = |issue: &Issue| {
        issue
            .get(Field::CreatedAt)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    items.sort_by(|a, b| {
        priority(a)
            .cmp(&priority(b))
            .then_with(|| created(a).cmp(&created(b)))
            .then_with(|| a.key().cmp(b.key()))
    });
}

/// What a register of `field` shows of `value`: a status register's status, any other's value.
fn shown(field: Field, value: &Value) -> Value {
    if field == Field::Status {
        value.get("status").cloned().unwrap_or(Value::Null)
    } else {
        value.clone()
    }
}

/// Whether `register`, of `field`, holds values two writers left different, as conflicts compare
/// them: what a projection keeps of a long field whose texts it has not read (roll-lqjj.6).
pub(crate) fn split(field: Field, register: &IdMap<Value>) -> bool {
    register.len() > 1 && {
        let distinct: HashSet<String> = (register.values())
            .map(|value| dumps(&shown(field, value)))
            .collect();
        distinct.len() > 1
    }
}

/// Sorted-key JSON used to compare conflicting values.
fn dumps(value: &Value) -> String {
    json::write(value, Format::SORTED_ASCII).unwrap_or_else(|_| format!("{value:?}"))
}

/// Tarjan's strongly connected components, without recursion, over `nodes` in order: only the
/// components that can hold a cycle, each sorted.
pub fn strongly_connected<N, F>(nodes: &[N], edges: &F) -> Vec<Vec<N>>
where
    N: Clone + Ord + Hash,
    F: Fn(&N) -> Vec<N>,
{
    let mut index: HashMap<N, usize> = HashMap::new();
    let mut low: HashMap<N, usize> = HashMap::new();
    let (mut on_stack, mut stack, mut out) = (HashSet::new(), Vec::new(), Vec::new());
    for root in nodes {
        if index.contains_key(root) {
            continue;
        }
        let mut work = vec![(root.clone(), edges(root).into_iter())];
        index.insert(root.clone(), index.len());
        low.insert(root.clone(), index.len() - 1);
        stack.push(root.clone());
        on_stack.insert(root.clone());
        while let Some((node, children)) = work.last_mut() {
            let node = node.clone();
            if let Some(child) = children.next() {
                if !index.contains_key(&child) {
                    let at = index.len();
                    index.insert(child.clone(), at);
                    low.insert(child.clone(), at);
                    stack.push(child.clone());
                    on_stack.insert(child.clone());
                    let next = edges(&child).into_iter();
                    work.push((child, next));
                } else if on_stack.contains(&child) {
                    let reached = index.get(&child).copied().unwrap_or(usize::MAX);
                    if let Some(own) = low.get_mut(&node) {
                        *own = (*own).min(reached);
                    }
                }
                continue;
            }
            work.pop();
            let node_low = low.get(&node).copied().unwrap_or(usize::MAX);
            if let Some((parent, _)) = work.last()
                && let Some(parent_low) = low.get_mut(parent)
            {
                *parent_low = (*parent_low).min(node_low);
            }
            if Some(&node_low) == index.get(&node) {
                let mut component = Vec::new();
                while let Some(member) = stack.pop() {
                    on_stack.remove(&member);
                    let last = member == node;
                    component.push(member);
                    if last {
                        break;
                    }
                }
                if component.len() > 1 || edges(&node).contains(&node) {
                    component.sort();
                    out.push(component);
                }
            }
        }
    }
    out
}

/// Enumerates elementary cycles using iterative Johnson's algorithm. Each begins at its smallest
/// member; return at most [`MAX_CYCLES`] in deterministic traversal order.
pub fn find_cycles<N, F>(nodes: &[N], edges: &F) -> Vec<Vec<N>>
where
    N: Clone + Ord + Hash,
    F: Fn(&N) -> Vec<N>,
{
    let mut cycles: Vec<Vec<N>> = Vec::new();
    let mut components = strongly_connected(nodes, edges);
    while cycles.len() < MAX_CYCLES {
        let Some(component) = components.pop() else {
            break;
        };
        let mut component: BTreeSet<N> = component.into_iter().collect();
        let Some(start) = component.first().cloned() else {
            continue;
        };
        let inside = |uid: &N, component: &BTreeSet<N>| -> Vec<N> {
            edges(uid)
                .into_iter()
                .filter(|to| component.contains(to))
                .collect()
        };
        let mut path = vec![start.clone()];
        let mut blocked: HashSet<N> = HashSet::from([start.clone()]);
        let mut closed: HashSet<N> = HashSet::new();
        let mut unblock_later: HashMap<N, BTreeSet<N>> = HashMap::new();
        let mut work = vec![(start.clone(), inside(&start, &component).into_iter())];
        while cycles.len() < MAX_CYCLES {
            let Some((node, children)) = work.last_mut() else {
                break;
            };
            let node = node.clone();
            if let Some(child) = children.next() {
                if child == start {
                    cycles.push(path.clone());
                    closed.extend(path.iter().cloned());
                } else if !blocked.contains(&child) {
                    path.push(child.clone());
                    blocked.insert(child.clone());
                    closed.remove(&child);
                    let next = inside(&child, &component).into_iter();
                    work.push((child, next));
                }
                continue;
            }
            work.pop();
            path.pop();
            if closed.contains(&node) {
                // A cycle went through here: everything waiting on it may try again.
                let mut queue = vec![node];
                while let Some(unblocked) = queue.pop() {
                    if blocked.remove(&unblocked) {
                        queue.extend(unblock_later.remove(&unblocked).unwrap_or_default());
                    }
                }
            } else {
                for to in inside(&node, &component) {
                    unblock_later.entry(to).or_default().insert(node.clone());
                }
            }
        }
        // Cycles through `start` are all found; the rest live in what remains.
        component.remove(&start);
        let remaining: Vec<N> = component.iter().cloned().collect();
        let within = |uid: &N| inside(uid, &component);
        components.extend(strongly_connected(&remaining, &within));
    }
    cycles
}
