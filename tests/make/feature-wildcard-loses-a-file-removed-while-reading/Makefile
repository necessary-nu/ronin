# A wildcard reads the directory afresh after a command, so a file a function
# call removed between two of them is gone from the second, by name as well as
# by pattern.
$(shell printf 'x\n' > gone.probe)
before := $(wildcard *.probe) $(wildcard gone.probe)
$(shell rm -f gone.probe)
after := $(wildcard *.probe) $(wildcard gone.probe)

all: ; @printf '[%s] [%s]\n' '$(before)' '$(after)' > out
