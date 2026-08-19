# `.ONESHELL` puts a recipe's lines in one shell, so a line before an
# invocation can have changed the directory that invocation runs in, or set a
# variable it reads, and no reading of the recipe can say it did not. A recipe
# like this names no child a compiler can lift out of it. GNU Make runs it and
# lets the Make it names start, which is also the answer for a recipe whose
# only recursion cannot be lifted — it is not a recipe to refuse.
.ONESHELL:
all:
	@cd sub
	$(MAKE) --no-print-directory
	echo parent >> ../log
