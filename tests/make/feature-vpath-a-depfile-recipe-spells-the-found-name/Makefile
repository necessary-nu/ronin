# A recipe carrying an automatic depfile is the fourth kind the compiler has to
# read for itself, and it carries the same references the other three do. GNU
# Make has no such variable and reads the recipe when it runs it, so the two
# answer the same way for the same reason.
VPATH = build

all: out.o
	@printf 'whole [%s] first [%s]\n' '$^' '$<' > seen
all: .KATI_DEPFILE := all.d

out.o: out.c ; @printf 'made\n' > $@
