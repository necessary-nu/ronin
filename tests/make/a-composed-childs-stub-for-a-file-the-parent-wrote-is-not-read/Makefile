# The other half of the same question. Here the enclosing unit's recipe really
# does write the file its target names, so GNU Make's child process stats
# `gen.txt`, finds it made, and never reaches the stub rule it holds for it —
# which is why zsh's every subdirectory can declare
# `$(dir_top)/Src/zsh.mdh: ; false` and still build. Read as a generator of the
# child's own private node the stub runs, and the build fails.
all: gen.txt
	@$(MAKE) --no-print-directory -f sub.mk out

gen.txt:
	@echo real > gen.txt
