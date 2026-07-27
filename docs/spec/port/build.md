# build.c, build.h

> [spec:samurai:def:build.build-fn]
> void build(void)

> [spec:samurai:sem:build.build-fn]
> If no dirty non-phony edge was scheduled, warns that there is nothing to do.
> Otherwise installs restartable handlers that relay termination signals through
> a pipe, records the start time, and repeatedly starts ready edges until the
> job, load, pool, and failure limits are reached. Phony and dry-run edges
> complete their outputs immediately; real edges run through a shell with
> output polled into per-job buffers. Completed jobs release pool capacity,
> print buffered output on failure (or when no console job owns output), record
> successful outputs, and unblock dependent edges. A relayed signal is sent to
> all live children, restored to its default action, and re-raised. It frees
> scheduler buffers when no work remains; any start or command failure then
> terminates with a diagnostic, otherwise resets the scheduled-total counter.

> [spec:samurai:def:build.buildadd-fn]
> void buildadd(struct node *n)

> [spec:samurai:sem:build.buildadd-fn]
> Recursively evaluates the generating edge for `n`. A source node is statted
> if necessary, must exist, and is marked clean. For generated nodes it rejects
> dependency cycles, skips an already visited edge, marks it visited and
> cycle-in-progress, stats all outputs, loads recorded dependencies, and visits
> each input. It tracks the newest non-order-only input and counts inputs that
> are dirty or blocked. It marks outputs dirty when missing, out of date, absent
> from the build log as applicable, or built by a changed command; any dirty
> input/output makes all outputs dirty. A dirty unblocked edge is queued, while
> clean/prunable edges get their prune count, and non-phony dirty work increases
> the total. The cycle marker is always removed before return.

> [spec:samurai:def:build.buildoptions]
> struct buildoptions {
>   size_t maxjobs, maxfail;
>   _Bool verbose, explain, keepdepfile, keeprsp, dryrun;
>   const char *statusfmt;
>   double maxload;
> }

> [spec:samurai:def:build.buildreset-fn]
> void buildreset(void)

> [spec:samurai:sem:build.buildreset-fn]
> Walks every edge in the global edge list and clears its scheduled-work flag,
> allowing a fresh build traversal to visit and queue those edges again.

> [spec:samurai:def:build.catchsig-fn]
> static void catchsig(int sig)

> [spec:samurai:sem:build.catchsig-fn]
> Writes the received signal number to the scheduler's signal-pipe write end so
> the normal polling loop, rather than the signal handler, performs shutdown.

> [spec:samurai:def:build.edgedone-fn]
> static void edgedone(struct edge *e)

> [spec:samurai:sem:build.edgedone-fn]
> For every output, saves its old mtime, stats it after the command, sets its
> log mtime to the new mtime or zero when missing, and propagates completion;
> with `restat`, unchanged outputs are pruned after recomputing their newest
> input log mtime. It removes a response file unless preservation is requested,
> computes the edge command hash, copies it to every output, and records both
> dependency and build-log state.

> [spec:samurai:def:build.formatstatus-fn]
> static size_t formatstatus(char *buf, size_t len)

> [spec:samurai:sem:build.formatstatus-fn]
> Expands the configured status format into a bounded buffer while returning
> the untruncated output length. Literal characters and `%%` are copied;
> `%s`, `%f`, `%t`, `%r`, `%u`, `%p`, `%o`, and `%e` emit started, finished,
> total, running, unstarted, percentage, throughput, and elapsed-time values.
> Throughput and elapsed time use the monotonic clock and warn on clock failure.
> Unknown placeholders and formatting failures are fatal; a positive-capacity
> output is NUL-terminated.

> [spec:samurai:def:build.isdirty-fn]
> static bool isdirty(struct node *n, struct node *newest, bool generator, bool restat)

> [spec:samurai:sem:build.isdirty-fn]
> Determines whether one output requires rebuilding. A missing phony output is
> dirty only when its phony edge has no inputs; otherwise it inherits the newest
> input mtime and is clean. A normal missing output is dirty. An output older
> than the newest input is dirty unless `restat` can rely on a prior log record.
> A missing log record dirties non-generators, and a log mtime older than the
> newest input dirties any output. Generators otherwise remain clean. Finally,
> non-generators compare the edge command hash with the output hash and are
> dirty on a mismatch. Each dirty reason is optionally explained.

> [spec:samurai:def:build.isnewer-fn]
> static bool isnewer(struct node *n1, struct node *n2)

> [spec:samurai:sem:build.isnewer-fn]
> Returns true exactly when `n1` is non-null and its modification time is
> greater than `n2`'s modification time.

> [spec:samurai:def:build.job]
> struct job {
>   struct string *cmd;
>   struct edge *edge;
>   struct buffer buf;
>   size_t next;
>   pid_t pid;
>   int fd;
>   bool failed;
> }

> [spec:samurai:def:build.jobdone-fn]
> static void jobdone(struct job *j)

> [spec:samurai:sem:build.jobdone-fn]
> Waits for the child, marks the job failed and warns for wait failures,
> nonzero exits, signals, or unexpected statuses, then closes its output pipe.
> Buffered output is printed when a console job did not own the console or the
> job failed. It releases the associated pool: console ownership is cleared and
> one waiting pooled edge is moved to the global queue, or the pool running
> count is decremented. A successful job finalizes and records its edge.

> [spec:samurai:def:build.jobstart-fn]
> static int jobstart(struct job *j, struct edge *e)

> [spec:samurai:sem:build.jobstart-fn]
> Counts the start, creates missing output directories, and writes any response
> file. It creates a close-on-exec pipe, stores the edge and expanded command in
> the job, prints status unless a console job is active, and spawns `/bin/sh -c`
> with pipe output except for the console pool, which inherits output. On
> success it stores the child and read descriptor, marks the job non-failed, and
> claims the console when applicable. Any setup or spawn failure closes opened
> descriptors, removes a temporary response file unless preservation is set,
> and returns -1; otherwise it returns the read descriptor.

> [spec:samurai:def:build.jobwork-fn]
> static bool jobwork(struct job *j)

> [spec:samurai:sem:build.jobwork-fn]
> Ensures at least half a standard I/O buffer of free capture space by growing
> the job buffer in standard-buffer increments. It reads from the child pipe:
> positive bytes extend the buffer and mean work remains; EOF finalizes the
> job. A read or reallocation error warns, sends SIGTERM to the child, marks it
> failed, finalizes it, and reports that no more work remains.

> [spec:samurai:def:build.nodedone-fn]
> static void nodedone(struct node *n, bool prune)

> [spec:samurai:sem:build.nodedone-fn]
> Visits every consumer edge scheduled in this build. It decrements that edge's
> prune count when the completed node is clean/pruned as required; when all
> relevant inputs are pruned, recursively prunes the edge outputs and removes
> its non-phony total from the status count. Otherwise it decrements the normal
> blocking count and queues the edge exactly when its final blocker completes.

> [spec:samurai:def:build.printstatus-fn]
> static void printstatus(struct edge *e, struct string *cmd)

> [spec:samurai:sem:build.printstatus-fn]
> Chooses the edge `description` unless verbose mode is set or that value is
> absent/empty, in which case it uses the command. It writes the expanded status
> prefix followed by the selected text and a newline to standard output.

> [spec:samurai:def:build.queryload-fn]
> static double queryload(void)

> [spec:samurai:sem:build.queryload-fn]
> Returns the one-minute system load average where supported. A failed query
> warns and returns 100.0 to suppress parallelism; unsupported platforms return
> zero.

> [spec:samurai:def:build.queue-fn]
> static void queue(struct edge *e)

> [spec:samurai:sem:build.queue-fn]
> Pushes an edge onto a LIFO work list. A non-phony edge with a bounded pool
> increments that pool's running count and goes to global work when below the
> limit; at the limit it instead joins the pool's private waiting list.

> [spec:samurai:def:build.shouldprune-fn]
> static bool shouldprune(struct edge *e, struct node *n, int64_t old)

> [spec:samurai:sem:build.shouldprune-fn]
> Returns false if the output mtime changed during the command. If unchanged,
> stats every non-order-only input, finds the newest extant one, stores that
> mtime as the output log mtime when present, and returns true so restat may
> prune dependent work.
