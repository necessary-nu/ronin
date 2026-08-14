# A target declared again in a later file is the ordinary re-declaration: the
# later recipe replaces the earlier one.
all: ; @printf '%s\n' from-first > out
