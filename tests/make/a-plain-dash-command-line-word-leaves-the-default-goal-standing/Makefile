# GNU Make's `handle_non_switch_argument` opens by returning on a lone `-`,
# under a comment reading `Ignore plain '-' for compatibility.` — so the word is
# neither a goal nor an assignment, the default goal stands, and MAKECMDGOALS
# never hears about it.
all: ; @printf 'built=[all] goals=[%s]\n' '$(MAKECMDGOALS)' > out
second: ; @printf 'built=[second]\n' >> out
