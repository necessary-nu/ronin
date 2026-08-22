# Both dependency chains reach the one object that is left, the kept one's
# first, and both prerequisites are made — even though only one of the two
# recipes survives.
vpath %.te d/

all: fail.te
	@printf 'all from [%s]\n' '$<' > seen

p1: ; @touch p1
p2: ; @touch p2

fail.te: p1 ; @printf 'here deps [%s]\n' '$^' > seen2
d/fail.te: p2 ; @printf 'there deps [%s]\n' '$^' > seen3
