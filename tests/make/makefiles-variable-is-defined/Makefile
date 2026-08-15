all:
	@printf '%s\n' 'origin=[$(origin MAKEFILES)]' 'flavor=[$(flavor MAKEFILES)]' \
	  'value=[$(MAKEFILES)]' 'list=[$(MAKEFILE_LIST)]' > out
	@printf 'child=[%s]\n' "$$(env | grep -c '^MAKEFILES=' || true)" >> out
