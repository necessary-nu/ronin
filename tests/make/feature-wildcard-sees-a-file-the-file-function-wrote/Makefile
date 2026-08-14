# Writing with $(file >) changes a directory without starting a shell, and a
# wildcard after it still finds what was written.
before := $(wildcard *.written)
$(file > made.written,contents)
after := $(wildcard *.written)

all: ; @printf '[%s] [%s]\n' '$(before)' '$(after)' > out
