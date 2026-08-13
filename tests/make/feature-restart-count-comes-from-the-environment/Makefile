# Make hands the restarted read its count through the environment, so
# MAKE_RESTARTS answers `environment` and is recursively flavoured. It is not
# passed on: a recipe's own environment does not have it.
all: ; @printf 'shell=%s value=%s origin=%s flavor=%s\n' "$${MAKE_RESTARTS-unset}" '$(MAKE_RESTARTS)' '$(origin MAKE_RESTARTS)' '$(flavor MAKE_RESTARTS)' > out

-include gen.mk

gen.mk: ; @printf '# made\n' > $@
