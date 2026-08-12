# The assembly half of the same shape. With `COMPILE.S` undefined the recipe
# collapsed to the source path alone and the shell tried to execute it.
ASFLAGS = -Wa,--noexecstack

huf_decompress_amd64.o: huf_decompress_amd64.S
	printf '%s\n' '$(COMPILE.S) $(OUTPUT_OPTION) $<' > out
