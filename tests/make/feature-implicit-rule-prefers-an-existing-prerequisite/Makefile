# Two pattern rules could make the target. The one whose prerequisite is
# already on disk is chosen, whichever was read first.
all: hello.tsk

%.tsk: %.c ; @printf '%s\n' from-c > $@
%.tsk: %.f ; @printf '%s\n' from-f > $@
