# A default is the weakest thing a name can be bound to. The Makefile's own
# assignment replaces it outright, and a compound macro it did not assign goes
# on reading whatever the constituent now holds.
COMPILE.c = my-compiler --compile
CC = clang

all: ; printf '[%s]\n' '$(COMPILE.c)' '$(COMPILE.S)' '$(origin COMPILE.c)' '$(origin CC)' '$(origin COMPILE.S)' > out
