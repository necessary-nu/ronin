# A prerequisite that closes a loop is not a refusal. GNU Make drops the one
# edge it was standing on when it noticed, says so, and builds the rest: the
# target it dropped the prerequisite from runs first, because the walk had
# already reached it.
all: a
a: b ; @printf 'a\n' >> order
b: a ; @printf 'b\n' >> order
