all: out.o
	@echo "new=$? dir=$(?D) file=$(?F)" >> ../log.txt
out.o:: p1
	@echo one >> $@
out.o:: p2
	@echo two >> $@
