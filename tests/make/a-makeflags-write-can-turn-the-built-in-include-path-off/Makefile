# A `-I -` written into MAKEFLAGS by a makefile is decoded and reaches the search
# path like any other, so it forgets the directory the command line named and
# turns the built-in ones off from here on. The fragment lives in that directory
# and nowhere else, so failing to find it is the assertion.
MAKEFLAGS += -I -

-include frag.mk

ifeq ($(FOUND),yes)
all: the-reset-went-nowhere
else
all: the-reset-emptied-the-path
endif

the-reset-emptied-the-path: ; @printf 'dirs=[%s]\n' '$(.INCLUDE_DIRS)' > out
the-reset-went-nowhere: ; @printf 'found=[%s] dirs=[%s]\n' '$(FOUND)' '$(.INCLUDE_DIRS)' > out
