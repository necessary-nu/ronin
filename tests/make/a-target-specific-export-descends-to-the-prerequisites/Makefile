# A target-specific binding supplies the VALUE a recipe reads. Whether that
# value reaches the recipe's ENVIRONMENT is a separate answer, and GNU Make
# takes it from the innermost scope that actually gave one:
# `target_environment` walks the file's whole set list from most specific
# outward, keeps the first definition it meets for the value, and lets each set
# further out fill in the export attribute while it is still unanswered.
#
# So the decision descends the prerequisite chain along with the value. `base`
# supplies its own `hello=base` and says nothing about exporting, so it takes
# `mid`'s answer — `export` here, `unexport` in the pair below. Carrying only
# the value across gets it backwards in BOTH directions: the exported chain
# leaves `base`'s value out of the environment, and the unexported one puts it
# in.
#
# `$(hello)` is what Make read and `$$hello` is what the recipe's shell saw;
# the two together are what separates the value from the flag.

all: exported unexported ; @: > all.done

exported: emid ; @printf 'exported make=[%s] shell=[%s]\n' '$(hello)' "$$hello" > exported.out
emid: ebase ; @printf 'emid make=[%s] shell=[%s]\n' '$(hello)' "$$hello" > emid.out
ebase:        ; @printf 'ebase make=[%s] shell=[%s]\n' '$(hello)' "$$hello" > ebase.out
emid: export hello=mid
ebase: hello=base

unexported: umid ; @printf 'unexported make=[%s] shell=[%s]\n' '$(hello)' "$$hello" > unexported.out
umid: ubase ; @printf 'umid make=[%s] shell=[%s]\n' '$(hello)' "$$hello" > umid.out
ubase:        ; @printf 'ubase make=[%s] shell=[%s]\n' '$(hello)' "$$hello" > ubase.out
umid: unexport hello=mid
ubase: hello=base
