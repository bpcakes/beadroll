//! Independent lifetimes of a link's git processes. Fetch replaces the object writers;
//! repository replacement also ends the reader and ref writer. A publication owns its tree batch.

use std::path::Path;

use beadroll_git::{Env, Hasher, NoAnswer, Objects, Refs, Trees};

#[derive(Debug)]
pub(super) struct Helpers {
    pub objects: Objects,
    hasher: Restarting<Hasher>,
    prepared_trees: Option<Trees>,
    refs: Restarting<Refs>,
}

impl Helpers {
    pub(super) fn new(env: &Env) -> Self {
        Self {
            objects: Objects::new(env),
            hasher: Restarting::Needed,
            prepared_trees: None,
            refs: Restarting::Needed,
        }
    }

    /// Writers started before a fetch cannot see the pack it brings. The reader and ref writer
    /// look up objects afresh and remain usable across fetches.
    pub(super) fn before_fetch(&mut self) {
        self.hasher = Restarting::Needed;
        self.prepared_trees = None;
    }

    /// Warm the writers after a successful fetch, including one that found no published head.
    pub(super) fn after_fetch(&mut self, env: &Env) {
        self.hasher = Restarting::started(Hasher::start(env));
        self.prepared_trees = Trees::start(env).ok();
    }

    pub(super) fn stop(&mut self) {
        self.objects.stop();
        self.before_fetch();
        self.refs = Restarting::Needed;
    }

    pub(super) fn hash(&mut self, env: &Env, paths: &[&Path]) -> Result<Vec<String>, NoAnswer> {
        self.hasher
            .ask(env, Hasher::start, |hasher| hasher.hash(paths))
    }

    pub(super) fn hash_data(&mut self, env: &Env, data: &[u8]) -> Result<String, NoAnswer> {
        self.hasher.ask(env, Hasher::start, |hasher| {
            hasher.hash_data(data, env.tmp())
        })
    }

    /// Transfer the warmed batch to this publication; its exit drops it on success or failure.
    pub(super) fn trees(&mut self, env: &Env) -> TreeBatch {
        match self.prepared_trees.take() {
            Some(trees) => TreeBatch::Batch(trees),
            None => TreeBatch::start(env),
        }
    }

    pub(super) fn ready_refs(&mut self, env: &Env) {
        self.refs.prepare(env, Refs::start);
    }

    pub(super) fn transaction(&mut self, env: &Env, commands: &[String]) -> Result<(), NoAnswer> {
        self.refs.ask(env, Refs::start, |refs| refs.apply(commands))
    }
}

/// Hash and ref writers retry on the next request after a failed start or answer. This request
/// still returns NoAnswer so the caller can run its established standalone fallback.
#[derive(Debug)]
enum Restarting<T> {
    Needed,
    Running(T),
}

impl<T> Restarting<T> {
    fn started(result: Result<T, NoAnswer>) -> Self {
        match result {
            Ok(process) => Self::Running(process),
            Err(NoAnswer) => Self::Needed,
        }
    }

    fn prepare(&mut self, env: &Env, start: fn(&Env) -> Result<T, NoAnswer>) {
        if matches!(self, Self::Needed) {
            *self = Self::started(start(env));
        }
    }

    fn ask<R>(
        &mut self,
        env: &Env,
        start: fn(&Env) -> Result<T, NoAnswer>,
        request: impl FnOnce(&mut T) -> Result<R, NoAnswer>,
    ) -> Result<R, NoAnswer> {
        self.prepare(env, start);
        let answer = match self {
            Self::Running(process) => request(process),
            Self::Needed => Err(NoAnswer),
        };
        if answer.is_err() {
            *self = Self::Needed;
        }
        answer
    }
}

/// A failed tree batch stays on standalone mktree for the rest of this publication, unless the
/// tree rebuild after a failed reuse explicitly retries it. It does not restart for each tree.
pub(super) enum TreeBatch {
    Batch(Trees),
    Standalone,
}

impl TreeBatch {
    fn start(env: &Env) -> Self {
        match Trees::start(env) {
            Ok(trees) => Self::Batch(trees),
            Err(NoAnswer) => Self::Standalone,
        }
    }

    pub(super) fn retry(&mut self, env: &Env) {
        if matches!(self, Self::Standalone) {
            *self = Self::start(env);
        }
    }

    pub(super) fn make(&mut self, entries: &[u8]) -> Result<String, NoAnswer> {
        let answer = match self {
            Self::Batch(trees) => trees.make(entries),
            Self::Standalone => Err(NoAnswer),
        };
        if answer.is_err() {
            *self = Self::Standalone;
        }
        answer
    }
}
