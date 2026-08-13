# Everything the Makefile update reached is work already done: `force` ran once
# for gen.mk and does not run again. What it still does is what a phony target
# always does — anything else that named it is out of date and is remade.
.PHONY: force

force: ; @printf 'force ran\n' >> log

include gen.mk

gen.mk: force ; @printf 'gen ran\n' >> log

other: force ; @printf 'other ran\n' >> log
