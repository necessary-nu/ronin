# What a read does on the way through, GNU Make does once, because it reads
# once. Two recursive children mean the compiler has to read this text again
# before it can read the second child's, and the `$(file >>)` above must not
# append a second time for that: a staging pass is the compiler's, not the
# Makefile's, and nothing here can be allowed to see it.
$(file >> seen,a read happened)

all:
	@$(MAKE) --no-print-directory -f first.mk sub
	@$(MAKE) --no-print-directory -f second.mk sub
