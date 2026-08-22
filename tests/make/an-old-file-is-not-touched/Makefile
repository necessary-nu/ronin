# -t brings the goals up to date by touching them. A target -o named is already
# up to date, so there is nothing to touch and the file stays absent.
all: app
app: out.o
	@echo made > $@
out.o: out.c
	@echo remade > $@
