# GNU Make runs a recipe's lines in the order they were written, and a line
# holding $(MAKE) is one of them: the lines before it have run when the child
# starts, and the lines after it have not. Two invocations make the ordering
# say something a single one cannot — a line between them is neither first nor
# last, and a recipe that runs its own lines in one block gets it wrong however
# it places the block.
all:
	@echo L1 >> log
	@$(MAKE) --no-print-directory -C a
	@echo L2 >> log
	@$(MAKE) --no-print-directory -C b
	@echo L3 >> log
