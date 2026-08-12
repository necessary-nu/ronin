# Zstd writes its object rules as nothing but built-in variables, so a Make
# that leaves them undefined produces a recipe with no compiler in it: the
# first surviving word was `MMD` and the shell went looking for it.
CFLAGS = -O3
CPPFLAGS = -DXXH_NAMESPACE=ZSTD_
DEPFLAGS = -MMD -MP

hello.o: hello.c
	printf '%s\n' '$(COMPILE.c) $(DEPFLAGS) $(OUTPUT_OPTION) $<' > out
