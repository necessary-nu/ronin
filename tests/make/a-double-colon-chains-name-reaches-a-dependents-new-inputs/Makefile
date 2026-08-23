# What a dependent of a `::` chain calls the prerequisite it had to wait for.
# The chain's target reaches its dependents through a completion proxy, so
# every entry runs before the dependent does; the proxy is a name the compiler
# invented and `$?` is a value GNU Make fills with the Makefile's own. `$^` and
# `$<` read the target already, and are the control beside `$?` and its halves.
all: out.o
	@echo "new=$? dir=$(?D) file=$(?F)" >> log.txt
	@echo "every=$^ first=$<" >> log.txt
out.o:: p1
	@echo one >> $@
out.o:: p2
	@echo two >> $@
