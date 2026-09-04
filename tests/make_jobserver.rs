//! `-j` bounds the whole Make tree, and the tree reaches past this process.
//!
//! Every case here measures peak concurrent recipes rather than wall time,
//! because what is under test is how many processes were allowed to exist at
//! once — a number a loaded host cannot move. Split from `tests/cli.rs`
//! because the group needs a real tree, a real second process and, for the
//! mixed-tool pairings, a real GNU Make, and none of that belongs beside the
//! command-line surface.
//!
//! [spec:ronin:req:make.jobserver+3]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[path = "support/make_mode.rs"]
mod make_mode;

use make_mode::{invoked_as, make_command, peak_concurrency, test_directory};

#[cfg(all(unix, feature = "make"))]
// [spec:ronin:req:make.jobserver+3/test]
// [spec:ronin:req:make.recursive-invocation+3/test]
#[test]
fn recursive_make_tree_uses_one_budget() {
    const LEVELS: [&str; 3] = ["a", "b", "c"];
    const UNITS: usize = 6;
    const BUDGETS: [usize; 3] = [1, 2, 4];

    let directory = test_directory("make-recursive-budget");
    let served = directory.join("jobservers");
    fs::create_dir_all(&served).unwrap();
    let log = directory.join("units");
    let stamp = directory.join("unit.sh");
    // Each unit records whether a served budget was reachable while it ran,
    // which is the half of the claim that peak concurrency cannot make: GNU
    // Make publishes a jobserver whenever it has more than one slot to hand
    // out, and a tree composed into one graph must publish the same one so a
    // `$(MAKE)` nothing could compose — or a `cargo`, or a `cc -flto` — joins
    // it rather than starting a second.
    fs::write(
        &stamp,
        "#!/bin/sh\nset -- \"$TMPDIR\"/ronin-jobserver-*\n[ ! -e \"$1\" ] || printf 'JOBSERVER\\n' >> \"$LOG\"\nprintf 'S %s\\n' \"$(date +%s.%N)\" >> \"$LOG\"\nsleep 0.2\nprintf 'E %s\\n' \"$(date +%s.%N)\" >> \"$LOG\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&stamp, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    // The levels nest and only the deepest one has work, which is the shape a
    // generated build has. Levels side by side would not measure the budget's
    // reach: every level owns one implicit slot, so a tree whose work sits one
    // hop down runs `-j` recipes whether or not the budget arrived at all.
    // Nothing tells any level how many jobs it may run.
    let tree = |prefix: &str, recurse: &str| {
        let (deepest, delegating) = LEVELS.split_last().expect("the tree has levels");
        for (index, level) in delegating.iter().enumerate() {
            let next = LEVELS[index + 1];
            fs::write(
                directory.join(format!("{prefix}{level}.mk")),
                format!("all:\n\t@{recurse} -f {prefix}{next}.mk all\n.PHONY: all\n"),
            )
            .unwrap();
        }
        let units = (0..UNITS)
            .map(|unit| format!("{deepest}{unit}"))
            .collect::<Vec<_>>()
            .join(" ");
        fs::write(
            directory.join(format!("{prefix}{deepest}.mk")),
            format!(
                "all: {units}\n{units}:\n\t@{} $@\n.PHONY: all {units}\n",
                stamp.display()
            ),
        )
        .unwrap();
        format!(
            "all:\n\t@{recurse} -f {prefix}{}.mk all\n.PHONY: all\n",
            LEVELS[0]
        )
    };
    fs::write(directory.join("Makefile"), tree("", "$(MAKE)")).unwrap();

    let program = invoked_as(&directory, "make");
    let measure = |jobs: usize| {
        let _ = fs::remove_file(&log);
        let output = make_command(&program, &directory)
            .arg(format!("-j{jobs}"))
            .env("LOG", &log)
            .env("TMPDIR", &served)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let events = fs::read_to_string(&log).unwrap();
        let served = events.lines().any(|line| line == "JOBSERVER");
        let (peak, units) = peak_concurrency(&events);
        assert_eq!(units, UNITS);
        (peak, served)
    };

    // Exactly `-j`, not at most it. The ceiling alone is met by a tree that
    // runs one recipe at a time, which is what a budget reaching nobody looks
    // like; the whole point of sharing it is that it is also spent.
    //
    // And a budget of more than one slot is published while it is spent, which
    // is where GNU Make draws the same line: `job_slots > 1 && jobserver_setup`
    // (main.c), so `-j1` stands up nothing and everything above it does.
    for jobs in BUDGETS {
        let (peak, published) = measure(jobs);
        assert_eq!(
            peak, jobs,
            "-j{jobs} ran {peak} recipes of a recursive Makefile tree at once"
        );
        assert_eq!(
            published,
            jobs > 1,
            "-j{jobs} published a jobserver where GNU Make {} one",
            if jobs > 1 {
                "publishes"
            } else {
                "publishes no"
            }
        );
    }
    // And takes it away again: the fifo belongs to the run that made it.
    assert_eq!(fs::read_dir(&served).unwrap().count(), 0);
}

/// A GNU Make this host can be measured against, if it has one.
///
/// The pinned oracle first, because a gate that has run has built it and its
/// version is the one every other Make answer here is checked against. A host
/// `make` will do otherwise — every GNU Make since 3.81 speaks the protocol —
/// and a host with neither leaves the mixed-tool pairings unmeasured rather
/// than failing over a tool that is not this repository's.
#[cfg(all(unix, feature = "make"))]
fn a_gnu_make() -> Option<PathBuf> {
    let oracle = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("reference/make-oracle/make-4.4.1")
        .join("make");
    if oracle.is_file() {
        return Some(oracle);
    }
    let host = PathBuf::from("make");
    Command::new(&host)
        .arg("--version")
        .output()
        .ok()
        .filter(|version| version.stdout.starts_with(b"GNU Make"))
        .map(|_| host)
}

/// The tree [`an_unlifted_recursion_draws_on_the_shared_budget`]
/// measures, written into `directory`.
///
/// `if true; then ...; fi` is the point of the shape: the recursion is a
/// branch of a shell script, so neither Make can see a child to compose and
/// both must start one. `$(L2)` and `$(L3)` name the program each level
/// recurses with, so one tree measures every pairing. `+` marks the line
/// recursive for GNU Make, which is what a makefile writing anything but a
/// literal `$(MAKE)` has to do.
#[cfg(all(unix, feature = "make"))]
fn write_unlifted_tree(directory: &std::path::Path, units: usize, cut: &str) {
    let stamp = directory.join("unit.sh");
    fs::write(
        &stamp,
        "#!/bin/sh\nprintf 'S %s\\n' \"$(date +%s.%N)\" >> \"$LOG\"\nsleep 0.2\nprintf 'E %s\\n' \"$(date +%s.%N)\" >> \"$LOG\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&stamp, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let names = |level: &str| {
        (0..units)
            .map(|unit| format!("{level}{unit}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let leaf = |level: &str| {
        format!(
            "all: {0}\n{0}:\n\t@{1} $@\n.PHONY: all {0}\n",
            names(level),
            stamp.display()
        )
    };
    let branch = |level: &str, next: &str, makefile: &str, budget: &str| {
        format!(
            "all: recurse {0}\nrecurse:\n\t+@if true; then {budget}$({next}) \
             --no-print-directory -f {makefile} all; fi\n{0}:\n\t@{1} $@\n\
             .PHONY: all recurse {0}\n",
            names(level),
            stamp.display()
        )
    };
    for (name, contents) in [
        ("two.mk", branch("p", "L2", "bottom.mk", "")),
        ("bottom.mk", leaf("c")),
        ("three.mk", branch("p", "L2", "middle.mk", "")),
        ("middle.mk", branch("c", "L3", "deepest.mk", "")),
        ("deepest.mk", leaf("g")),
        // The control: the same tree with the address withheld from the child,
        // which is then told the width and nothing to spend it against.
        ("cut.mk", branch("p", "L2", "bottom.mk", cut)),
    ] {
        fs::write(directory.join(name), contents).unwrap();
    }
}

/// A recursion nothing could compose still draws on the budget it was given,
/// and so does a GNU Make on either side of one.
///
/// The composed case is [`recursive_make_tree_uses_one_budget`]: every
/// `$(MAKE)` there becomes part of one graph and one scheduler bounds the lot.
/// This is the other half — a recursion wrapped in shell syntax that no
/// compiler can lift, so a real Make process starts and the only thing holding
/// it to the budget is the jobserver address in the `MAKEFLAGS` it was handed.
///
/// `-j2` throughout, because that is the width at which a shared budget and a
/// doubled one are one number apart: the parent spends one of its two slots on
/// the child process itself, leaving one for its own recipes beside whatever
/// the child runs. Three is what two budgets look like, and the control at the
/// end reaches three by having the address taken away — without it the
/// measurement would be a tautology about a tree that was never wide enough.
#[cfg(all(unix, feature = "make"))]
// [spec:ronin:req:make.jobserver+3/test]
#[test]
fn an_unlifted_recursion_draws_on_the_shared_budget() {
    const JOBS: usize = 2;
    const UNITS: usize = 5;

    let directory = test_directory("make-unlifted-budget");
    // Both names a jobserver client reads have to go for the control, not just
    // Make's: `CARGO_MAKEFLAGS` is the one the Rust ecosystem looks at first.
    write_unlifted_tree(
        &directory,
        UNITS,
        &format!("env MAKEFLAGS=-j{JOBS} MFLAGS=-j{JOBS} CARGO_MAKEFLAGS= "),
    );

    let ronin = invoked_as(&directory, "make");
    let log = directory.join("units");
    let measure = |makefile: &str, top: &std::path::Path, levels: &[(&str, &std::path::Path)]| {
        let _ = fs::remove_file(&log);
        let mut command = make_command(top, &directory);
        command
            .arg(format!("-j{JOBS}"))
            .arg("--no-print-directory")
            .arg("-f")
            .arg(makefile)
            .env("LOG", &log);
        for (name, program) in levels {
            command.env(name, program);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{makefile}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        peak_concurrency(&fs::read_to_string(&log).unwrap())
    };

    // Ronin above Ronin. The child is a second process with a scheduler of its
    // own, and the budget is what stops the two schedulers from adding up.
    let (peak, units) = measure("two.mk", &ronin, &[("L2", ronin.as_path())]);
    assert_eq!(units, UNITS * 2);
    assert_eq!(peak, JOBS, "an unlifted Ronin child brought its own budget");

    // And the control: the same tree, the same two processes, the address
    // taken out of what the child is told. Three is what the defect looked
    // like, and reaching it here is what makes the two above evidence.
    let (unshared, units) = measure("cut.mk", &ronin, &[("L2", ronin.as_path())]);
    assert_eq!(units, UNITS * 2);
    assert!(
        unshared > JOBS,
        "the control ran {unshared} at once, so the tree is not wide enough to measure"
    );

    let Some(gnu) = a_gnu_make() else {
        return;
    };
    // GNU Make under Ronin: it is entitled to a jobserver, and `.FEATURES`
    // has always said Ronin has one.
    let (peak, units) = measure("two.mk", &ronin, &[("L2", gnu.as_path())]);
    assert_eq!(units, UNITS * 2);
    assert_eq!(peak, JOBS, "a GNU Make under Ronin was offered no budget");

    // Ronin under GNU Make: the direction that already worked, kept working.
    let (peak, units) = measure("two.mk", &gnu, &[("L2", ronin.as_path())]);
    assert_eq!(units, UNITS * 2);
    assert_eq!(peak, JOBS, "Ronin did not join the budget GNU Make offered");

    // And the direction neither of those catches: a budget that reaches Ronin
    // has to reach past it too, or a well-behaved tool in the middle doubles
    // what the tool above it asked for.
    let (peak, units) = measure(
        "three.mk",
        &gnu,
        &[("L2", ronin.as_path()), ("L3", gnu.as_path())],
    );
    assert_eq!(units, UNITS * 3);
    assert_eq!(peak, JOBS, "the budget Ronin joined stopped at Ronin");
}

/// A `-j` written on a recursive recipe line founds a budget of its own, which
/// every level below it joins.
///
/// GNU Make's `-j%d forced in submake` (main.c:1855): a Make handed a jobserver
/// address AND a `-j` of its own on its command line leaves the group it was
/// given and masters a new one, whatever the two numbers are. Measured on the
/// oracle over this exact tree: `-j2` at the root and `-j4` on the first
/// recursion runs six units four at a time.
///
/// Ronin reaches the same number without a second scheduler. The budget is one
/// pool that grew to the widest any unit asked for, and the levels that asked
/// for nothing are held to the run's own by a pool of their own.
#[cfg(all(unix, feature = "make"))]
// [spec:ronin:req:make.jobserver+3/test]
#[test]
fn a_typed_child_jobs_founds_a_new_budget() {
    const LEVELS: [&str; 3] = ["a", "b", "c"];
    const UNITS: usize = 6;
    /// Different from the root limit, so the two budgets are distinguishable.
    const FORCED: usize = 4;

    let directory = test_directory("make-forced-budget");
    let served = directory.join("jobservers");
    fs::create_dir_all(&served).unwrap();
    let log = directory.join("units");
    let stamp = directory.join("unit.sh");
    fs::write(
        &stamp,
        "#!/bin/sh\nprintf 'S %s\\n' \"$(date +%s.%N)\" >> \"$LOG\"\nsleep 0.2\nprintf 'E %s\\n' \"$(date +%s.%N)\" >> \"$LOG\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&stamp, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    // The forcing level is the first, and the work is two hops below it, so
    // what is measured is the budget reaching the bottom rather than the
    // forcing level spending its own implicit slot.
    fs::write(
        directory.join("Makefile"),
        format!(
            "all:\n\t@$(MAKE) -j{FORCED} -f {}.mk all\n.PHONY: all\n",
            LEVELS[0]
        ),
    )
    .unwrap();
    let (deepest, delegating) = LEVELS.split_last().expect("the tree has levels");
    for (index, level) in delegating.iter().enumerate() {
        fs::write(
            directory.join(format!("{level}.mk")),
            format!(
                "all:\n\t@$(MAKE) -f {}.mk all\n.PHONY: all\n",
                LEVELS[index + 1]
            ),
        )
        .unwrap();
    }
    let units = (0..UNITS)
        .map(|unit| format!("{deepest}{unit}"))
        .collect::<Vec<_>>()
        .join(" ");
    fs::write(
        directory.join(format!("{deepest}.mk")),
        format!(
            "all: {units}\n{units}:\n\t@{} $@\n.PHONY: all {units}\n",
            stamp.display()
        ),
    )
    .unwrap();

    let program = invoked_as(&directory, "make");
    let output = make_command(&program, &directory)
        .arg("-j2")
        .env("LOG", &log)
        .env("TMPDIR", &served)
        .output()
        .unwrap();
    // Both streams: the forcing level is a recipe, and a recipe's diagnostics
    // reach the run through the captured output its parent replays.
    let said = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{said}");
    let (peak, ran) = peak_concurrency(&fs::read_to_string(&log).unwrap());
    assert_eq!(ran, UNITS);
    assert_eq!(
        peak, FORCED,
        "a -j{FORCED} recursion under -j2 ran {peak} at once where GNU Make runs {FORCED}"
    );
    // The behaviour without the warning: the seven jobserver lines GNU Make
    // says are narration Ronin does not repeat.
    assert!(!said.contains("resetting jobserver mode"), "{said}");

    assert_eq!(fs::read_dir(&served).unwrap().count(), 0);
}

/// A makefile's own `MAKEFLAGS += -jN` resizes the budget its unit runs at,
/// whichever way it moves it.
///
/// GNU Make's `-j%d forced in makefile` (main.c:2107). Both directions are
/// here because they fail differently: a budget that will not GROW costs wall
/// time — a sub-makefile asking for four runs at the parent's two — and one
/// that will not NARROW is a correctness defect, because a makefile asking to
/// be run one recipe at a time is asking for something it needs.
///
/// Measured rather than timed. Each recipe writes when it starts and when it
/// stops, and what is asserted is how many overlapped; a loaded host moves the
/// wall clock and cannot move that.
#[cfg(all(unix, feature = "make"))]
// [spec:ronin:req:make.jobserver+3/test]
#[test]
fn a_makefiles_own_jobs_resize_the_budget() {
    const UNITS: usize = 4;

    let directory = test_directory("make-makefile-budget");
    let served = directory.join("jobservers");
    fs::create_dir_all(&served).unwrap();
    let log = directory.join("units");
    let stamp = directory.join("unit.sh");
    fs::write(
        &stamp,
        "#!/bin/sh\nprintf 'S %s\\n' \"$(date +%s.%N)\" >> \"$LOG\"\nsleep 0.2\nprintf 'E %s\\n' \"$(date +%s.%N)\" >> \"$LOG\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&stamp, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    // The work is in the sub-makefile and the recursion carries no `-j` of its
    // own, so the only thing that can move the number is the line the child's
    // own makefile wrote.
    fs::write(
        directory.join("Makefile"),
        "all:\n\t@$(MAKE) --no-print-directory -f sub.mk all\n.PHONY: all\n",
    )
    .unwrap();
    let units = (0..UNITS)
        .map(|unit| format!("u{unit}"))
        .collect::<Vec<_>>()
        .join(" ");

    let program = invoked_as(&directory, "make");
    let measure = |root: usize, forced: usize| {
        fs::write(
            directory.join("sub.mk"),
            format!(
                "MAKEFLAGS += -j{forced}\nall: {units}\n{units}:\n\t@{} $@\n.PHONY: all {units}\n",
                stamp.display()
            ),
        )
        .unwrap();
        let _ = fs::remove_file(&log);
        let output = make_command(&program, &directory)
            .arg(format!("-j{root}"))
            .env("LOG", &log)
            .env("TMPDIR", &served)
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{said}");
        assert!(!said.contains("forced in makefile"), "{said}");
        let (peak, ran) = peak_concurrency(&fs::read_to_string(&log).unwrap());
        assert_eq!(ran, UNITS);
        peak
    };

    // Wider than the budget that already exists: the pool the run published
    // grows to four at the address every recipe already carries, rather than a
    // second pool being stood up at a second address nothing was compiled with.
    assert_eq!(
        measure(2, 4),
        4,
        "a sub-makefile's -j4 ran at the parent's 2"
    );
    // Narrower than it, which the same defect got wrong in the direction that
    // breaks builds rather than slowing them.
    assert_eq!(
        measure(4, 1),
        1,
        "a sub-makefile's -j1 ran 4 recipes at once"
    );
    assert_eq!(
        measure(4, 2),
        2,
        "a sub-makefile's -j2 ran 4 recipes at once"
    );
    // And a run nothing asked to resize is the run it was invoked as.
    assert_eq!(measure(4, 4), 4);

    assert_eq!(fs::read_dir(&served).unwrap().count(), 0);
}
