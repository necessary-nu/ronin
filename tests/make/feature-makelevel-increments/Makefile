# MAKELEVEL is zero at the top and one deeper at each sub-make.
all: ; @printf 'level=%s\n' '$(MAKELEVEL)' > top; $(MAKE) --no-print-directory -f sub.mk
