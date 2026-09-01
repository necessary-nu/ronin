# Why the eight Ninja rows read adverse — 2026-09-02

`gate-statistic-2026-09-01.md` closed with a finding it deliberately declined to
act on: **seven of the eight Ninja rows read 5% to 13% adverse against
[`baseline-v1.csv`](baseline-v1.csv) on the same median statistic the record was
made with**, `scheduler-barrier` worst. Two explanations were live and the node
refused to choose between them without measuring — a slow Ninja-mode regression
accumulated under the Make campaign's twenty-five binary-level landings while
the Ninja gate was flapping ~50% and nobody was running it, or a host that had
drifted away from the quiet one the record was taken on.

**It is the host, and the record's own revision proves it.** A smaller, real,
separately-attributed campaign cost sits on top of it, and it is not a
regression in a hot path.

**Nothing is re-recorded.** The reason is in the last section and it is not the
one the previous node had.

## Provenance

- Ronin `032c179`, kati `b2ed08c`; comparison arm built from `3e6b947`, the
  revision [`performance-validation-2026-08-10.md`](performance-validation-2026-08-10.md)
  records the rows at, with kati at that commit's own gitlink `27b868f`
- Both arms: release, `codegen-units = 1`, `lto = "fat"`, `+crt-static`,
  static-pie, `rustc 1.97.1` — verified identical profile and linkage, so the
  arms differ by their source and nothing else
- Oracle: pinned Ninja `b51a1e37c2fb89bbefa600bd155e1ce13983f09d`, CMake Release
- Platform: Linux `6.12.100+deb13-amd64`, x86_64, 32 cores — **the same kernel
  string the record carries**, so no kernel update is available as an excuse
- One-minute load **12.9 to 19.3** throughout, floor held by four runaway `nsh`
  processes belonging to another tenant and the rest by a foreign Rust build
  campaign. `--max-load` was raised deliberately to 25–60; every number below is
  a RATIO of two or three tools sampled interleaved into the same rotation, and
  the milliseconds beside them are not quoted as results.
- Workloads: `examples/support/workloads.rs` is **byte-identical across the
  whole 580-commit span** from the record to `HEAD`, so both arms face the same
  work and the comparison needs no allowance.
- The benchmark tree is built under `/tmp`, which is tmpfs with 13 GiB free. The
  root filesystem being 98% full — one of the host causes on the table — cannot
  reach these workloads and is not one.

## The drift is real, and it is on every row

`HEAD` against pinned Ninja, three windows, median statistic so it is
like-for-like with the record. `A` is a two-way pool at 151 repetitions; `B` and
`C` are three-way pools at 151 which also carry the `3e6b947` arm, with the
slots swapped between them.

| Workload | recorded | A | B | C |
| --- | ---: | ---: | ---: | ---: |
| `manifest-command-evaluation` | 0.4918 | 1.0697 | 1.1130 | 1.1023 |
| `deep-graph-evaluation` | 0.7212 | 1.0150 | 1.1259 | 1.0918 |
| `wide-noop-build` | 0.8178 | 1.0950 | 1.1091 | 1.1960 |
| `path-canonicalization` | 0.5851 | 1.0792 | 1.0789 | 1.0681 |
| `dependency-log-load` | 0.7282 | 1.0241 | 1.1212 | 1.1339 |
| `scheduler-barrier` | 1.0158 | **1.1570** | **1.1608** | **1.1567** |
| `clean-tree-noop` | 0.6461 | 1.0385 | 1.0035 | 0.9951 |
| `large-manifest-parse` | 0.5135 | 1.0126 | 1.0367 | 1.0580 |
| **mean over rows** | | | **1.0851** | |

The finding reproduces and is if anything understated: all eight rows read
adverse, not seven, and the mean adverse move is 8.5%.

## It is the host, and this is how that was settled

The question is not answerable by measuring `HEAD` however carefully, because
every measurement of `HEAD` is also a measurement of today's machine. So the
record's own tree was rebuilt and run **beside** today's, both against the same
Ninja, interleaved in one rotation so the host cannot favour one.

`3e6b947` — the revision the recorded rows were taken from — against its own
record, today:

| Workload | recorded | B | C | L0 |
| --- | ---: | ---: | ---: | ---: |
| `manifest-command-evaluation` | 0.4918 | 1.1005 | 1.0722 | 1.0742 |
| `deep-graph-evaluation` | 0.7212 | 1.0834 | 1.0710 | 1.0523 |
| `wide-noop-build` | 0.8178 | 1.0698 | 1.0845 | 0.9216 |
| `path-canonicalization` | 0.5851 | 1.0637 | 1.1154 | 0.9635 |
| `dependency-log-load` | 0.7282 | 1.0828 | 1.0684 | 1.0745 |
| `scheduler-barrier` | 1.0158 | **1.1615** | **1.1182** | **1.1244** |
| `clean-tree-noop` | 0.6461 | 0.9273 | 0.9502 | 1.0677 |
| `large-manifest-parse` | 0.5135 | 1.0572 | 1.0252 | 1.0388 |
| **mean over rows** | | | **1.0570** | |

**The binary that produced the record misses the record by 5.7% on average and
by up to 16%, on seven of eight rows, in three windows.** That is the finding.
`L0` is an independent two-way pool at 101 repetitions taken in its own window;
`B` and `C` swapped the arm between the first and third slot of the rotation and
agree, so it is not a slot artefact.

`scheduler-barrier` — the row the drift is sharpest on, and the one a
scheduler-weight change (`c4441b2`) was the obvious suspect for — is **host in
full**: 1.1615 / 1.1182 / 1.1244 from the baseline's own binary against 1.1570 /
1.1608 / 1.1567 from today's. There is nothing left for a code change to
explain, and the reading that node's log recorded — 1.109×, called host — was
right.

## The mechanism, which is why the ratio is not host-immune

Normalizing against Ninja was believed to make the comparison portable across
differently loaded hosts. It does not, and `scheduler-barrier` shows why.
Twenty-one interleaved rounds on the 128-edge `-j8` fixture, whole process tree,
all three arms rotating in one window at load ~15:

| Counter | `3e6b947` | `032c179` | pinned Ninja | old/nj | new/nj |
| --- | ---: | ---: | ---: | ---: | ---: |
| `instructions:u` | 43,572,671 | 43,943,824 | 61,136,786 | 0.713 | 0.719 |
| `cycles:u` | 93,021,181 | 98,178,894 | 143,461,031 | 0.648 | 0.684 |
| `cycles:k` | 387,127,512 | 400,010,844 | 738,573,786 | 0.524 | 0.542 |
| `task-clock` (ms) | 192.3 | 196.3 | 383.4 | 0.501 | 0.512 |
| `context-switches` | 137 | 138 | 418 | 0.328 | 0.330 |
| `page-faults` | 8,922 | 8,952 | 17,576 | 0.508 | 0.509 |
| **CPUs utilised** | **2.63** | **2.61** | **5.46** | | |

**Ronin does half of Ninja's work on this row and loses 15% of the wall anyway.**
Both tools were asked for eight jobs; Ninja keeps 5.46 cores busy and Ronin
2.63. Ronin is wait-bound where Ninja is CPU-bound — it is not spending the
cycles, it is spending the latency.

That is exactly the shape whose ratio moves with the host. A tool that idles
between wake-ups pays the run queue's depth on every one of them; a tool holding
cores does not give them back to pay anything. So as load rises the wait-bound
tool loses ground and the ratio climbs **while both binaries stand still** —
which is what the two tables above measure from either end, one at 99% idle
three weeks ago and one at load 12–19 today.

Both Ronin arms sit at 2.63 and 2.61, so the under-filled job budget is
long-standing and is not something the campaign did. It is filed as its own
finding; it is a Ninja-mode opportunity, not a regression.

The dose-response — ratio against load on one binary — is the measurement this
section would rather rest on and it could not be taken. The load floor is four
foreign runaway processes that must not be killed, so the host cannot be made
quieter, and raising it synthetically was refused on a shared machine. What is
here is the same binary at two host states and a mechanism that predicts the
direction, which is weaker than a curve and stronger than an assertion.

## The campaign's real cost, which is separate and is not a hot path

Both three-way windows also measure the two Ronin arms directly against each
other. Ninja cancels, so this is `HEAD`/`3e6b947` at the tenth percentile:

| Workload | B (`HEAD` slot 1) | C (`HEAD` slot 3) |
| --- | ---: | ---: |
| `manifest-command-evaluation` | 1.0323 | 1.0252 |
| `deep-graph-evaluation` | 1.0624 | 1.0502 |
| `wide-noop-build` | 1.0112 | 1.0533 |
| `path-canonicalization` | 0.9994 | 1.0329 |
| `dependency-log-load` | 1.0568 | 0.9989 |
| `scheduler-barrier` | 1.0140 | 1.0338 |
| `clean-tree-noop` | 1.0448 | 1.0762 |
| `large-manifest-parse` | 0.9985 | 1.0169 |
| **mean** | **1.0274** | **1.0359** |

**The Make campaign has cost Ninja mode about 3%**, consistently signed across
two slot-swapped windows. Set against the 5.7% the host takes, that is the
smaller half of the 8.5%, and 1.057 × 1.032 = 1.091 recovers it.

It is not a regression in a hot path. `HEAD` built `--no-default-features` — the
same source with the Make front end left out — is a 4,178,128-byte binary
against the full build's 6,455,584 and the record's 3,953,080, and run beside
the full build against the same Ninja at 101 repetitions it recovers essentially
all of the gap:

| Workload | ninja-only / full, p10 |
| --- | ---: |
| `manifest-command-evaluation` | 0.9825 |
| `deep-graph-evaluation` | 0.9924 |
| `wide-noop-build` | 0.9719 |
| `path-canonicalization` | 0.9534 |
| `dependency-log-load` | 0.9892 |
| `scheduler-barrier` | 0.9748 |
| `clean-tree-noop` | 0.9954 |
| `large-manifest-parse` | 0.9804 |
| **mean** | **0.9800** |

Every row improves. The cost is the Make front end's object code sitting in the
binary Ninja mode runs — a 63% larger image whose startup these 4–11 ms rows pay
for — and not a branch the campaign put in a Ninja path. That is a product
question about what one binary should contain, and it belongs to whoever wants
to answer it; it is not a defect to fix inside this node.

## The control: the Make gate passes on the same host in the same hour

If today's machine were simply bad at everything, both gates would say so. Run
within an hour of the refusals below, at load 16.3, `--max-load 25`:

| Row | this run | recorded |
| --- | ---: | ---: |
| `wide-noop` | 0.62× | 0.62× |
| `recursive-noop` | 0.93× | 1.01× |
| `vim-noop` | 1.59× | 1.71× |
| `zsh-incremental` | 1.05× | 1.03× |

**Every Make row is at or better than its record, and the gate passes.** Same
host, same hour, same tenth-percentile statistic, same 1.20 tolerance, the same
raised guard.

That is the strongest single piece of evidence here, because it rules out the
lazy version of "it's the host". The host has not become generally slower; what
has happened is specific to the Ronin/Ninja *pairing*. GNU Make spawns processes
and waits on them much as Ronin does, so the two move together under contention
and their ratio holds. Pinned Ninja does not — it holds cores where Ronin waits
— so that ratio moves. A ratio is host-immune exactly when the two tools respond
to the host the same way, and these two do not.

## The Ninja gate is refusing right now, on an unmodified binary

Four consecutive `--validate` runs at load 17–19, guard raised to 25 and 30
because the gate's own 4.00 would refuse to measure at all:

| Run | load before | load after | worst row | worst p10/recorded |
| --- | ---: | ---: | --- | ---: |
| 1 | 17.59 | 19.45 | `manifest-command-evaluation` | 1.2672 |
| 2 | 18.26 | 17.45 | `dependency-log-load` | 1.2623 |
| 3 | 17.45 | 17.22 | `clean-tree-noop` | 1.2687 |
| 4 | 17.22 | 17.12 | `deep-graph-evaluation` | 1.2494 |

**Four refusals, four different rows.** A regression refuses the same row every
time; only noise moves the name around. `src/` and `kati` are untouched by this
node, so the binary under test is the same code that passed the gate earlier in
the session.

And it did pass earlier. The 151-repetition pool `A` taken at load 12.9 puts the
worst row at 1.1296 — inside the tolerance with room. The same tree, the same
statistic, the same day: **inside at load 13, refused four times over at load
17–19.** That is the dose-response this note could not obtain by adding load
deliberately, arriving on its own.

It is also the honest state of the gate to hand on: with 8.5% of the 20% margin
already spent — 5.7% of it host, 3% of it campaign — the remaining headroom is
smaller than this host's own load excursions, so the Ninja gate cannot currently
return a trustworthy verdict here at all. That is what the 4.00 guard is for,
and it is the guard, not the statistic, that is doing the refusing now.

## Where the margin stands

`HEAD` measured properly — 151 repetitions at load 12.9, the gate's own
tenth-percentile statistic against the recorded ratios, worst row first:
`scheduler-barrier` 1.1296, `wide-noop-build` 1.1013, `deep-graph-evaluation`
1.0929, `manifest-command-evaluation` 1.0885, `path-canonicalization` 1.0721,
`dependency-log-load` 1.0678, `clean-tree-noop` 1.0451, `large-manifest-parse`
1.0066 — all inside 1.20, and the absolute and peak-RSS ceilings pass too.

**That leaves about 6% of the margin on `scheduler-barrier`**, on the row whose
ratio is the one that moves with the host. That is the number worth carrying
forward. Six points is less than the spread between load 13 and load 18 on this
machine, which is why the section above shows the same tree passing and then
refusing four times inside an hour. The threshold has not stopped meaning
something, but it now means "this host was quiet enough" at least as much as it
means "this tree is fast enough", and the two cannot be told apart from a red
result alone.

## Why the rows are still not re-recorded

The previous node declined to re-record because the drift was unattributed. It
is attributed now and they are still not re-recorded, for a better reason.

A record is a claim about a tree. The ratio on these rows is measurably also a
claim about the machine — that is this note's whole middle section — and this
host has not been under load 12 at any point in this work. Re-recording here
would write today's contention into `baseline-v1.csv` and call it Ronin, and the
gate would then be loose by however much the host happened to be busy on the
afternoon it was written. It would also bank the campaign's 3% as normal, which
is the one part of the drift that is genuinely the tree's.

The protocol's answer is the right one and it has not changed: re-record on a
host the 4.00 guard admits. The guard exists precisely to refuse the window this
work had to run in, and every measurement above states the load it was taken at
so that a later reader can discount it correctly.
