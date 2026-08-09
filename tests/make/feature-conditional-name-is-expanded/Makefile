# The argument of ifdef is expanded before it is looked up, so the name being
# asked about can itself be computed.
foo = set
FOO = foo
F = f

first = no
second = no

ifdef $(FOO)
first = yes
endif

ifdef $(F)oo
second = yes
endif

all: ; @printf '%s %s\n' '$(first)' '$(second)' > out
