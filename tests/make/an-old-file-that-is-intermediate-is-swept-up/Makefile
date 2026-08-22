# f_mtime ends by turning the intermediate bit off for a file that was already
# lying there, and f_mtime never runs for an -o name because its date is
# asserted rather than stated. remove_intermediates then deletes it, because -o
# also writes us_success over the status that would have spared it. Without the
# switch this build keeps out.o.
.INTERMEDIATE: out.o
all: app
app: out.o
	@echo made > $@
out.o: out.c
	@echo remade > $@
