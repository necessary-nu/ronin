# The environment's second option stream, and what it is worth to a build.
#
# GNU Make decodes GNUMAKEFLAGS exactly as it decodes MAKEFLAGS, immediately
# before it, and then empties the name — `decode_env_switches (…
# GNUMAKEFLAGS_NAME …, o_command)` followed by `define_variable_cname
# (GNUMAKEFLAGS_NAME, "", o_env, 0)` in main.c. So the switches arrive, and
# what a Makefile reads under that name is nothing at all: everything they
# asked for has already been folded into MAKEFLAGS.
#
# The `-k` here is the whole point of the case. It is not a report: with the
# stream read, `good` still runs after `bad` fails and `kept-going` is written;
# without it the build stops at the first failure and the file is never there.
$(file > read,gmf=[$(GNUMAKEFLAGS)] origin=[$(origin GNUMAKEFLAGS)] flavor=[$(flavor GNUMAKEFLAGS)] value=[$(value GNUMAKEFLAGS)])
$(file >> read,mf=[$(MAKEFLAGS)] mfl=[$(MFLAGS)])

all: bad good

bad:
	@exit 1

good:
	@printf 'the second stream was read\n' > kept-going
