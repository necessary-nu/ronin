# A glob in `-include` that matches nothing leaves the word as written, and
# make looks for a rule making a file of that name. There is none, so the
# optional include is forgotten and the build carries on.
-include *.d

all: ; @printf '%s\n' ok > out
