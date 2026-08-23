# The reading is not the dependent's alone: an entry of a chain whose own
# prerequisite is another chain reads the same value, and the entry beside it
# whose prerequisite is an ordinary file is the control that says the two
# spellings come from the same list.
all: a.o
	@echo "all new=$?" >> log.txt
a.o:: b.o
	@echo "one new=$?" >> log.txt
	@echo one >> $@
a.o:: p2
	@echo "two new=$?" >> log.txt
	@echo two >> $@
b.o:: p1
	@echo three >> $@
b.o:: p2
	@echo four >> $@
