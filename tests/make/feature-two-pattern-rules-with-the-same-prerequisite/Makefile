# Both overlapping pattern rules can use the same existing prerequisite.
# Definition order chooses the first rule for %.elf; the requested %.dbg still
# selects the second rule, so the first rule's peer output must not hide it.
all: a.elf a.dbg

%.elf %.lnk: %.c ; @printf '%s\n' elf-and-lnk >> log; touch $*.elf $*.lnk

%.elf %.dbg: %.c ; @printf '%s\n' elf-and-dbg >> log; touch $*.elf $*.dbg
