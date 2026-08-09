list := word.pl general.pl1 FORCE.pl /tmp.c/bar foo.baz/bar.c plain
all: ; @printf '[%s]\n' '$(suffix $(list))' '$(sort $(suffix $(list)))' > out
