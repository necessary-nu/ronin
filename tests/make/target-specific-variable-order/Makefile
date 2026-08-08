BLAH := foo
COMMAND = echo $(BLAH)
all: ; @$(COMMAND) > out
all: BLAH := bar
all: COMMAND += snafu $(BLAH)
