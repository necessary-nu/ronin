# `.POSIX:` changes how a continuation is folded in a variable's value and
# nothing at all about a recipe's, which GNU Make never folds.
.POSIX:
all:
	@printf '%s' 'a\
	b' | od -An -c > out
