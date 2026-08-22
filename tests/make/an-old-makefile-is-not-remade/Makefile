# The -o files are stamped before the makefile update rather than after it, and
# with no restarts guard beside them, so a makefile the switch names is not
# remade and the read does not go around.
include gen.mk
all:
	@echo hi-$(V) > ran
gen.mk: gen.src
	@echo "V=1" > $@
	@echo remade >> genlog
