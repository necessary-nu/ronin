# The two ways a makefile could try to interfere with the fragments' variable,
# and what each is worth.
#
# `-*-eval-flags-*-` is defined at `o_automatic`, which outranks every origin a
# makefile can write at, so an assignment to the name is declined and an
# `undefine` of it is declined too — both leave the binding, and therefore
# `$(MAKEFLAGS)`, exactly as the invocation left them. The spelling keeps the
# name out of the way; the origin is what actually holds it.
-*-eval-flags-*- := mine
undefine -*-eval-flags-*-

$(file > read,ef=[$(-*-eval-flags-*-)] origin=[$(origin -*-eval-flags-*-)])
$(file >> read,mf=[$(MAKEFLAGS)] val=[$(value MAKEFLAGS)] X=[$(X)])

all:
	@printf 'ran\n' > ran
