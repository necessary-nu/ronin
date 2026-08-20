sub:
	@printf 'K := rewritten-by-a-child\n' > kept.mk
	@rm -f gone.mk
	@printf 'one\n' > first.out
