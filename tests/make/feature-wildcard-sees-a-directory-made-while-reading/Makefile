# A directory that was not there when a wildcard looked into it is looked at
# again once a command has made it, as a name and as a pattern alike.
before := $(wildcard sub) $(wildcard sub/*)
$(shell mkdir -p sub && printf 'x\n' > sub/inner.txt)
after := $(wildcard sub) $(wildcard sub/*)

all: ; @printf '[%s] [%s]\n' '$(before)' '$(after)' > out
