SHELL := /bin/echo
NAMED := $(shell hello world)
SHELL := /bin/sh
.SHELLFLAGS := -ec
STOPPED := $(shell false; echo reached)
STATUS := $(.SHELLSTATUS)
.SHELLFLAGS := -c
CONTINUED := $(shell false; echo reached)
all: ; @printf '%s\n' 'NAMED=[$(NAMED)]' 'STOPPED=[$(STOPPED)]' 'STATUS=[$(STATUS)]' 'CONTINUED=[$(CONTINUED)]' > out
