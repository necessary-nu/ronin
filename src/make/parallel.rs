//! Threads that read a recursive child's Makefile while the composition walks.
//!
//! # Why the threads own a working directory
//!
//! Kati resolves a relative name against the process working directory: a child
//! invocation's Makefile is named `Makefile`, not `sub/Makefile`, and
//! `$(wildcard)`, `$(realpath)`, `CURDIR`, the find emulator and every
//! existence test the implicit-rule search makes are all read from wherever the
//! process happens to be standing. [`crate::make::in_directory`] is what stands
//! it in the right place, and it is a process-wide `chdir`, so two units read at
//! once would each be reading against the other's directory.
//!
//! A worker calls `unshare(CLONE_FS)` before it takes any work. That gives the
//! thread its own copy of the kernel's filesystem context — its own working
//! directory — so `chdir` on a worker moves that worker and nothing else, and
//! the directory the process is standing in never moves at all. A `$(shell)`
//! forked from a worker inherits the forking thread's directory, which is the
//! one its Makefile was read from, so a command a read runs still runs where
//! GNU Make would have run it.
//!
//! This is the whole reason the reads may overlap. It is also why the pool is
//! Linux-only: `CLONE_FS` is a Linux clone flag, and on a platform without it
//! there is no per-thread working directory to have, so
//! [`ReadPool::available`] answers false and every read stays on the calling
//! thread.

use std::sync::mpsc;

/// One unit of work handed to a worker, already holding everything it needs.
type Read = Box<dyn FnOnce() + Send + 'static>;

/// How many Makefile reads have been handed to a worker.
///
/// Test-only, and it exists to keep a test honest rather than to be read by
/// anything that runs: whether a read may start early depends on the shape of
/// the unit and on how the invocation collects its diagnostics, so a test that
/// meant to exercise the workers can stop exercising them without failing. This
/// is what lets it assert that it did.
#[cfg(test)]
pub(crate) static READS_STARTED: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Give this thread its own working directory, root and umask.
///
/// `unshare` is deprecated in rustix in favour of an `unsafe` spelling, and the
/// safety condition it carries is about `UnshareFlags::FILES` alone: unsharing
/// the *descriptor* table lets one thread hold a descriptor another thread's
/// table does not have. Only `FS` is passed here, which copies the filesystem
/// context — working directory, root, umask — and leaves the descriptor table
/// shared, so no descriptor changes hands and the condition cannot be reached.
#[cfg(target_os = "linux")]
fn unshare_filesystem_context() -> rustix::io::Result<()> {
    // SAFETY: `FS` only, never `FILES`, so the descriptor table stays shared
    // and the hazard the safety contract names does not arise.
    unsafe { rustix::thread::unshare_unsafe(rustix::thread::UnshareFlags::FS) }
}

/// Worker threads that read child Makefiles, each standing in its own
/// directory.
pub(crate) struct ReadPool {
    /// `None` once the pool is being dropped, which closes the queue and is
    /// what tells the workers to finish.
    queue: Option<mpsc::Sender<Read>>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl ReadPool {
    /// A pool of up to `threads` workers, or `None` where a read may not leave
    /// the calling thread.
    ///
    /// `None` rather than a pool of one, so that a caller that cannot use
    /// workers takes the same path it took before this existed rather than a
    /// path that merely behaves like it.
    ///
    /// `threads` is a job count, and a job count can say "as many as it takes":
    /// both `-j` with no number and no `-j` at all reach here as
    /// [`usize::MAX`]. That is a limit on the processes a build may run, and
    /// this is not that — it is how many Makefiles may be read at once, which
    /// the machine bounds rather than the switch. Clamping is therefore load
    /// bearing rather than tidy: unclamped, this would ask for `usize::MAX`
    /// threads.
    pub(crate) fn new(threads: usize) -> Option<Self> {
        let threads = threads.min(crate::os::cores());
        if threads < 2 || !Self::available() {
            return None;
        }
        let (sender, receiver) = mpsc::channel::<Read>();
        let receiver = std::sync::Arc::new(std::sync::Mutex::new(receiver));
        let mut workers: Vec<std::thread::JoinHandle<()>> = Vec::with_capacity(threads);
        for _ in 0..threads {
            let receiver = std::sync::Arc::clone(&receiver);
            let Ok(worker) = std::thread::Builder::new()
                .name("ronin-make-read".to_owned())
                .spawn(move || Self::serve(&receiver))
            else {
                // A pool that could not be staffed is not a smaller pool: the
                // workers already spawned are told to finish by dropping the
                // queue, and the caller reads on its own thread.
                drop(sender);
                for worker in workers {
                    let _: std::thread::Result<()> = worker.join();
                }
                return None;
            };
            workers.push(worker);
        }
        Some(Self {
            queue: Some(sender),
            workers,
        })
    }

    /// Whether a thread on this platform can be given a working directory of
    /// its own.
    #[cfg(target_os = "linux")]
    fn available() -> bool {
        // Asked by unsharing on a thread of its own, which is the only honest
        // answer: a sandbox may refuse the call, and a pool whose workers
        // cannot unshare would read every unit against one directory. The
        // thread is spawned for the question alone, so the unshare it leaves
        // behind reaches nothing.
        std::thread::spawn(|| unshare_filesystem_context().is_ok())
            .join()
            .unwrap_or(false)
    }

    #[cfg(not(target_os = "linux"))]
    fn available() -> bool {
        false
    }

    /// Take work until the queue closes, standing in a directory of this
    /// thread's own.
    fn serve(receiver: &std::sync::Mutex<mpsc::Receiver<Read>>) {
        #[cfg(target_os = "linux")]
        if unshare_filesystem_context().is_err() {
            // Without a directory of its own this thread would read against
            // whatever directory another one moved to. It takes no work at
            // all; the submitting side sees the answer never arrive and reads
            // the unit itself.
            return;
        }
        loop {
            let work = {
                let Ok(receiver) = receiver.lock() else {
                    return;
                };
                receiver.recv()
            };
            let Ok(work) = work else {
                return;
            };
            work();
        }
    }

    /// Start `read` on a worker, answering on the returned receiver.
    ///
    /// The answer is a channel rather than a shared slot because the caller
    /// consumes the reads in the order the recipe wrote them, not the order
    /// they finish: it waits for the one it has reached and lets the rest go on
    /// arriving behind it.
    pub(crate) fn start<T, F>(&self, read: F) -> mpsc::Receiver<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel();
        if let Some(queue) = &self.queue {
            let _: Result<(), mpsc::SendError<Read>> = queue.send(Box::new(move || {
                let _: Result<(), mpsc::SendError<T>> = sender.send(read());
            }));
        }
        receiver
    }
}

impl Drop for ReadPool {
    fn drop(&mut self) {
        // Closing the queue is what ends the workers: a worker blocked on an
        // empty queue wakes when the last sender goes, and one part way through
        // a read finishes it first. Joined rather than detached so that no
        // worker is still standing in a directory, or still holding a session,
        // after the compilation that owns them has returned.
        self.queue = None;
        for worker in self.workers.drain(..) {
            let _: std::thread::Result<()> = worker.join();
        }
    }
}

// ---------------------------------------------------------------------------
// Reading ahead
// ---------------------------------------------------------------------------

use super::{
    Compilation, CompilationContext, CompilationState, Evaluated, MakeError, ReadJournals, Session,
    evaluate, in_directory, sink,
};
use std::collections::HashSet;

/// Read one unit's Makefiles, which is the half of the read that reaches no
/// graph.
///
/// Split from [`super::read_unit`] because it is at once the expensive half and the
/// only half that can leave this thread: it is handed a session and gives back
/// what the Makefiles said, without ever touching the one [`super::GraphSink`] every
/// unit shares. Everything downstream of it — emitting the build, taking the
/// unit, the layout — writes to that sink and stays where the sink is. See
/// [`ReadPool`].
///
/// The directory is entered here rather than around both halves because
/// entering it is what a worker does on its own behalf: kati reads relative
/// names against the working directory, and on a worker that directory is the
/// worker's own.
pub(super) fn evaluate_unit(
    session: Session,
    directory: &std::path::Path,
) -> Result<Evaluated, MakeError> {
    in_directory(directory, || {
        evaluate(session).map_err(|error| MakeError::evaluate(&error))
    })
}

/// Tell one unit's session everything it is told before it reads.
///
/// Factored out of [`super::compile_unit`] because a read that happens on a worker is
/// prepared here, on the thread that owns the compilation state, and then sent:
/// what a session is told must not depend on which thread ends up reading it.
pub(super) fn prepare_session(compilation: &mut Compilation, read_units: &ReadJournals) {
    let shuffle = compilation.shuffle;
    let interrupts = std::sync::Arc::clone(&compilation.context.interrupts);
    let replayed = read_units.get(&compilation.cache_key);
    let session = &mut compilation.session;
    // `--shuffle` is Make's own reordering rather than this frontend's: the
    // walk that drops circular prerequisites reads the order it chose, so the
    // evaluator has to be told before it plans.
    session.flags.shuffle = shuffle;
    // A `$(shell)` in this unit's makefile and a recipe line in the graph it
    // compiles to are the same language, so the read uses the shell the build
    // will use. Per unit, because a recursive child reads with its own session
    // and must not read with a different shell than its parent.
    // [spec:ronin:req:product.builtin-shell]
    session.flags.default_shell_program =
        crate::subprocess::builtin_shell().map(std::path::Path::to_path_buf);
    // Per unit for the same reason, and the same watch in every one of them: a
    // recursive child reading its own makefile is stopped by the interrupt its
    // parent was sent.
    session.interrupts = Some(interrupts);
    // Per unit rather than per pass, because a staging pass reads units it has
    // read before AND one it has not: the parent and the children behind the
    // settled boundaries are repeating themselves, while the child the pass
    // was taken for is speaking for the first time.
    session.flags.is_repeated_read = replayed.is_some();
    // And what that first read was TOLD, for the calls that cannot be held
    // back because the expansion that asked has to be handed a value. A unit
    // reading for the first time — the child this pass was taken for — gets
    // nothing and asks the ground itself.
    session.ground_journal.replay(
        replayed
            .map(|journal| journal.ground.clone())
            .unwrap_or_default(),
    );
    // And the text it READ, which is the other half of the same premise. A
    // makefile a staged child has since written was not part of the first read
    // and must not become part of this one; one it has rewritten is still the
    // text the first read had.
    for (name, contents) in replayed.map_or(&[][..], |journal| &journal.sources) {
        session.supply_makefile(name.clone(), contents.clone());
    }
}

/// A child compilation the composition is about to make, and its Makefiles
/// either still to be read or already read on a worker.
///
/// Three cases rather than an `Option`, because resolving the invocation is
/// itself something that can happen early and can fail: a read started ahead of
/// time has already resolved, and a resolution that refused must hand the
/// composition the same refusal at the same place rather than be tried again.
/// Resolving twice is not free — an invocation that Make would answer
/// immediately is answered by running it.
pub(super) enum ChildUnit {
    /// Resolved where the composition reached it, and not yet read.
    Unread(Box<Compilation>),
    /// Resolved earlier, and read on a worker since.
    Read(Box<ChildRead>),
    /// Resolving it refused, held until the composition reaches the invocation
    /// it refused for.
    Refused(MakeError),
}

/// One recursive child's Makefiles, read on a worker before the composition
/// reached the recipe that asks for them.
pub(super) struct ChildRead {
    pub(super) cache_key: Vec<u8>,
    pub(super) context: CompilationContext,
    /// Where the worker answers.
    answer: std::sync::mpsc::Receiver<Option<Result<Evaluated, MakeError>>>,
    /// What the read raised, held apart from the invocation's own descriptor
    /// so that two reads at once cannot interleave their warnings.
    raised: std::sync::Arc<kati::diagnostics::Diagnostics>,
    /// The session, until whichever of the worker and this thread reads it
    /// takes it.
    ///
    /// Shared rather than moved into the worker so that a read no worker
    /// performed can still be performed here: exactly one side takes the
    /// session out, so the Makefiles are read exactly once either way.
    unread: std::sync::Arc<std::sync::Mutex<Option<Session>>>,
}

impl ChildRead {
    /// Wait for the read, and put what it raised where the invocation collects
    /// it.
    ///
    /// The warnings are drained here, at the moment the composition reaches
    /// this child, rather than as they were raised: that is what puts them back
    /// in the order the recipes were written in. A Makefile read early on a
    /// worker still belongs behind everything the recipes before it said, which
    /// is where GNU Make would have put it.
    pub(super) fn collect(self) -> Result<Evaluated, MakeError> {
        let evaluated = if let Ok(Some(evaluated)) = self.answer.recv() {
            evaluated
        } else {
            // No worker read it. The session was left where either side could
            // take it for exactly this, so the read happens here instead.
            let unread = self
                .unread
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            match unread {
                Some(session) => evaluate_unit(session, &self.context.directory),
                None => Err(MakeError::Evaluate(
                    "reading a recursive Make child was abandoned".to_owned(),
                )),
            }
        };
        self.context.diagnostics.absorb(&self.raised);
        evaluated.map(|mut evaluated| {
            // From here the session speaks for itself again: anything it says
            // while a recipe of it is expanded is said in the order the build
            // reaches it, and belongs in the invocation's own descriptor.
            evaluated.ev.session.diagnostics = std::sync::Arc::clone(&self.context.diagnostics);
            evaluated
        })
    }
}

impl ChildUnit {
    /// What this child compilation is cached under.
    pub(super) fn cache_key(&self) -> &[u8] {
        match self {
            Self::Unread(compilation) => &compilation.cache_key,
            Self::Read(read) => &read.cache_key,
            // A refusal is answered where the composition reaches it, which is
            // ahead of anything that would ask what it is cached under.
            Self::Refused(_) => &[],
        }
    }
}

/// Start reading the Makefiles of this unit's later recursive recipes, so they
/// are read while its earlier ones are composed.
///
/// # What may be read early, and why it is only ever a read
///
/// Composition is not moved. One graph is built by one thread in the order it
/// was built in before, so every node, every edge and every invented name
/// falls where it fell — the graph does not depend on what the workers did or
/// when. Only the Makefile read moves, which is both the expensive half and the
/// half that reaches no graph at all. See [`evaluate_unit`].
///
/// A read may only start early if the composition is certain to reach it,
/// because reading a Makefile runs its `$(shell)` calls and those are not
/// free to happen speculatively. Every recipe of the unit must therefore be one
/// that cannot stop the composition short and cannot be skipped:
///
/// * `always_dirty` — the recipe's wrapper is out of date whatever the disk
///   says, so the child is composed rather than short-circuited to a phony.
///   This is `.PHONY`, which is what a recursive dispatch rule is.
/// * one invocation, with no recipe lines ahead of it and no prerequisites of
///   its own — so nothing is staged, and staging is the only thing that stops a
///   composition at a boundary and leaves the later recipes unreached.
/// * no shell command substitution in the invocation — resolving one runs a
///   command, and resolving early would run it early.
///
/// The whole unit has to qualify, not one recipe of it: a recipe that could
/// stop the composition would leave every recipe after it unreached, and a read
/// started for one of those would be a read GNU Make never did.
///
/// # What reading early can still see differently
///
/// Two things, both of which need `-j` greater than one to happen at all, and
/// both of which are what GNU Make does there too: at `-j8` it has several
/// recursive children running at once, reading their own Makefiles and running
/// their own `$(shell)` calls against a tree its siblings are still changing.
/// At `-j1` there is no pool, nothing starts early, and neither arises.
///
/// A descendant of an earlier recipe can stop the composition at a boundary,
/// and then a read started here was one this pass did not need. The pass that
/// follows reads it anyway, so no Makefile is read that this invocation was not
/// going to read — it is read a pass earlier than it would have been.
///
/// And a read that runs early sees the tree as it is early. Resolving an
/// invocation looks for the child's default Makefile on disk, and the read
/// itself runs the Makefile's `$(shell)` calls; either could see a file that an
/// earlier recipe's own read was going to write and has not written yet. What
/// the graph is built from still reaches the sink in recipe order, so the graph
/// does not move — but which bytes a Makefile was read from can, exactly as it
/// can for GNU Make's concurrent children.
pub(super) fn read_ahead<F>(
    ordered: &[sink::PendingSubninja],
    resolve: &mut F,
    descendant_context: &CompilationContext,
    state: &CompilationState<'_>,
) -> Vec<Option<ChildUnit>>
where
    F: FnMut(&[u8], &[u8], &[u8], &[u8], &CompilationContext) -> Result<Compilation, MakeError>,
{
    // A run with no workers to read on, or a unit with nothing to overlap, is
    // answered on an integer comparison. Asked first, and before the shape of
    // the unit is examined at all, because this is the whole of what a `-j1`
    // run pays for the existence of read-ahead: at `-j1` there is no second
    // thread to read on, and a serial run must not pay even the cost of being
    // asked whether it could have used one.
    if state.read_threads < 2 || ordered.len() < 2 {
        return Vec::new();
    }
    if !reads_may_start_early(ordered, descendant_context) {
        return Vec::new();
    }
    // Started only now, so a compilation that never reads ahead never starts a
    // worker.
    let Some(pool) = state
        .read_pool
        .get_or_init(|| ReadPool::new(state.read_threads))
    else {
        return Vec::new();
    };
    // Every recipe is resolved here, in the order the recipes were written,
    // which is the order they were resolved in before. Resolving is not silent
    // — it is where an invocation's own switches are parsed, and parsing one
    // can raise a warning — so the recipe that is not read early is resolved
    // early all the same rather than resolved out of turn later.
    //
    // A unit two of these recipes both invoke is compiled once and taken from
    // the cache the second time. Reading it twice would run its `$(shell)`
    // calls twice, so the second recipe is left to find it where the first put
    // it.
    let mut started = Vec::with_capacity(ordered.len());
    let mut claimed = HashSet::new();
    for (index, pending) in ordered.iter().enumerate() {
        let invocation = &pending.invocations[0];
        started.push(Some(
            match resolve(
                &invocation.command,
                &invocation.make,
                &invocation.shell,
                &invocation.shell_flags,
                descendant_context,
            ) {
                Err(refusal) => ChildUnit::Refused(refusal),
                Ok(mut compilation) => {
                    let claimable = !state.cache.contains_key(&compilation.cache_key)
                        && claimed.insert(compilation.cache_key.clone());
                    // The first recipe is read where it always was: the
                    // composition asks for it next, so handing it to a worker
                    // would add a handoff to a wait that buys nothing.
                    if index == 0 || !claimable {
                        ChildUnit::Unread(Box::new(compilation))
                    } else {
                        prepare_session(&mut compilation, state.read_units);
                        ChildUnit::Read(Box::new(start_read(pool, compilation)))
                    }
                }
            },
        ));
    }
    started
}

/// Hand one prepared child compilation to a worker.
fn start_read(pool: &ReadPool, compilation: Compilation) -> ChildRead {
    #[cfg(test)]
    READS_STARTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let Compilation {
        mut session,
        context,
        cache_key,
        ..
    } = compilation;
    // Its own descriptor for the length of the read. Two reads at once would
    // otherwise append to the invocation's in whatever order they got there,
    // and what a read says is drained into that one when the composition
    // reaches this child. See [`ChildRead::collect`].
    let raised = std::sync::Arc::new(kati::diagnostics::Diagnostics::collected());
    session.diagnostics = std::sync::Arc::clone(&raised);
    let unread = std::sync::Arc::new(std::sync::Mutex::new(Some(session)));
    let held = std::sync::Arc::clone(&unread);
    let directory = context.directory.clone();
    let answer = pool.start(move || {
        let session = held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        session.map(|session| evaluate_unit(session, &directory))
    });
    ChildRead {
        cache_key,
        context,
        answer,
        raised,
        unread,
    }
}

/// Whether every recursive recipe of one unit is one a read may start early
/// for. See [`read_ahead`].
fn reads_may_start_early(ordered: &[sink::PendingSubninja], context: &CompilationContext) -> bool {
    // A report classifies invocations as it reaches them and tolerates a child
    // it cannot read; neither is worth teaching to happen out of order for a
    // path that is not the one under time pressure.
    if context.reporting || context.census.is_recording() {
        return false;
    }
    // A descriptor that writes each warning through as it is raised has already
    // put it wherever the threads happened to put it. Only a held one can be
    // drained back into the order the recipes were written in.
    if !context.diagnostics.is_collecting() {
        return false;
    }
    ordered.iter().all(|pending| {
        pending.always_dirty
            && pending.invocations.len() == 1
            && pending.invocations[0].preceding_rule.is_none()
            && pending.evaluation_inputs().is_empty()
            && !runs_a_command_to_resolve(&pending.invocations[0].command)
    })
}

/// Whether resolving this invocation would have to run a command.
///
/// The bytes here have already been through Make's own expansion, so a `$(`,
/// `${` or backtick left in them is the shell's substitution rather than
/// Make's, and resolving the invocation would run it. Asked textually and
/// answered conservatively: a command that merely looks like one is read where
/// it always was, which costs a little parallelism and never a wrong answer.
fn runs_a_command_to_resolve(command: &[u8]) -> bool {
    command.contains(&b'`')
        || command
            .windows(2)
            .any(|pair| pair == b"$(" || pair == b"${")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property the whole design rests on: a `chdir` on one worker is not
    /// seen by another worker.
    ///
    /// Two workers enter different directories, both stay there while the other
    /// is entering its own, and each then reads back its own. Sharing one
    /// filesystem context is exactly what this would catch: `chdir` would be
    /// process-wide, the second worker's would land on top of the first's, and
    /// both would read back the same directory.
    ///
    /// It deliberately does not assert anything about the *process* directory,
    /// much as that is the other half of the property. This runs in a shared
    /// test process where other tests enter directories of their own, so the
    /// process directory can move under it for reasons that have nothing to do
    /// with the pool — which it did, and the assertion failed a gate rather
    /// than finding a defect. The two workers disagreeing about where they are
    /// is what says the contexts are unshared, and that is what is asserted.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_worker_stands_in_its_own_directory() {
        let Some(pool) = ReadPool::new(2) else {
            return;
        };
        let entered = std::sync::Arc::new(std::sync::Barrier::new(2));
        let held = std::sync::Arc::clone(&entered);
        let first = pool.start(move || {
            std::env::set_current_dir("/tmp").expect("entering a directory");
            held.wait();
            std::env::current_dir().expect("a working directory")
        });
        let held = std::sync::Arc::clone(&entered);
        let second = pool.start(move || {
            std::env::set_current_dir("/usr").expect("entering a directory");
            held.wait();
            std::env::current_dir().expect("a working directory")
        });
        assert_eq!(
            first.recv().expect("the first read answers"),
            std::path::Path::new("/tmp")
        );
        assert_eq!(
            second.recv().expect("the second read answers"),
            std::path::Path::new("/usr")
        );
    }

    /// A pool of one is no pool: the caller reads on its own thread rather than
    /// paying for a handoff that buys nothing.
    #[test]
    fn one_thread_is_not_a_pool() {
        assert!(ReadPool::new(1).is_none());
        assert!(ReadPool::new(0).is_none());
    }

    /// "As many as it takes" is what both `-j` with no number and no `-j` at
    /// all arrive as. It has to become a number of threads the machine can
    /// actually have, rather than a request for `usize::MAX` of them.
    #[test]
    fn an_unlimited_job_count_is_bounded() {
        let pool = ReadPool::new(usize::MAX);
        if let Some(pool) = pool {
            assert!(
                pool.workers.len() <= crate::os::cores(),
                "an unlimited job count asked for {} workers",
                pool.workers.len()
            );
        }
    }
}
