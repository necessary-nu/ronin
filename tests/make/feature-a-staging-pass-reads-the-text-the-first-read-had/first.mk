sub:
	@printf 'G := from-a-child\nS := $$(shell printf child >> shelllog; echo asked-again)\n$$(file >> seen,the newly written text spoke)\n' > gen.mk
	@mkdir -p parts
	@printf 'P := from-a-glob\n' > parts/one.mk
	@printf 'one\n' > first.out
