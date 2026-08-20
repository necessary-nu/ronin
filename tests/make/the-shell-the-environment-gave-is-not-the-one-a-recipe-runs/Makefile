# GNU Make offers the built-in shell at default rank and then refuses to let the
# environment's answer survive: `define_automatic_variables` (variable.c)
# rewrites the rank to `file` and the text back to /bin/sh, leaving the
# environment import's recursive flavour alone. So a SHELL the environment
# carried stands `file` and recursive holding /bin/sh — a rank a makefile's own
# assignment is a peer of, and one `-e` cannot lift, because the rank `-e`
# promotes an environment binding to is the rank this same block overwrites.
#
# The program the environment named therefore never runs a recipe. This case
# says so twice: by the branch it takes, and by the file the impostor would
# leave behind if it were the one invoked.
ifeq ($(origin SHELL),file)
all: the-shell-is-the-built-in-one
else
all: the-environment-kept-it
endif

the-shell-is-the-built-in-one: ; @printf 'origin=[%s] flavor=[%s] value=[%s]\n' '$(origin SHELL)' '$(flavor SHELL)' '$(value SHELL)' > out
the-environment-kept-it: ; @printf 'branch=[%s]\n' '$(origin SHELL)' > out
