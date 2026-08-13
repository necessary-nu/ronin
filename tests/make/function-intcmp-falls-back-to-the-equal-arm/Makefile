lazy := $(intcmp 2,2,$(shell touch lt-was-taken),eq)
all: ; @printf '[%s]\n' '$(intcmp 1,2,lt)' '$(intcmp 2,2,lt)' '$(intcmp 3,2,lt)' \
 '$(intcmp 1,2,lt,eq)' '$(intcmp 2,2,lt,eq)' '$(intcmp 3,2,lt,eq)' \
 '$(intcmp 3,2,lt,eq,)' '$(lazy)' > out
