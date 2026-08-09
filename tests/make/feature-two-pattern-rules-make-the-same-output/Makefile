# Two multi-target pattern rules may both offer %.elf. Make chooses one rule
# for each requested target; a peer output from one rule does not prevent the
# other rule from making its own requested target.
.SECONDARY: a.lnk

all: a.elf a.dbg

%.elf %.lnk: %.c ; @printf '%s\n' elf-and-lnk >> log; touch $*.elf $*.lnk

%.elf %.dbg: %.lnk ; @printf '%s\n' elf-and-dbg >> log; touch $*.elf $*.dbg
