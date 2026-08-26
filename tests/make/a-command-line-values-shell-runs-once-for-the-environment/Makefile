# A command-line variable is exported without being asked, so its value is
# settled to build the one environment this run needs — once, not twice. The
# makefile never reads $(V), so the only reason `gen` runs at all is that
# settle, and `counted` counts it.
GEN = $(shell ./gen)
all: ; @printf 'done\n' > out
