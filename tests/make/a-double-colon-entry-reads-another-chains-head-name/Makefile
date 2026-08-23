# An entry of one `::` record reading another record's name. The two readings
# are separate: this entry reads its OWN record's latch for `$@`, and the other
# record's head for the prerequisite it waited on. `dep.o`'s first entry is the
# dependent here, and `out.o`'s head is remade, so the prerequisite is the name
# as written however the entry after it renamed itself.
VPATH = src
all: dep.o
	@echo "all=$^" >> log.txt
dep.o:: out.o
	@echo "entry first=$< new=$?" >> log.txt
	@echo dep >> $@
dep.o:: p2
	@echo dep2 >> $@
out.o:: late
	@echo one >> $@
out.o:: keep
	@echo two >> $@
