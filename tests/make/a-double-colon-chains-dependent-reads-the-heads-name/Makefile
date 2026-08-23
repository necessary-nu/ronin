# Which entry's rename a DEPENDENT of a `::` chain reads. The rename reaches
# forward through the chain and no further, and what a dependent holds is the
# entry the record was filed under — the first. So a first entry that is
# remade keeps the name as written for every dependent, however many entries
# after it were found current and renamed themselves.
VPATH = src
all: out.o
	@echo "all first=$<" >> log.txt
out.o:: late
	@echo "one at=$@" >> log.txt
	@echo one >> $@
out.o:: keep
	@echo "two at=$@" >> log.txt
	@echo two >> $@
