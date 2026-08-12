# `-R` withholds the catalogue outright: the names are not bound at all, so
# `$(origin)` answers `undefined` rather than naming an empty default.
all: ; printf '[%s]\n' '$(COMPILE.c)' '$(OUTPUT_OPTION)' '$(origin CC)' '$(origin COMPILE.c)' '$(origin RM)' > out
