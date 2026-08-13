# Expanding the line runs the $(shell), so its side effect lands before any of
# the commands the line became.
all: ; @echo recipe >> log $(shell echo shellfunc >> log)
	@cp log out
