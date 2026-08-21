# Where a `--eval` fragment lives, which is not inside MAKEFLAGS.
#
# GNU Make's `define_makeflags` writes the literal text `$(-*-eval-flags-*-)`
# where the fragments would go, beside the `$(MAKEOVERRIDES)` it writes where
# the command-line assignments would, and keeps the fragments in a variable of
# that odd name — `define_variable_cname ("-*-eval-flags-*-", value,
# o_automatic, 0)` in main.c. So `$(MAKEFLAGS)` and `$(value MAKEFLAGS)` are
# two different strings, and the second says what the first is made of.
#
# The variable is SIMPLE, and that is the load-bearing part: substituting a
# simple variable's bytes does not expand them again, so the `$` the fragment
# carries — doubled by the quoting that stored it — is halved exactly once on
# the way back out and `X` holds `a$b`. A literal in the variable's place
# would shield it exactly once too, which is why the first line below reads
# the same either way and why nothing but `$(value)` and the odd name itself
# can tell the two spellings apart.
#
# It is `automatic`, which outranks a makefile, so the two lines a makefile
# could use to interfere with it do nothing at all: see the sibling case.
$(file > read,mf=[$(MAKEFLAGS)])
$(file >> read,val=[$(value MAKEFLAGS)])
$(file >> read,ef=[$(-*-eval-flags-*-)] vef=[$(value -*-eval-flags-*-)])
$(file >> read,origin=[$(origin -*-eval-flags-*-)] flavor=[$(flavor -*-eval-flags-*-)])
$(file >> read,names=[$(filter -*-eval-flags-*- -*-command-variables-*-,$(.VARIABLES))])
$(file >> read,mfl=[$(MFLAGS)] ov=[$(MAKEOVERRIDES)] X=[$(X)])

all:
	@printf 'ran\n' > ran
