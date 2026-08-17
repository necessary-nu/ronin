# `ar_glob` returning no matches is not an empty prerequisite list: GNU Make
# keeps the member name exactly as the makefile wrote it and lets the build
# refuse over it, so the wildcard reaches the diagnostic unexpanded.
all: libx.a(*.zzz)
	@echo "[$^]" > log
