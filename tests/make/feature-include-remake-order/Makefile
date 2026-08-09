# Makefiles that have to be built are built in the order they were included,
# across separate include lines as well as within one.
all: ; @printf '%s\n' done > out

-include i1.mk i2.mk
-include i3.mk

i1.mk i2.mk i3.mk: ; @printf '%s\n' '$@' >> order; printf '# made\n' > $@
