# Under `-k`, `kbd.o` has no way to be made: `kbd.c` is not in `work/` and no
# rule makes it. GNU Make walks the goal's prerequisites anyway, builds the
# three it can, and abandons `edit` afterwards. Ronin refuses at graph load,
# because the goal provably cannot complete, and builds nothing.
VPATH = work
edit:  main.o kbd.o commands.o display.o
	@echo linked > edit
main.o : main.c defs.h
	@echo compiled > main.o
kbd.o : kbd.c defs.h command.h
	@echo compiled > kbd.o
commands.o : command.c defs.h command.h
	@echo compiled > commands.o
display.o : display.c defs.h buffer.h
	@echo compiled > display.o
