# What the catalogue is, rather than what it expands to: every entry carries
# the `default` origin and is deferred, so a Makefile can tell a built-in from
# something it set itself and a compound macro tracks a later assignment.
all: ; printf '[%s]\n' '$(origin CC)' '$(flavor CC)' '$(origin COMPILE.c)' '$(flavor COMPILE.c)' '$(origin OUTPUT_OPTION)' '$(origin CHECKOUT,v)' '$(AR) $(ARFLAGS)' '$(RM)' '$(CPP)' '$(LINK.o)' '$(LINK.cpp)' '$(PREPROCESS.S)' '$(.LIBPATTERNS)' > out
