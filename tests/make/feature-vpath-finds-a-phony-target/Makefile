# A phony name is a target, so the search answers with it, and what the
# dependent reads is the found path.
vpath %.te d/
.PHONY: d/fail.te
all: fail.te
	@printf 'all from %s\n' '$<' > seen
