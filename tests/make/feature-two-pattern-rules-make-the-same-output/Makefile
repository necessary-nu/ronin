# A pattern rule with several targets is a rule that can make any one of them,
# not a rule that makes them together, so two such rules may both offer %.elf.
# Make chooses one of them per goal and builds; it does not refuse a graph in
# which one name has two possible makers.
.SECONDARY:

all: a.elf a.dbg

%.elf %.lnk: %.c ; @printf '%s\n' elf-and-lnk >> log; touch $*.elf $*.lnk

%.elf %.dbg: %.lnk ; @printf '%s\n' elf-and-dbg >> log; touch $*.elf $*.dbg
