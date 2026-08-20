# GNU Make's `define_makeflags` runs one `quote_for_env` over every switch
# argument — `p = quote_for_env (p, flags->arg)`, with no question about which
# switch it belongs to. A `$` is doubled and a backslash or a blank is escaped,
# so that reading MAKEFLAGS back as a command line finds the word it started as.
#
# Two switches carry arbitrary text and so are the only two where it shows:
# `-I` and `--debug`. `$(value MAKEFLAGS)` is that stored text verbatim, and
# `$(MFLAGS)` is the same switch table bound recursively on its own — GNU Make
# defines it from `flagstring` before the variable references are appended, so
# it reads back as the command line the table spells.
$(file > out,val=[$(value MAKEFLAGS)])
$(file >> out,mfl=[$(MFLAGS)])

all:
	@printf 'ran\n' > ran
