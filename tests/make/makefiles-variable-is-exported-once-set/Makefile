MAKEFILES =
EMPTY_SET := $(shell env | grep -c '^MAKEFILES=' || true)
all:
	@printf 'emptyset=[$(EMPTY_SET)]\n' > out
	@printf 'child=[%s]\n' "$$(env | grep -c '^MAKEFILES=' || true)" >> out
