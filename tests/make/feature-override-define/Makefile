# override applies to a multi-line define as well, so the command line cannot
# replace the recipe body the makefile meant to use.
override define body
@printf '%s\n' one > out
@printf '%s\n' two >> out
endef

all: ; $(body)
