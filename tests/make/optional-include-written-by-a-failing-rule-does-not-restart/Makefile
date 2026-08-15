# What raises `any_remade` is a Makefile whose own update succeeded, not one
# whose file happens to have moved. A recipe that writes the file it names and
# then exits non-zero leaves the file there and starts nothing over.
all: ; @printf 'restarts=%s one=%s\n' '$(MAKE_RESTARTS)' '$(ONE)' > out

-include one.mk

one.mk: ; @printf 'ONE := yes\n' > $@; exit 1
