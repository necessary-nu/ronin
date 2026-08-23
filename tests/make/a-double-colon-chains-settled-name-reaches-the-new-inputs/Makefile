# `$?` is read off the same file objects `$^` is, so the rename an entry found
# current commits reaches it too. The first entry has nothing to do, which
# latches the found path for the chain, and the dependent then names the file
# down the search path rather than the one the Makefile wrote.
VPATH = src
all: out.o
	@echo "new=$? dir=$(?D) file=$(?F)" >> log.txt
	@echo "every=$^ first=$<" >> log.txt
out.o:: keep
	@true
out.o:: late
	@echo two >> $@
