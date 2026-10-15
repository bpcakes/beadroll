//! Every issue's blockers listed at once (roll-lqjj.24): the lists `bead blocked` prints, which
//! [`State::open_blockers`] builds for one issue by walking up its ancestors, so that listing them
//! for a chain of N issues held by its root walked N²/2 ancestors. Here what an issue inherits from
//! its ancestors is found once for every group of issues that reach each other up their parents
//! (one issue, or a merged cycle), each group after every group above it, and kept once: a group
//! that adds no blocker of its own to the one set it inherits shares that set, so the work grows
//! with the issues, their edges and the blockers listed, not with the depth of every ancestry.

use crate::id::EventId;
use crate::state::{Issue, State};

use super::blocking;

/// No place: an issue the walk has not reached, or one that is done and passes nothing on.
const NONE: u32 = u32::MAX;

/// The set of no blockers, which most issues share.
const EMPTY: u32 = 0;

/// `n` as a place (a state holds fewer than 2³² issues).
fn small(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(NONE)
}

/// Rows of places, each a slice of one vector.
#[derive(Clone, Debug)]
struct Rows {
    /// Where each row starts, then where the last one ends.
    starts: Vec<u32>,
    items: Vec<u32>,
}

impl Rows {
    /// The rows `fill` makes, one for each of `count` places: it is given the row's place and the
    /// vector to push the row's items onto.
    fn of(count: usize, mut fill: impl FnMut(usize, &mut Vec<u32>)) -> Rows {
        let mut rows = Rows {
            starts: Vec::with_capacity(count + 1),
            items: Vec::new(),
        };
        rows.starts.push(0);
        for at in 0..count {
            fill(at, &mut rows.items);
            rows.starts.push(small(rows.items.len()));
        }
        rows
    }

    /// Row `at`, empty when there is none.
    fn row(&self, at: u32) -> &[u32] {
        let at = at as usize;
        match (self.starts.get(at), self.starts.get(at + 1)) {
            (Some(&from), Some(&to)) => {
                (self.items.get(from as usize..to as usize)).unwrap_or_default()
            }
            _ => &[],
        }
    }
}

/// Every issue's blockers, found once for all of them, each issue's as [`State::open_blockers`]
/// lists it: [`State::blocker_lists`] makes it, and `bead blocked` asks it for the lists it prints.
#[derive(Clone, Debug)]
pub struct BlockerLists<'a> {
    state: &'a State,
    /// Whether each issue is done, by its place in the state.
    done: Vec<bool>,
    /// The places of the issues each issue names as parents.
    parents: Rows,
    /// The places of each issue's direct blockers, ascending.
    direct: Rows,
    /// For each issue that is not done, its set in `sets`: the direct blockers of every issue it
    /// reaches up its parents through issues that are not done, itself included. [`NONE`] for an
    /// issue that is done, which nothing reaches through.
    inherited: Vec<u32>,
    /// Sets of places, ascending, each kept once however many issues share it; the first is
    /// [`EMPTY`].
    sets: Vec<Box<[u32]>>,
}

impl<'a> BlockerLists<'a> {
    /// The issues blocking `issue`, sorted by key: what [`State::open_blockers`] lists for it. An
    /// issue that is not the state's is listed on its own.
    pub fn of(&self, issue: &Issue) -> Vec<&'a Issue> {
        let Some(at) = self.state.issues.get_index_of(&issue.uid()).map(small) else {
            return self.state.open_blockers(issue);
        };
        let mut found: Vec<u32> = match self.inherited.get(at as usize) {
            Some(&set) if set != NONE => self.set(set).to_vec(),
            // a done issue is not passed through, but its own walk starts from it: its direct
            // blockers, and what each of its parents that is not done inherits
            _ => {
                let mut found = self.direct.row(at).to_vec();
                for &parent in self.parents.row(at) {
                    if let Some(&set) = self.inherited.get(parent as usize) {
                        found.extend_from_slice(self.set(set));
                    }
                }
                found
            }
        };
        if issue.issue_type() == "epic" {
            // only an epic waits for its children, and nothing beneath it inherits that wait
            let children = self.state.children(issue.uid()).iter();
            let places = children.filter_map(|child| self.state.issues.get_index_of(child));
            found.extend(places.map(small).filter(|&child| !self.is_done(child)));
        }
        found.sort_unstable();
        found.dedup();
        let state = self.state;
        let mut listed: Vec<&'a Issue> = (found.iter())
            .filter_map(|&at| state.issues.get_index(at as usize))
            .map(|(_, issue)| &**issue)
            .collect();
        listed.sort_by(|a, b| a.key().cmp(b.key()));
        listed
    }

    fn is_done(&self, at: u32) -> bool {
        self.done.get(at as usize).copied().unwrap_or(false)
    }

    /// Set `at`, empty when it is [`NONE`].
    fn set(&self, at: u32) -> &[u32] {
        self.sets
            .get(at as usize)
            .map(|set| &set[..])
            .unwrap_or_default()
    }
}

impl State {
    /// Every issue's blockers, found once for all of them (roll-lqjj.24): [`BlockerLists::of`]
    /// lists an issue's as [`State::open_blockers`] does. The issues that are not done are grouped
    /// as they reach each other up their parents through issues that are not done (Tarjan's
    /// strongly connected components, without recursion), each group found after every group above
    /// it; a group's set is its members' direct blockers and the sets of the groups above it, the
    /// set of the one group above it when it adds nothing. An issue that is done is not passed
    /// through: it has its own direct blockers and its parents' sets, as its walk would find them.
    pub fn blocker_lists(&self) -> BlockerLists<'_> {
        let count = self.issues.len();
        let place = |to: &str| EventId::parse(to).and_then(|uid| self.issues.get_index_of(&uid));
        let done: Vec<bool> = self.issues().map(|issue| self.is_done(issue)).collect();
        let edges = |at: usize| {
            (self.issues.get_index(at).into_iter()).flat_map(|(_, issue)| issue.deps().keys())
        };
        let parents = Rows::of(count, |at, row| {
            let named = edges(at).filter(|(_, kind)| kind == "parent-child");
            row.extend(named.filter_map(|(to, _)| place(to)).map(small));
        });
        let direct = Rows::of(count, |at, row| {
            let from = row.len();
            let named = edges(at).filter(|(_, kind)| blocking(kind));
            let targets = named.filter_map(|(to, _)| place(to));
            row.extend(
                targets
                    .filter(|&to| !done.get(to).copied().unwrap_or(false))
                    .map(small),
            );
            if let Some(added) = row.get_mut(from..) {
                added.sort_unstable();
            }
        });
        let (inherited, sets) = Groups::new(&done, &parents, &direct).walk();
        BlockerLists {
            state: self,
            done,
            parents,
            direct,
            inherited,
            sets,
        }
    }
}

/// The walk that groups the issues that are not done by what they reach up their parents.
struct Groups<'r> {
    done: &'r [bool],
    parents: &'r Rows,
    direct: &'r Rows,
    /// Each issue's number in the order the walk reached it; [`NONE`] until it does.
    reached: Vec<u32>,
    /// The least number each issue's walk led back to while its group was open.
    low: Vec<u32>,
    /// Whether each issue is in a group not yet closed.
    open: Vec<bool>,
    /// The issues of the groups not yet closed, in the order reached.
    stack: Vec<u32>,
    next: u32,
    inherited: Vec<u32>,
    sets: Vec<Box<[u32]>>,
}

impl<'r> Groups<'r> {
    fn new(done: &'r [bool], parents: &'r Rows, direct: &'r Rows) -> Groups<'r> {
        let count = done.len();
        Groups {
            done,
            parents,
            direct,
            reached: vec![NONE; count],
            low: vec![NONE; count],
            open: vec![false; count],
            stack: Vec::new(),
            next: 0,
            inherited: vec![NONE; count],
            sets: vec![Box::new([])],
        }
    }

    /// Every issue's set, and the sets.
    fn walk(mut self) -> (Vec<u32>, Vec<Box<[u32]>>) {
        for root in 0..small(self.done.len()) {
            if !self.is_done(root) && self.number(root) == NONE {
                self.from(root);
            }
        }
        (self.inherited, self.sets)
    }

    fn is_done(&self, at: u32) -> bool {
        self.done.get(at as usize).copied().unwrap_or(true)
    }

    fn number(&self, at: u32) -> u32 {
        self.reached.get(at as usize).copied().unwrap_or(NONE)
    }

    fn least(&self, at: u32) -> u32 {
        self.low.get(at as usize).copied().unwrap_or(NONE)
    }

    /// Lowers what the walk from `at` led back to, to `to`.
    fn lower(&mut self, at: u32, to: u32) {
        if let Some(low) = self.low.get_mut(at as usize) {
            *low = (*low).min(to);
        }
    }

    /// Reaches `at`: numbered, and open.
    fn enter(&mut self, at: u32) {
        let number = self.next;
        self.next += 1;
        if let (Some(reached), Some(low), Some(open)) = (
            self.reached.get_mut(at as usize),
            self.low.get_mut(at as usize),
            self.open.get_mut(at as usize),
        ) {
            (*reached, *low, *open) = (number, number, true);
        }
        self.stack.push(at);
    }

    /// Walks up from `root`, closing every group found on the way, each after the groups above it.
    fn from(&mut self, root: u32) {
        // each issue on the way, and how many of its parents have been followed
        let mut work: Vec<(u32, usize)> = vec![(root, 0)];
        self.enter(root);
        while let Some(&(at, next)) = work.last() {
            if let Some(&parent) = self.parents.row(at).get(next) {
                if let Some(top) = work.last_mut() {
                    top.1 += 1;
                }
                if self.is_done(parent) {
                    continue;
                }
                if self.number(parent) == NONE {
                    self.enter(parent);
                    work.push((parent, 0));
                } else if self.open.get(parent as usize).copied().unwrap_or(false) {
                    self.lower(at, self.number(parent));
                }
                continue;
            }
            work.pop();
            if let Some(&(below, _)) = work.last() {
                self.lower(below, self.least(at));
            }
            if self.least(at) == self.number(at) {
                self.close(at);
            }
        }
    }

    /// Closes the group whose first issue reached is `first`: its members share one set.
    fn close(&mut self, first: u32) {
        let mut members = Vec::new();
        while let Some(member) = self.stack.pop() {
            if let Some(open) = self.open.get_mut(member as usize) {
                *open = false;
            }
            members.push(member);
            if member == first {
                break;
            }
        }
        let set = self.combine(&members);
        for member in members {
            if let Some(inherited) = self.inherited.get_mut(member as usize) {
                *inherited = set;
            }
        }
    }

    /// The set of a group: its members' direct blockers and the sets of the groups above it, each
    /// closed already, as every group a member reaches is closed before the member's own. The set
    /// of the one group above when the group adds nothing, and any set above that holds the whole
    /// union, is shared rather than copied.
    fn combine(&mut self, members: &[u32]) -> u32 {
        let mut own = false;
        let mut above: Vec<u32> = Vec::new();
        for &member in members {
            own |= !self.direct.row(member).is_empty();
            for &parent in self.parents.row(member) {
                // a member's own group has no set yet, and an issue that is done none at all
                if let Some(&set) = self.inherited.get(parent as usize).filter(|s| **s != NONE) {
                    above.push(set);
                }
            }
        }
        above.sort_unstable();
        above.dedup();
        match (own, above.as_slice()) {
            (false, []) => return EMPTY,
            (false, [only]) => return *only,
            _ => {}
        }
        let set = |at: u32| {
            self.sets
                .get(at as usize)
                .map(|set| &set[..])
                .unwrap_or_default()
        };
        let mut union: Vec<u32> = Vec::new();
        for &member in members {
            union.extend_from_slice(self.direct.row(member));
        }
        for &at in &above {
            union.extend_from_slice(set(at));
        }
        union.sort_unstable();
        union.dedup();
        // a set above is part of the union: one as long as it is the union itself
        if let Some(&whole) = (above.iter()).find(|&&at| set(at).len() == union.len()) {
            return whole;
        }
        self.sets.push(union.into_boxed_slice());
        small(self.sets.len() - 1)
    }
}
