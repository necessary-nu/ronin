all: ; @echo all-$(V) >> ran
include gen.mk
gen.mk: stamp
	@echo remade >> ran
	@echo "V=1" > gen.mk
