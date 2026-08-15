# The implicit rule search can invent the name that closes the loop: hello.f is
# on disk, so `%.o: %.f %.tsk` is the rule chosen for hello.o, and its hello.tsk
# prerequisite is the hello.tsk the goal is already being made for.
all: hello.tsk
%.tsk: %.o ; @printf 'tsk\n' >> order
%.o: %.c ; @printf 'c\n' >> order
%.o: %.f %.tsk ; @printf 'f\n' >> order
