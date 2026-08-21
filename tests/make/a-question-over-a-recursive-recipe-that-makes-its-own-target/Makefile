# `-q` runs nothing and says whether there is work to do. A recursive recipe
# whose earlier lines would write its own target is work to do: the answer is
# one, and neither the staged lines nor the child may run to produce it.
pre.out:
	@echo before > pre.out
	@$(MAKE) -f sub.mk sub
	@echo after >> pre.out
