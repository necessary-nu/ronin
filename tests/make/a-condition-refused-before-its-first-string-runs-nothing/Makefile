# The control for the two fixtures beside it. `conditional_line' has two ways of
# refusing above the expansion -- the first string's own terminator never
# arriving, and an opener that is neither a paren nor a quote -- and neither has
# run anything by the time it gives up. This is what says the expansion there
# belongs to where the refusal happens rather than to the refusal itself.
#
# Refused at the first of the two; the second is written below it so a reader
# can see both shapes, and moving it up swaps which one the read reaches.
ifeq ($(shell touch the-first-string-ran; echo x) junk
endif

ifeq junk$(shell touch the-opener-was-read; echo x)
endif

all: ; @echo built > out
