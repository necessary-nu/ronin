foo := a.o b.o c.o
doubled := fooBARfooBARfoo
all: ; @printf '[%s]\n' '$(foo:.o=.c)' '$(foo:%.o=%.x)' '$(doubled:fooBARfoo=REPL)' > out
