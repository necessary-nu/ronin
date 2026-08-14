# Withdrawing the one-step link rule leaves the two-step road: `%.o: %.c`
# compiles and `%: %.o` links. Nothing wrote `hello.o` down, so it is an
# intermediate the build invented and sweeps up again.
%: %.c

all: hello
