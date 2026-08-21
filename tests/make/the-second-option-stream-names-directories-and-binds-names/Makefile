# Everything MAKEFLAGS can carry out of the environment, GNUMAKEFLAGS carries
# too, because GNU Make hands both to the same `decode_env_switches` at the
# same `o_command` origin.
#
# Two of them at once, which is what makes this more than a spelling: `-I`
# reaches the search path the `include` below is resolved against, and a word
# holding an `=` is a command-line assignment rather than a switch — the dash
# `decode_env_switches` prepends to a first word without one is withheld from
# exactly those. Both end up published as MAKEFLAGS, where the second stream's
# name is never mentioned again.
include fragment.mk

$(file > read,frag=[$(FOUND_THE_FRAGMENT)] foo=[$(FOO)] origin=[$(origin FOO)])
$(file >> read,mf=[$(MAKEFLAGS)] mfl=[$(MFLAGS)] ov=[$(MAKEOVERRIDES)])
$(file >> read,gmf=[$(GNUMAKEFLAGS)] origin=[$(origin GNUMAKEFLAGS)])

all:
	@printf 'ran\n' > ran
