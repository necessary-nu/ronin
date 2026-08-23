# One record, two files written in one run, and the dependent names the one the
# HEAD wrote. The head is remade and writes the name as written; the recipe-less
# entry after it is current and renames the rest of the record; the third entry
# then remakes the file where the search found it. A reading that followed the
# last rename would name a file that was also written, which is what makes this
# cell worth having beside the one where nothing is written at all.
VPATH = src
all: out.o
	@echo "first=$<" >> log.txt
out.o:: late
	@echo "one at=$@" >> log.txt
	@echo one >> $@
out.o:: keep
out.o:: late
	@echo "three at=$@" >> log.txt
	@echo three >> $@
