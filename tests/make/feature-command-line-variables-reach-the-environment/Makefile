# A command-line value is exported without being asked, and travels in
# MAKEFLAGS as well, so a child reads it either way.
all: ; @printf 'CLV=[%s]\n' "$$CLV" > out
