notdir = SHADOW
suffix = SHADOW
all: ; @printf '[%s]\n' '$(call notdir,a/b c/d)' '$(call suffix,a.c b.o)' '$(call notdir)' \
 '$(call ,anything)' '$(call call,notdir,x/y)' '$(call subst,a,b,banana,ignored)' > out
