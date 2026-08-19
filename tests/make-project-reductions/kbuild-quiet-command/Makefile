# kbuild compiles every object through one macro, and the macro puts the quiet
# line and the work on a single silenced line behind `set -e`:
#
#   echo-cmd = $(if $($(quiet)cmd_$(1)),echo '  $(call escsq,$($(quiet)cmd_$(1)))';)
#   cmd      = @set -e; $(echo-cmd) $(cmd_$(1))
#
# so what reaches the shell is `set -e; echo '  CC      fused.o'; cc …`, and
# the `@` is read off the expansion rather than off the makefile.
quiet = quiet_

quiet_cmd_cc_o_c = CC      $@
      cmd_cc_o_c = cp $< $@

echo-cmd = echo '  $($(quiet)cmd_$(1))';
cmd = @set -e; $(echo-cmd) $(cmd_$(1))

all: fused.o split.o

fused.o: fused.c
	$(call cmd,cc_o_c)

# The same intent written the other way round, which is what a build system
# that never grew a macro writes: the quiet line silenced on a line of its
# own, and the work under it.
split.o: split.c
	@echo '  CC      $@'
	@cp $< $@
