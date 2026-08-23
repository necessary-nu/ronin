# The second expansion is `..._for_file`, and the file is the target whose
# recipe is about to run — so a target-specific binding is what the `$` in the
# flags resolves against, and the same global `.SHELLFLAGS` gives two targets
# two different flags.
X := global
SHELL = ./shell
.SHELLFLAGS := -e $$X -c
all: X := pertarget
all:
	@echo hello
