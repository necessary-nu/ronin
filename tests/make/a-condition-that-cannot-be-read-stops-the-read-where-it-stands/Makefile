# A condition that cannot be read is still refused -- but where the read reaches
# it, not before the read starts. GNU Make reads and evaluates one line at a
# time, so everything above the refused line has already happened when the read
# stops, and this one leaves a file behind to say so.
EARLY := $(shell printf 'ran\n' > early; echo done)

ifeq (a,a junk
endif

all: ; @echo built > out
