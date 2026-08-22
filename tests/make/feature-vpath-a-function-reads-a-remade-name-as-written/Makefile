# The other side of the boundary the marker beside this records. A prerequisite
# the build remade here has one name again, so a function handed it reads what
# GNU Make's would: declining the reference inside a function argument is what
# keeps this case answering as it always did.
VPATH = build

all: out.o plain.h ; @printf 'whole [%s] filtered [%s] based [%s] new [%s]\n' '$^' '$(filter %.o,$^)' '$(basename $^)' '$?' > seen

out.o: out.c ; @printf 'made\n' > $@
