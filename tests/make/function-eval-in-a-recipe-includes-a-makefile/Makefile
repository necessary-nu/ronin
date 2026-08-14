# Reading a makefile from a recipe brings in its variables. Any rule it carried
# would be refused, but this one carries none.
all:
	$(eval include included.mk)
	@printf '%s\n' 'W=[$(W)]' > out
