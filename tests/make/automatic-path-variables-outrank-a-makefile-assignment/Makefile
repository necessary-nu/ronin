# An automatic variable sits at the top of GNU Make's origin ladder, so a
# global assignment, an `override`, an `undefine` and an `+=` all bounce off
# `@D` and leave it reading `automatic` with its startup text. Nothing is
# printed when they do: GNU Make declines them silently.
#
# A target-specific assignment is a different question and does take, because
# it is written into the rule's own variable set, which is searched before the
# global one. The base form is not defined at read time at all, so assigning
# `@` there makes an ordinary file variable that the per-file automatic then
# outranks once a rule is chosen.
@D = global assignment
override @F = override directive
undefine %D
<D += appended
@ = read time

$(file >out,read @D origin=[$(origin @D)] value=[$(value @D)])
$(file >>out,read @F origin=[$(origin @F)] value=[$(value @F)])
$(file >>out,read %D origin=[$(origin %D)] value=[$(value %D)])
$(file >>out,read <D origin=[$(origin <D)] value=[$(value <D)])
$(file >>out,read @ origin=[$(origin @)] value=[$(value @)] expand=[$(@)])

.PHONY: all
all: sub/a.o ; @:

sub/a.o: @D = target specific
sub/a.o:
	@mkdir -p sub
	@printf '%s\n' 'recipe @D [$(@D)] origin=[$(origin @D)]' \
		'recipe @F [$(@F)]' 'recipe @ [$@] origin=[$(origin @)]' >> out
	@: > $@
