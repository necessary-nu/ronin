# Two chains on one prerequisite list, which settles that the reading is per
# word rather than per value: each target has a proxy of its own, and `$(?D)`
# repeats the directory once for every name GNU Make puts in `$?`.
all: a.o b.o
	@echo "new=$? dir=$(?D) file=$(?F)" >> log.txt
a.o:: p1
	@echo one >> $@
a.o:: p2
	@echo two >> $@
b.o:: p1
	@echo three >> $@
b.o:: p2
	@echo four >> $@
