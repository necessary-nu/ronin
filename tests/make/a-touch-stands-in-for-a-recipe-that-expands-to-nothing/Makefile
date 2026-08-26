# `-t` touches a target that HAS a recipe and does not run it, whatever that
# recipe expands to. `b`'s comes to no command at all, which is the case
# GNU Make's own `notice_finished_file` answers with `file->cmds != 0` rather
# than by asking what the recipe produced.
.SUFFIXES:
NOTHING =
a: b
	@echo made > $@
b: c
	$(NOTHING)
