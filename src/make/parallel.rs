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
//!
//! # Why anything is freed here
//!
//! Many threads read and one thread composes, so by default many threads
//! allocate and one frees. That asymmetry does not shrink when workers are
//! added: the composing thread's share of it grows with the TOTAL work, which
//! is why the run stopped getting faster past `-j4` however many workers it was
//! given. [`Reaper`] and [`Released`] are the two answers, and they differ in
//! whether anybody waits for the memory to come back. Neither changes what is
//! built — freeing is observable to nothing but the allocator, and none of what
//! they free has a `Drop` that does anything else.
//!
//! # What the two of them and the first recipe's dispatch are worth
//!
//! One number for all three, because they were measured together. On the
//! recursive workload the Make baseline uses — 259 Makefiles, fan-out 6, depth
//! 3 — at `-j8`: 259 ms against 192, 2.8 cores busy against 3.5, and the
//! composing thread 34% of the run's samples against 20%.
//!
//! Taken apart against the same baseline, on a host that had other work on it
//! at the time and whose absolute numbers are therefore its own: dispatching
//! the first recipe was 276 ms against 257. Not freeing on this thread AT ALL —
//! which bounds what [`Reaper`] and [`Released`] can be worth rather than
//! measuring them — was 257 against 213.
//!
//! # What serving the reads in the composition's order is worth
//!
//! Reading a Makefile early buys nothing if the composition is not standing
//! where the read lands, and for a long time it was not. Instrumenting every
//! read with the moment it was dispatched, started, finished and consumed, over
//! twenty runs of the same 259-Makefile tree at `-j8`, said where the wall
//! went: the composing thread spent 60 ms of a 127 ms wall blocked in
//! [`ChildRead::collect`], all eight workers were busy the whole time, and 198
//! reads sat dispatched and unstarted at a tenth of the way in. It was not
//! waiting for work to be done. It was waiting for the RIGHT work to be done,
//! while eight workers read units it would not reach for another sixty
//! milliseconds. See [`ReadOrder`] for what that is and why.
//!
//! Serving the queue in the order the composition asks, same instrumentation,
//! same twenty runs: 127.5 ms to 85.3 ms of wall, the composing thread's wait
//! 59.9 ms to 9.0 ms, and the count of units it reached having to wait for a
//! read nothing had yet started 3.7 a run to zero. The two phases the run used
//! to have — every worker busy and the composition blocked, then the
//! composition alone — became one phase with both sides busy: the composing
//! thread is 85% to 98% occupied from a twentieth of the way in, and the cores
//! the run keeps busy went from 5.1 to 6.7.
//!
//! It also moved what the run is waiting on. The composing thread is now busy
//! for 90% of the wall — 76.7 ms of 85.3 — where it was busy for half of a
//! wall half as long again, so it is close to being the thing the run is
//! bound by rather than sitting well under it, and work taken off THAT thread
//! is worth about one to one against the wall while work taken off a worker is
//! still divided by however many there are. It is not the ceiling yet: `-j16`
//! still measures about 12% under `-j8`, as it did before, and the cores the
//! run keeps busy went 5.4 to 6.3 at `-j8` and 6.9 to 7.6 at `-j16`.

use std::sync::mpsc;

/// One unit of work handed to a worker, already holding everything it needs.
type Read = Box<dyn FnOnce() + Send + 'static>;

/// Something a read is finished with, on its way to being freed.
type Discarded = Box<dyn Send + 'static>;

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

/// How many of those a worker handed to another worker, rather than the
/// composing thread handing it over.
///
/// Test-only, and for the same reason [`READS_STARTED`] is: whether a worker
/// carries on into the children of the unit it just read depends on the shape
/// of that unit, so a test that meant to exercise the chaining can stop
/// exercising it without failing. This is what lets it assert that it did. See
/// [`chained_reads`].
#[cfg(test)]
pub(crate) static CHAINED_READS_STARTED: std::sync::atomic::AtomicUsize =
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

/// A thread that frees what the composition is finished with, so that the
/// thread composing does not.
///
/// What a read leaves behind is the evaluator's own working memory — the
/// dependency nodes the graph was emitted from, and the session itself where no
/// recipe of that unit still needs one. It is allocated on whichever worker
/// read the unit and would otherwise be freed on the one thread every unit
/// passes through, so the composition pays a teardown proportional to the total
/// read rather than to its own work.
///
/// This one is waited for: [`ReadPool`] joins it when the compilation that owns
/// it returns, and by then it has had the whole compilation to keep up with a
/// composition that is doing far more than freeing. See [`Released`] for the
/// case where waiting is what there is nothing left to overlap with.
pub(crate) struct Reaper {
    /// `None` once the reaper is being dropped, which closes the queue and is
    /// what tells the thread to finish.
    queue: Option<mpsc::Sender<Discarded>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Reaper {
    /// A reaper, or `None` where the thread could not be started and the caller
    /// should free what it has where it stands.
    fn new() -> Option<Self> {
        let (sender, receiver) = mpsc::channel::<Discarded>();
        let thread = std::thread::Builder::new()
            .name("ronin-make-free".to_owned())
            .spawn(move || {
                while let Ok(discarded) = receiver.recv() {
                    drop(discarded);
                }
            })
            .ok()?;
        Some(Self {
            queue: Some(sender),
            thread: Some(thread),
        })
    }

    /// Free `value` on the reaper's thread rather than on this one.
    fn discard<T: Send + 'static>(&self, value: T) {
        if let Some(queue) = &self.queue {
            // A queue whose reaper has gone hands the value straight back, and
            // dropping the returned error frees it here.
            let _: Result<(), mpsc::SendError<Discarded>> = queue.send(Box::new(value));
        }
    }
}

impl Drop for Reaper {
    fn drop(&mut self) {
        self.queue = None;
        if let Some(thread) = self.thread.take() {
            let _: std::thread::Result<()> = thread.join();
        }
    }
}

/// Free `value` on `reaper` where there is one, and here where there is not.
///
/// A run with no workers has no reaper either, and frees exactly where it froze
/// before this existed.
pub(super) fn discard<T: Send + 'static>(reaper: Option<&Reaper>, value: T) {
    match reaper {
        Some(reaper) => reaper.discard(value),
        None => drop(value),
    }
}

/// Something the run holds until it is over, whose memory nothing waits for.
///
/// The recipes a read left standing are one evaluator session per unit — on a
/// recursive tree, several hundred of them, read by as many threads as the pool
/// has and freed by the one thread the run ends on. Unlike what [`Reaper`]
/// takes, they are freed at the very end: the build has run, the result is
/// settled, and there is no work left to overlap the freeing with. So nothing
/// waits for it. The thread returns the memory if the process is still there to
/// receive it, and the process exits over the top of it if not, which is what
/// the operating system would have done anyway.
///
/// Safe to leave running because none of what it holds reaches outside itself:
/// no descriptor, no file, no lock of this program's own, and no `Drop` that
/// does anything but free.
///
/// A thread is not free either. `elsewhere` is how the caller says there is
/// enough here to be worth one, and it is load bearing: starting one
/// unconditionally cost a one-Makefile build 0.23 ms a run, measured over three
/// hundred runs of it, which is a tenth of that build. A small run frees where
/// it always froze.
pub(crate) struct Released<T: Send + 'static> {
    held: Option<T>,
    elsewhere: bool,
}

impl<T: Send + 'static> Released<T> {
    pub(crate) const fn new(value: T, elsewhere: bool) -> Self {
        Self {
            held: Some(value),
            elsewhere,
        }
    }
}

impl<T: Send + 'static> std::ops::Deref for Released<T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.held
            .as_ref()
            .expect("a released value is held until drop")
    }
}

impl<T: Send + 'static> std::ops::DerefMut for Released<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.held
            .as_mut()
            .expect("a released value is held until drop")
    }
}

impl<T: Send + 'static> Drop for Released<T> {
    fn drop(&mut self) {
        let Some(value) = self.held.take() else {
            return;
        };
        if !self.elsewhere {
            // Freed right here, which is what a run with little to free would
            // have paid more to give away than to do.
            drop(value);
            return;
        }
        if let Err(unstarted) = std::thread::Builder::new()
            .name("ronin-make-free".to_owned())
            .spawn(move || drop(value))
        {
            // No thread to free it on is not a reason to keep it: the closure
            // holding the value comes back with the error, and dropping that
            // frees it here.
            drop(unstarted);
        }
    }
}

/// Where a read stands in the order the composition will ask for it.
///
/// The composition walks the tree depth first — a unit, then the whole subtree
/// under its first recursive recipe, then the subtree under its second — and it
/// consumes the reads in exactly that order. The reads are STARTED in a
/// different one: a worker that has read a unit resolves every child that unit
/// names and hands them all over at once, so what arrives at the queue is a
/// breadth-first wave. Served in arrival order those two diverge immediately,
/// and the composition ends up blocked on a read sitting near the back of a
/// queue two hundred deep while eight workers read units it will not reach for
/// another sixty milliseconds. Measured on the 259-Makefile recursive tree at
/// `-j8`: 198 reads dispatched and not yet started at a tenth of the way in,
/// the composing thread blocked for 60 ms of a 127 ms wall, and 22% of the
/// units composed by the time the last read finished.
///
/// This is the key that serves the queue in the composition's order instead.
/// Each unit holds a half-open interval of the order; its recursive recipes
/// divide that interval between them, in recipe order, one part each; and a
/// read is served by the low end of its own part. So a subtree sorts ahead of
/// its parent's next sibling, the leftmost descendant sorts first of all, and
/// what comes out is a depth-first pre-order over a tree nobody has yet seen
/// the whole of — computed from the two things every dispatching side already
/// has, which are its own interval and which child this is.
///
/// The interval divides at every level, so on a fan-out of six there is nothing
/// left to divide past about twenty-four of them. Descendants below that share
/// a key and are served in the order they arrived, which is the order every
/// read was served in before this existed: the ordering degrades to
/// first-come-first-served rather than to anything wrong.
#[derive(Clone, Copy)]
pub(super) struct ReadOrder {
    /// Where this unit's subtree begins.
    at: u64,
    /// How much of the order it has to divide between its children.
    span: u64,
}

impl ReadOrder {
    /// The whole order, held by a unit no worker read ahead.
    ///
    /// The composition read that unit on its own thread and is about to
    /// dispatch its children and block on the first of them, so those children
    /// are the most urgent reads there are and the whole order is what says so.
    /// It is also what the root of a compilation holds, which is the same
    /// statement about the same thing.
    ///
    /// A unit deeper in the tree that reaches here — one whose worker declined
    /// to chain — lifts its whole subtree over reads that were started for its
    /// own later siblings. The composition is standing inside that subtree, so
    /// lifting it is right; what is imprecise is only that the subtree's last
    /// children are lifted as far as its first.
    pub(super) const WHOLE: Self = Self {
        at: 0,
        span: u64::MAX,
    };

    /// The part of this order belonging to the `index`th of `children`
    /// recursive recipes.
    fn nth(self, index: usize, children: usize) -> Self {
        let children = u64::try_from(children).unwrap_or(u64::MAX).max(1);
        let index = u64::try_from(index).unwrap_or(0).min(children - 1);
        let span = self.span / children;
        Self {
            at: self.at + span * index,
            span,
        }
    }

    /// What the queue serves by, smallest first.
    const fn key(self) -> u64 {
        self.at
    }
}

/// How many reads have been started, which is what breaks a tie in the order.
///
/// Process-wide rather than per pool, because it is only ever compared with
/// itself and a compilation only ever has one pool: what it has to be is
/// increasing, not small.
static ARRIVALS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// One read waiting for a worker, and where the composition will ask for it.
struct Queued {
    key: u64,
    /// Which arrival this was, so that two reads the order cannot tell apart
    /// are served in the order they were started — where they were before
    /// [`ReadOrder`] existed.
    arrival: u64,
    read: Read,
}

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        (self.key, self.arrival) == (other.key, other.arrival)
    }
}

impl Eq for Queued {}

impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Queued {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.key, self.arrival).cmp(&(other.key, other.arrival))
    }
}

/// Reads that have arrived and not yet been taken, held by whichever worker is
/// picking one. See [`ReadOrder`].
///
/// The arrival is a channel and the choice is a heap, and the split is what
/// keeps the sorting off the dispatching thread. A dispatcher only sends, which
/// takes no lock; a worker takes the one lock the workers already shared — a
/// channel receiver cannot be read by eight threads without one — moves
/// whatever has arrived into the heap, and takes the read the composition will
/// ask for soonest. The whole of what this adds to a worker's turn is a heap
/// push and a heap pop, on the thread that was going to take that lock anyway.
///
/// The other spelling — a lock the dispatcher takes too, holding the heap and a
/// condition variable in place of the channel — was written and measured
/// against this one. On the recursive tree the two are the same to within the
/// host's noise, and on a unit with two hundred and sixteen recursive recipes
/// all dispatched from the one composing thread NEITHER could be told from the
/// other or from an unchanged tree: semantically inert variants of this file
/// measured anywhere from 0.94x to 1.27x of it on that shape, which brackets
/// both. This one is kept because a thread starting a read should not have to
/// wait behind eight taking them, not because the difference was measurable.
struct Taking {
    ready: std::collections::BinaryHeap<std::cmp::Reverse<Queued>>,
    /// Where dispatchers put reads. Closing it is what ends the workers, which
    /// is why a dispatcher is what a read in flight holds: the queue stays open
    /// until every read that could start another has finished.
    arrivals: mpsc::Receiver<Queued>,
}

impl Taking {
    /// The read the composition will ask for soonest, or `None` where nothing
    /// is waiting and nothing can arrive.
    ///
    /// A read taken while nothing was waiting is taken as it arrives rather
    /// than against what arrives just after it. That is the one place the order
    /// gives way, and it gives way where there was no choice to make.
    fn take(&mut self) -> Option<Read> {
        loop {
            while let Ok(queued) = self.arrivals.try_recv() {
                self.ready.push(std::cmp::Reverse(queued));
            }
            if let Some(std::cmp::Reverse(queued)) = self.ready.pop() {
                return Some(queued.read);
            }
            self.ready
                .push(std::cmp::Reverse(self.arrivals.recv().ok()?));
        }
    }
}

/// Where more reads may be started from, held by a worker that is performing
/// one.
///
/// A worker that has just read a unit resolves the recursive children that unit
/// names and reads them too, which needs a way back into the queue. It holds
/// this rather than the pool itself, and the difference is whether the
/// compilation can ever return: the pool ends its workers by dropping its own
/// dispatcher, and a worker holding the pool would keep the pool alive instead
/// of the queue open, so nothing would ever close and the join would never
/// return. A dispatcher is dropped when the read that holds it finishes, so the
/// queue closes as soon as the last read is over.
#[derive(Clone)]
pub(crate) struct Dispatcher {
    queue: mpsc::Sender<Queued>,
    /// Set when the pool is going away. A read still in the queue when that
    /// happens answers nothing and reads nothing: whoever asked for it holds the
    /// session and reads it where it stands, which is what the answer never
    /// arriving already means. Without this, abandoning a compilation would wait
    /// for every read its workers had queued ahead of it.
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Dispatcher {
    /// Start `read` on a worker, answering on the returned receiver.
    ///
    /// The answer is a channel rather than a shared slot because the caller
    /// consumes the reads in the order the recipe wrote them, not the order
    /// they finish: it waits for the one it has reached and lets the rest go on
    /// arriving behind it. `order` is where in that order this one stands, and
    /// it is what decides which waiting read a free worker takes next.
    pub(crate) fn start<T, F>(&self, order: ReadOrder, read: F) -> mpsc::Receiver<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel();
        let stopped = std::sync::Arc::clone(&self.stopped);
        let _: Result<(), mpsc::SendError<Queued>> = self.queue.send(Queued {
            key: order.key(),
            arrival: ARRIVALS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            read: Box::new(move || {
                if stopped.load(std::sync::atomic::Ordering::Acquire) {
                    return;
                }
                let _: Result<(), mpsc::SendError<T>> = sender.send(read());
            }),
        });
        receiver
    }
}

/// Worker threads that read child Makefiles, each standing in its own
/// directory.
pub(crate) struct ReadPool {
    /// `None` once the pool is being dropped, which closes the queue and is
    /// what tells the workers to finish.
    dispatcher: Option<Dispatcher>,
    workers: Vec<std::thread::JoinHandle<()>>,
    /// Where what the reads leave behind is freed. `None` where the thread
    /// could not be started, which costs speed and nothing else.
    reaper: Option<Reaper>,
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
        let (sender, arrivals) = mpsc::channel::<Queued>();
        let taking = std::sync::Arc::new(std::sync::Mutex::new(Taking {
            ready: std::collections::BinaryHeap::new(),
            arrivals,
        }));
        let mut workers: Vec<std::thread::JoinHandle<()>> = Vec::with_capacity(threads);
        for _ in 0..threads {
            let served = std::sync::Arc::clone(&taking);
            let Ok(worker) = std::thread::Builder::new()
                .name("ronin-make-read".to_owned())
                .spawn(move || Self::serve(&served))
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
            dispatcher: Some(Dispatcher {
                queue: sender,
                stopped: std::sync::Arc::default(),
            }),
            workers,
            reaper: Reaper::new(),
        })
    }

    /// Where this pool's callers free what a read left behind.
    pub(crate) const fn reaper(&self) -> Option<&Reaper> {
        self.reaper.as_ref()
    }

    /// Where a read is started from, whether by the composition or by a worker
    /// already performing one.
    const fn dispatcher(&self) -> Option<&Dispatcher> {
        self.dispatcher.as_ref()
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
    fn serve(taking: &std::sync::Mutex<Taking>) {
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
                let Ok(mut taking) = taking.lock() else {
                    return;
                };
                taking.take()
            };
            let Some(work) = work else {
                return;
            };
            work();
        }
    }

    /// Start `read` on a worker, answering on the returned receiver.
    ///
    /// Answers on a receiver that never delivers where the pool is already
    /// going away, which is what the caller reads as "no worker read it".
    #[cfg(test)]
    fn start<T, F>(&self, read: F) -> mpsc::Receiver<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        self.dispatcher.as_ref().map_or_else(
            || mpsc::channel().1,
            |dispatcher| dispatcher.start(ReadOrder::WHOLE, read),
        )
    }
}

impl Drop for ReadPool {
    fn drop(&mut self) {
        // Closing the queue is what ends the workers: a worker blocked on an
        // empty queue wakes when the last dispatcher goes, and one part way through
        // a read finishes it first. Joined rather than detached so that no
        // worker is still standing in a directory, or still holding a session,
        // after the compilation that owns them has returned.
        //
        // A read a worker queued for a descendant may still be waiting when
        // that happens, and there may be a whole subtree of them. Stopping them
        // first is what keeps abandoning a compilation cheap: each one answers
        // nothing, which is what whoever asked already treats as its own cue to
        // read the unit itself.
        if let Some(dispatcher) = &self.dispatcher {
            dispatcher
                .stopped
                .store(true, std::sync::atomic::Ordering::Release);
        }
        self.dispatcher = None;
        for worker in self.workers.drain(..) {
            let _: std::thread::Result<()> = worker.join();
        }
        // The reaper is joined by its own drop, which runs as this struct's
        // fields are released — so the compilation returns with nothing of it
        // still holding a session either.
    }
}

// ---------------------------------------------------------------------------
// Reading ahead
// ---------------------------------------------------------------------------

use super::{
    Compilation, CompilationContext, CompilationState, Evaluated, MakeError, Prepared,
    ReadJournals, Session, admit_regeneration_roots, command_line_environment, evaluate,
    evaluated_flag_variables, exported_environment, flag_recipe_environment, in_directory,
    refused_makefiles, sink,
};

/// Settle everything a read says that the graph is not told.
///
/// Split from [`super::read_unit`] because none of it reaches the
/// [`super::GraphSink`] every unit shares — it reads the evaluator and the
/// dependency nodes and nothing else — so it can run on the worker that read
/// the Makefile instead of on the one thread every unit's emission passes
/// through. Emission stays where the sink is, and this runs in the order it
/// always ran relative to it: before.
///
/// The environment work is why the split is worth making. Deciding a unit's
/// export set walks its whole variable table, and it is done once per unit for
/// as many units as the tree has.
///
/// So is the population. [`kati::ninja::populate_build`] is the half of an
/// emission that never reaches the sink: it walks the dependency nodes, expands
/// whichever recipes the destination cannot expand for itself, and mints the
/// rule numbers. Only what it settled is then handed over, on the one thread
/// that writes the graph, in the order it always was.
///
/// What moves with the two of them is the timing of the evaluation-time effects
/// they carry — a `$(shell)` in an exported recursive value or in `MAKEFLAGS`,
/// and, for a recipe the compiler itself has to read, the `$(shell)`, `$(file)`
/// and `$(info)` in that recipe. Under
/// [`RecipeExpansion::Launch`](kati::build_sink::RecipeExpansion::Launch) the
/// recipes read here are only the ones whose text the graph's shape depends on
/// — a recursive `$(MAKE)` line, an automatic depfile, a `::` action — and
/// their effects were already happening while the graph was built rather than
/// when the recipe ran. This is the same edge reading a Makefile early already
/// takes, moved to the same thread, and taken only where a worker read the
/// unit. What they SAY is not moved: the diagnostics a worker raises are held
/// and drained where the composition reaches this child. See [`ChildRead`] and
/// [`read_ahead`].
fn prepare_read(
    evaluated: Evaluated,
    evaluation: kati::ninja::BuildEvaluation,
    chains: bool,
) -> Result<Prepared, MakeError> {
    let Evaluated {
        mut ev,
        mut nodes,
        regeneration_nodes,
        refusals,
    } = evaluated;
    let refusals = refused_makefiles(refusals);
    let regeneration_names = admit_regeneration_roots(&mut nodes, regeneration_nodes);
    let (exported, unreadable) =
        exported_environment(&mut ev).map_err(|error| MakeError::evaluate(&error))?;
    let command_line = command_line_environment(&mut ev, &exported, unreadable.as_ref())
        .map_err(|error| MakeError::evaluate(&error))?;
    // A Makefile may replace MAKEOVERRIDES (and therefore the recursive
    // MAKEFLAGS value) before naming a child. That evaluated compiler
    // variable, not the invocation's pre-evaluation seed, is what the
    // semantic subninja parses.
    let (makeflags, mflags) =
        evaluated_flag_variables(&mut ev).map_err(|error| MakeError::evaluate(&error))?;
    let flag_environment = flag_recipe_environment(&makeflags, mflags);
    let populated = kati::ninja::populate_build(&nodes, &mut ev, evaluation)
        .map_err(|error| MakeError::evaluate(&error))?;
    // The dependency graph the build will be emitted from, which nothing
    // downstream of the population reads: it is the read's own working memory,
    // and it is freed on whichever thread built it rather than on the one
    // thread every unit passes through.
    drop(nodes);
    // Asked only where a worker could act on the answer. A serial run reads
    // every Makefile on its own thread whatever this says, and must not pay
    // even the walk that would tell it what it could have read ahead.
    let recursions = chains.then(|| populated.liftable_recursions(&ev)).flatten();
    Ok(Prepared {
        ev,
        populated,
        recursions,
        refusals,
        regeneration_names,
        exported,
        unreadable,
        command_line,
        makeflags,
        flag_environment,
    })
}

/// Read one unit's Makefiles and settle everything they say that the graph is
/// not told, which is the half of the read that reaches no graph.
///
/// Split from [`super::read_unit`] because it is at once the expensive half and the
/// only half that can leave this thread: it is handed a session and gives back
/// what the Makefiles said, without ever touching the one [`super::GraphSink`] every
/// unit shares. Everything downstream of it — emitting the build, taking the
/// unit, the layout — writes to that sink and stays where the sink is. See
/// [`ReadPool`].
///
/// [`prepare_read`] is here rather than beyond the handoff for the same reason
/// the evaluation is: it reads the evaluator this call just built and writes to
/// no sink, so the thread that made the evaluator is the thread that should
/// pay for reading it.
///
/// The directory is entered here rather than around both halves because
/// entering it is what a worker does on its own behalf: kati reads relative
/// names against the working directory, and on a worker that directory is the
/// worker's own.
pub(super) fn evaluate_unit(
    session: Session,
    directory: &std::path::Path,
    evaluation: kati::ninja::BuildEvaluation,
    chains: bool,
) -> Result<Prepared, MakeError> {
    in_directory(directory, || {
        prepare_read(
            evaluate(session).map_err(|error| MakeError::evaluate(&error))?,
            evaluation,
            chains,
        )
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

/// One unit a worker read, and whatever that worker went on to read for the
/// children the unit itself names. See [`chained_reads`].
pub(super) struct ReadAhead {
    pub(super) prepared: Prepared,
    pub(super) chained: Vec<ChainedRead>,
}

/// The four values a recursive invocation is resolved from, kept so the
/// composition can recognise the invocation a worker resolved for it.
///
/// Matched on rather than trusted by position, because position is a fact about
/// two lists built in two places and this is a fact about the invocation itself.
#[derive(PartialEq, Eq)]
struct InvocationKey {
    command: Vec<u8>,
    make: Vec<u8>,
    shell: Vec<u8>,
    shell_flags: Vec<u8>,
}

impl InvocationKey {
    /// What a worker resolved, as the composition will ask for it.
    fn of(invocation: &sink::SubninjaInvocation) -> Self {
        Self {
            command: invocation.command.clone(),
            make: invocation.make.clone(),
            shell: invocation.shell.clone(),
            shell_flags: invocation.shell_flags.clone(),
        }
    }

    /// The same, as the population settled it before any of it reached a sink.
    fn lifted(recursion: &kati::ninja::PopulatedRecursion) -> Self {
        Self {
            command: recursion.command.to_vec(),
            make: recursion.make.to_vec(),
            shell: recursion.shell.to_vec(),
            shell_flags: recursion.shell_flags.to_vec(),
        }
    }
}

/// What the worker that read one unit left the composition of that unit's own
/// children.
///
/// The two travel together because they come from the same place and are used
/// in the same breath: [`read_ahead`] takes the reads the worker already
/// started, and orders whatever it has to start itself out of where this unit
/// stands. See [`ReadOrder`].
pub(super) struct ReadsAhead {
    pub(super) chained: Vec<ChainedRead>,
    pub(super) order: ReadOrder,
}

/// One recursive invocation of a unit, resolved and read by the worker that
/// read the unit naming it.
pub(super) struct ChainedRead {
    invocation: InvocationKey,
    child: ChildUnit,
}

/// Everything a worker needs to carry on reading past the unit it was given.
///
/// Cloned into each read rather than borrowed, because a read outlives the call
/// that started it: the composition has moved on by the time the worker finishes
/// and resolves the children it found.
#[derive(Clone)]
struct ChainPlan {
    dispatcher: Dispatcher,
    resolve: super::Resolver,
    /// What an earlier pass told each unit it read, which is what this pass
    /// tells it again.
    read_units: std::sync::Arc<ReadJournals>,
    /// Every unit whose read has been started, so that no unit is read twice.
    claims: std::sync::Arc<std::sync::Mutex<crate::htab::RapidHashSet<Vec<u8>>>>,
    evaluation: kati::ninja::BuildEvaluation,
}

impl ChainPlan {
    /// Say whether this cache key is ours to read, claiming it if it is.
    ///
    /// One unit is read once per pass however many recipes name it. The
    /// composition gets that from its own cache of compiled subgraphs; a worker
    /// cannot see that cache, and both sides claim here instead.
    fn claim(&self, key: &[u8]) -> bool {
        self.claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.to_vec())
    }
}

/// One recursive child's Makefiles, read on a worker before the composition
/// reached the recipe that asks for them.
pub(super) struct ChildRead {
    pub(super) cache_key: Vec<u8>,
    pub(super) context: CompilationContext,
    /// Where the worker answers.
    answer: std::sync::mpsc::Receiver<Option<Result<ReadAhead, MakeError>>>,
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
    /// What the emission this read prepares will be asked to evaluate, for the
    /// case where this thread ends up performing the read after all.
    evaluation: kati::ninja::BuildEvaluation,
    /// Where the composition will ask for this unit, and therefore the order
    /// its own children are divided out of. See [`ReadOrder`].
    order: ReadOrder,
}

impl ChildRead {
    /// Wait for the read, and put what it raised where the invocation collects
    /// it.
    ///
    /// The warnings are drained here, at the moment the composition reaches
    /// this child, rather than as they were raised: that is what puts them back
    /// in the order the recipes were written in. A Makefile read early on a
    /// worker still belongs behind everything the recipes before it said, which
    /// is where GNU Make would have put it. What the worker said while it
    /// resolved this child's own children is in the same descriptor and behind
    /// the read, which is where those words fall when the composition does the
    /// resolving itself.
    /// The context hands itself back rather than being read through the read,
    /// because the composition wants it too and it is not a cheap thing to have
    /// twice: three shared handles, three paths, a string and four vectors, all
    /// of which would be copied here and thrown away again on the one thread
    /// every unit passes through.
    ///
    /// What the read is left holding — the answer channel, the descriptor it
    /// raised into and the cell the session was offered through — is freed on
    /// the reaper's thread for the same reason everything else a read leaves
    /// behind is. See [`Reaper`].
    pub(super) fn collect(
        self,
        reaper: Option<&Reaper>,
    ) -> (CompilationContext, Result<ReadAhead, MakeError>) {
        let Self {
            cache_key,
            context,
            answer,
            raised,
            unread,
            evaluation,
            order: _,
        } = self;
        let read =
            if let Ok(Some(read)) = answer.recv() {
                read
            } else {
                // No worker read it. The session was left where either side could
                // take it for exactly this, so the read happens here instead — and
                // nothing was read ahead for its children either.
                let taken = unread
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                match taken {
                    Some(session) => evaluate_unit(session, &context.directory, evaluation, false)
                        .map(|prepared| ReadAhead {
                            prepared,
                            chained: Vec::new(),
                        }),
                    None => Err(MakeError::Evaluate(
                        "reading a recursive Make child was abandoned".to_owned(),
                    )),
                }
            };
        context.diagnostics.absorb(&raised);
        let read = read.map(|mut read| {
            // From here the session speaks for itself again: anything it says
            // while a recipe of it is expanded is said in the order the build
            // reaches it, and belongs in the invocation's own descriptor.
            read.prepared.ev.session.diagnostics = std::sync::Arc::clone(&context.diagnostics);
            read
        });
        discard(reaper, (cache_key, answer, raised, unread));
        (context, read)
    }
}

impl ChildUnit {
    /// Where the composition stands when it reaches this unit, which is what
    /// its own children's reads are ordered out of. See [`ReadOrder`].
    pub(super) const fn read_order(&self) -> ReadOrder {
        match self {
            Self::Read(read) => read.order,
            // Nothing read this one ahead, so the composition is about to read
            // it here and then block on the first child it dispatches.
            Self::Unread(_) | Self::Refused(_) => ReadOrder::WHOLE,
        }
    }

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

    /// What this unit is cached under, and the read that will answer for it.
    ///
    /// Either a worker already read it while an earlier recipe of the same
    /// parent was being composed, or nothing has read it and the composition
    /// reads it. Both go through the same prepared session and produce the same
    /// value; the only difference is which thread paid for it.
    ///
    /// An unread unit carries its context out beside the session, because it is
    /// the composition that needs it. A started read leaves its own where it is
    /// until [`ChildRead::collect`] hands it back, so neither is copied.
    pub(super) fn claim(self, state: &super::CompilationState<'_>) -> Result<Claimed, MakeError> {
        Ok(match self {
            Self::Refused(error) => return Err(error),
            Self::Unread(compilation) => {
                let mut compilation = *compilation;
                let cache_key = compilation.cache_key.clone();
                let context = compilation.context.clone();
                prepare_session(&mut compilation, &state.read_units);
                state
                    .read_claims
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(cache_key.clone());
                (cache_key, Err((compilation.session, context)))
            }
            // The key is taken rather than copied: a started read holds the only
            // copy of it and nothing reads it through the read again.
            Self::Read(mut read) => (std::mem::take(&mut read.cache_key), Ok(read)),
        })
    }
}

/// What [`ChildUnit::claim`] answers with.
pub(super) type Claimed = (
    Vec<u8>,
    Result<Box<ChildRead>, (Session, CompilationContext)>,
);

/// Start reading the Makefiles of every recursive recipe of one unit, so that
/// none of them is read on the thread that composes.
///
/// # What may be read early, and why it is only ever a read
///
/// Composition is not moved. One graph is built by one thread in the order it
/// was built in before, so every node, every edge and every invented name
/// falls where it fell — the graph does not depend on what the workers did or
/// when. Only the Makefile read moves, and the half of the emission that
/// reaches no graph at all with it. See [`evaluate_unit`].
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
/// # Where the reads come from
///
/// This one thread starts the reads for the unit it is composing. Everything
/// deeper is started by the workers themselves: a worker that finishes a unit
/// resolves the children that unit names and reads those too, so a subtree's
/// reads are in flight long before the composition walks down to it. See
/// [`chained_reads`]. What arrives here as `chained` is what the worker that
/// read THIS unit already did for it, and where there is any of it this call
/// resolves nothing and starts nothing: the invocations are matched against
/// what the worker resolved, and each pending recipe takes its own.
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
/// going to read — it is read a pass earlier than it would have been. Chaining
/// widens that: the reads in flight when a boundary is reached are a subtree's
/// worth rather than one recipe's.
///
/// And a read that runs early sees the tree as it is early. Resolving an
/// invocation looks for the child's default Makefile on disk, and the read
/// itself runs the Makefile's `$(shell)` calls; either could see a file that an
/// earlier recipe's own read was going to write and has not written yet. What
/// the graph is built from still reaches the sink in recipe order, so the graph
/// does not move — but which bytes a Makefile was read from can, exactly as it
/// can for GNU Make's concurrent children.
pub(super) fn read_ahead(
    ordered: &[sink::PendingSubninja],
    resolve: super::Resolver,
    descendant_context: &CompilationContext,
    state: &CompilationState<'_>,
    ahead: ReadsAhead,
) -> Vec<Option<ChildUnit>> {
    let ReadsAhead { chained, order } = ahead;
    if !chained.is_empty() {
        return adopted(ordered, chained);
    }
    // A run with no workers to read on, or a unit with nothing to overlap, is
    // answered on an integer comparison. Asked first, and before the shape of
    // the unit is examined at all, because this is the whole of what a `-j1`
    // run pays for the existence of read-ahead: at `-j1` there is no second
    // thread to read on, and a serial run must not pay even the cost of being
    // asked whether it could have used one.
    //
    // A unit with one recursive recipe has nothing to overlap: the composition
    // asks for that read immediately and has no other recipe's resolution or
    // staging to get on with while a worker performs it, so the handoff would
    // be the whole of what moved.
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
        .get_or_init(|| ReadPool::new(state.read_threads).map(std::sync::Arc::new))
    else {
        return Vec::new();
    };
    let Some(dispatcher) = pool.dispatcher() else {
        return Vec::new();
    };
    let plan = ChainPlan {
        dispatcher: dispatcher.clone(),
        resolve,
        read_units: std::sync::Arc::clone(&state.read_units),
        claims: std::sync::Arc::clone(&state.read_claims),
        evaluation: state.evaluation,
    };
    // Every recipe is resolved here, in the order the recipes were written,
    // which is the order they were resolved in before. Resolving is not silent
    // — it is where an invocation's own switches are parsed, and parsing one
    // can raise a warning — so the order it happens in is the order the recipes
    // stand in and not the order the reads finish.
    //
    // A unit two of these recipes both invoke is compiled once and taken from
    // the cache the second time. Reading it twice would run its `$(shell)`
    // calls twice, so the second recipe is left to find it where the first put
    // it.
    //
    // THE FIRST RECIPE IS DISPATCHED LIKE THE REST, and dispatched before the
    // ones after it are even resolved. The composition asks for it next, so a
    // worker cannot finish it any sooner than this thread would have — but it
    // can finish it while this thread resolves the recipes behind it, works out
    // which of their targets the disk says are current, and stages the first
    // one's wrapper. Read here instead, that work waits behind a read that does
    // not need it; read there, it is the read that waits.
    let mut started = Vec::with_capacity(ordered.len());
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
                    if plan.claim(&compilation.cache_key) {
                        prepare_session(&mut compilation, &plan.read_units);
                        ChildUnit::Read(Box::new(start_read(
                            &plan,
                            compilation,
                            order.nth(index, ordered.len()),
                        )))
                    } else {
                        ChildUnit::Unread(Box::new(compilation))
                    }
                }
            },
        ));
    }
    started
}

/// Line the reads a worker started up against the recipes they were started
/// for.
///
/// Matched on the invocation rather than taken in order, because the two lists
/// are built in two places — one from the populated nodes, the other from what
/// those nodes' edges left in the sink — and a pending recipe whose invocation
/// is not among them is simply one nothing was read for, which the composition
/// resolves for itself.
fn adopted(ordered: &[sink::PendingSubninja], chained: Vec<ChainedRead>) -> Vec<Option<ChildUnit>> {
    let mut chained = chained.into_iter().map(Some).collect::<Vec<_>>();
    ordered
        .iter()
        .map(|pending| {
            let wanted = InvocationKey::of(pending.invocations.first()?);
            let matched = chained
                .iter()
                .position(|read| read.as_ref().is_some_and(|read| read.invocation == wanted))?;
            chained[matched].take().map(|read| read.child)
        })
        .collect()
}

/// Read the children one unit names, from the worker that has just read the
/// unit itself.
///
/// This is what stops the reads being bounded by one recipe's fan-out. The
/// composition dispatches the children of the unit it is composing and then
/// waits for the first of them; everything below that would otherwise wait for
/// the composition to walk down to it. Started here, a subtree's reads are in
/// flight while the composition is still emitting the unit above them.
///
/// The gate is [`read_ahead`]'s, asked of the populated nodes instead of the
/// sink's pending recipes, because the sink has not been written yet: see
/// [`kati::ninja::PopulatedBuild::liftable_recursions`], which answers `None`
/// unless every recursive recipe of the unit is one the composition is certain
/// to reach.
///
/// What the resolutions say goes into `raised` with what the read said, and
/// behind it, which is where those words fall when the composition resolves for
/// itself. The children's own contexts get the real descriptor back, because a
/// child's diagnostics belong wherever the composition puts them and not in the
/// collection its parent's read is held in.
fn chained_reads(
    prepared: &Prepared,
    context: &CompilationContext,
    raised: &std::sync::Arc<kati::diagnostics::Diagnostics>,
    plan: &ChainPlan,
    order: ReadOrder,
) -> Vec<ChainedRead> {
    let Some(recursions) = prepared.recursions.as_ref() else {
        return Vec::new();
    };
    if recursions.len() < 2
        || !context_admits_early_reads(context)
        || recursions
            .iter()
            .any(|recursion| runs_a_command_to_resolve(&recursion.command))
    {
        return Vec::new();
    }
    let mut descendant_context = context.clone();
    descendant_context.makeflags.clone_from(&prepared.makeflags);
    super::apply_exported_environment(&mut descendant_context.environment, &prepared.command_line);
    super::apply_exported_environment(&mut descendant_context.environment, &prepared.exported);
    super::apply_recipe_environment(
        &mut descendant_context.recipe_environment,
        &prepared.flag_environment,
    );
    super::apply_recipe_environment(
        &mut descendant_context.recipe_environment,
        &prepared.exported,
    );
    let reached = std::mem::replace(
        &mut descendant_context.diagnostics,
        std::sync::Arc::clone(raised),
    );
    let mut chained = Vec::with_capacity(recursions.len());
    let children = recursions.len();
    for (index, recursion) in recursions.iter().enumerate() {
        let child = match (plan.resolve)(
            &recursion.command,
            &recursion.make,
            &recursion.shell,
            &recursion.shell_flags,
            &descendant_context,
        ) {
            Err(refusal) => ChildUnit::Refused(refusal),
            Ok(mut compilation) => {
                compilation.context.diagnostics = std::sync::Arc::clone(&reached);
                if plan.claim(&compilation.cache_key) {
                    prepare_session(&mut compilation, &plan.read_units);
                    #[cfg(test)]
                    CHAINED_READS_STARTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    ChildUnit::Read(Box::new(start_read(
                        plan,
                        compilation,
                        order.nth(index, children),
                    )))
                } else {
                    ChildUnit::Unread(Box::new(compilation))
                }
            }
        };
        chained.push(ChainedRead {
            invocation: InvocationKey::lifted(recursion),
            child,
        });
    }
    chained
}

/// Hand one prepared child compilation to a worker, and let that worker carry
/// on into the child's own children.
fn start_read(plan: &ChainPlan, compilation: Compilation, order: ReadOrder) -> ChildRead {
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
    let evaluation = plan.evaluation;
    let carried = plan.clone();
    let below = context.clone();
    let collecting = std::sync::Arc::clone(&raised);
    let answer = plan.dispatcher.start(order, move || {
        let session = held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        session.map(|session| {
            let prepared = evaluate_unit(session, &directory, evaluation, true)?;
            let chained = chained_reads(&prepared, &below, &collecting, &carried, order);
            Ok(ReadAhead { prepared, chained })
        })
    });
    ChildRead {
        cache_key,
        context,
        answer,
        raised,
        unread,
        evaluation,
        order,
    }
}

/// Whether this invocation's surroundings allow a read to start before the
/// composition reaches it. See [`read_ahead`].
fn context_admits_early_reads(context: &CompilationContext) -> bool {
    // A report classifies invocations as it reaches them and tolerates a child
    // it cannot read; neither is worth teaching to happen out of order for a
    // path that is not the one under time pressure.
    if context.reporting || context.census.is_recording() {
        return false;
    }
    // A descriptor that writes each warning through as it is raised has already
    // put it wherever the threads happened to put it. Only a held one can be
    // drained back into the order the recipes were written in.
    context.diagnostics.is_collecting()
}

/// Whether every recursive recipe of one unit is one a read may start early
/// for. See [`read_ahead`].
fn reads_may_start_early(ordered: &[sink::PendingSubninja], context: &CompilationContext) -> bool {
    if !context_admits_early_reads(context) {
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

    /// A value that says when it was freed, and on which thread.
    struct Witness(std::sync::Arc<std::sync::Mutex<Option<std::thread::ThreadId>>>);

    impl Drop for Witness {
        fn drop(&mut self) {
            *self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(std::thread::current().id());
        }
    }

    /// What is given to a reaper is freed, and freed somewhere else.
    ///
    /// Both halves matter and for different reasons: something the reaper
    /// swallowed and never freed would be a leak that grows with the size of
    /// the tree, and something it freed on the calling thread would be the cost
    /// this exists to move, still being paid.
    #[test]
    fn a_reaper_frees_its_work_on_another_thread() {
        let Some(reaper) = Reaper::new() else {
            return;
        };
        let freed = std::sync::Arc::new(std::sync::Mutex::new(None));
        reaper.discard(Witness(std::sync::Arc::clone(&freed)));
        // Joins the reaper, so the answer is settled by the time it returns.
        drop(reaper);
        let freed_on = *freed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(
            freed_on.is_some(),
            "the reaper never freed what it was given"
        );
        assert_ne!(
            freed_on,
            Some(std::thread::current().id()),
            "the reaper freed it on the thread that handed it over"
        );
    }

    /// A release the caller said was not worth a thread is freed where it
    /// stands, and one that was is freed off it.
    ///
    /// The small case is the one with teeth: starting a thread costs about what
    /// freeing one unit's session does, so a run with little to free that
    /// started one anyway would be slower for the trouble.
    #[test]
    fn a_release_takes_a_thread_only_when_told() {
        let freed = std::sync::Arc::new(std::sync::Mutex::new(None));
        drop(Released::new(Witness(std::sync::Arc::clone(&freed)), false));
        assert_eq!(
            *freed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            Some(std::thread::current().id()),
            "a release not worth a thread was not freed where it stood"
        );

        let freed = std::sync::Arc::new(std::sync::Mutex::new(None));
        drop(Released::new(Witness(std::sync::Arc::clone(&freed)), true));
        // Nothing waits for this one, which is the whole point of it, so the
        // answer is waited for here instead rather than asserted immediately.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let freed_on = loop {
            let seen = *freed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if seen.is_some() || std::time::Instant::now() > deadline {
                break seen;
            }
            std::thread::yield_now();
        };
        assert!(freed_on.is_some(), "a released value was never freed");
        assert_ne!(
            freed_on,
            Some(std::thread::current().id()),
            "a release worth a thread was freed on the run's own thread"
        );
    }
}
