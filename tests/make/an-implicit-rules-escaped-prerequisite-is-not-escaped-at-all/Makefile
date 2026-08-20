# An implicit rule's prerequisite never goes through `enter_prereqs` at all —
# `record_files` skips it deliberately, "We don't want to enter pattern rules
# at all" — so `pattern_search` sees the text exactly as written and finds its
# `%` with `strchr (nptr, '%')`, which knows nothing about escaping. The
# backslash is a literal byte of the name being built, ahead of the stem.
#
# So `\%.c` stands for `\a.c` here, where the same text in a static pattern
# rule stands for the one file called `\%.c`, and `\\%.c` stands for `\\a.c`
# rather than for `\a.c`. Three cells, three different names on disk.
all: a.o a.q

%.o: \%.c
	@cat '$<' > $@

%.q: \\%.c
	@cat '$<' > $@
