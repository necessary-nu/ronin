# The cell that settles what the rule IS. The head has no recipe and a phony
# prerequisite this run reaches, so it is out of date and never renames; the
# entry after it is current and renames itself. Nothing is written anywhere by
# either tool — and the dependent still reads the name as written, which is the
# head's verdict and cannot be "whichever name the run wrote".
VPATH = src
all: out.o
	@echo "first=$<" >> log.txt
out.o:: ph
out.o:: keep
	@echo "two at=$@" >> log.txt
	@echo two >> $@
.PHONY: ph
ph: ;
