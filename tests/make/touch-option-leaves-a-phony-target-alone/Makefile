# `-t` brings a target up to date by dating it instead of making it, and a
# `.PHONY` target has no date to give: GNU Make declines it in `update_file_1`,
# so neither is the recipe run nor is a file of that name left behind. The
# ordinary target beside it is dated, and its recipe does not run either.
.PHONY: ph
all: out.txt ph
out.txt: in.txt ; @echo REAL > out.txt
ph: ; @echo PHONY > phony-ran
