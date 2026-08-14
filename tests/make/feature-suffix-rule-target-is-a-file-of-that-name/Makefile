# `.c.o:` defines the built-in-shaped suffix rule and names a file called
# `.c.o`. Asking for that file by name runs the rule's recipe with no stem
# anywhere in it.
.c.o: ; @printf '%s\n' 'asked for $@' > out
