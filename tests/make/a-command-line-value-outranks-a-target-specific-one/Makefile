# `record_target_var` (read.c) ends by asking whether the name was set on the
# command line, and if it was, the target-specific binding takes that value,
# origin and flavour and drops its own append: "If it's not an override, check
# to see if there was a command-line setting.  If so, reset the value."  A
# command-line definition outranks a target's own, exactly as it outranks a
# makefile's — and an `override` on the rule line is what takes it back.
#
#   one   `override FOO = one` beats the `FOO=1` in `args`.
#   two   a plain `BAR = two` does not beat `BAR=2`.
#   plus  neither does a `+=`: the append is dropped whole rather than pasted
#         onto the command line's value, because `append` is cleared with it.
#   patb  a PATTERN-specific assignment is the same question — the check sits
#         after the pattern/file split and covers both.
#   own   the control: `OWN` is named nowhere on the command line, so the
#         target's own assignment is what its recipe reads.
#
# Each target writes its own file, so what the build left does not depend on
# the order two independent recipes ran in.
export FOO = foo
export BAR = bar
OWN = global

one: override FOO = one
two: BAR = two
plus: BAR += appended
%b: PAT = pattern
own: OWN = mine

one two plus patb own: ; @printf '[%s][%s][%s][%s]\n' '$(FOO)' '$(BAR)' '$(PAT)' '$(OWN)' > $@.out

.PHONY: one two plus patb own
