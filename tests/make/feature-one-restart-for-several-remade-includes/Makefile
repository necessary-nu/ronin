# Every Makefile that has to be made is made before the read starts again, so
# two generated includes cost one restart between them rather than one each.
all: ; @printf 'restarts=%s first=%s second=%s\n' '$(MAKE_RESTARTS)' '$(A)' '$(B)' > out

-include a.mk
-include b.mk

a.mk: ; @printf 'A := a\n' > $@
b.mk: ; @printf 'B := b\n' > $@
