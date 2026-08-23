# The switches are stamped after the read and before the update, so `enter_file`
# reads the file database as the makefiles left it and nothing has been renamed
# yet. `src/out` is therefore not the double-colon target `out` is: the stamp
# makes an ordinary entry of its own, and the GPATH rename then merges the two
# and keeps the newer date — file.c's `%%% Kludge so -W wins on a file that gets
# vpathized`. So the run is not refused, and app is remade.
VPATH = src
GPATH = src
all: app
app: out
	@echo made > $@
out:: a
	@echo remade > $@
out:: b
	@echo again >> $@
