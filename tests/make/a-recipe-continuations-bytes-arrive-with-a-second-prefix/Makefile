# The only thing the recipe reader removes after the newline is ONE recipe
# prefix character. A second tab is text, and reaches the shell inside the
# quotes with the continuation's own two bytes.
all:
	@printf '%s' 'a\
		b' | od -An -c > out
