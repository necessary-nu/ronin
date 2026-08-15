# GNU Make brings every makefile it can reach up to date and refuses over the
# one it cannot afterwards, because `complain()` ends the run from inside the
# update rather than before it. The refusal also wins over the restart the
# successful remake would have caused.
include nope.mk

all: ; @printf 'all ran\n' > out

Makefile: Makefile.src ; @printf 'remade\n' >> log; touch Makefile
Makefile.src: ; @printf 'src\n' >> log; touch Makefile.src
