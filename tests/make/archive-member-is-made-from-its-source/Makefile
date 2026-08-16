# The member need not exist yet. `(%): %` needs `foo.o`, which no rule names,
# so the implicit search chains through a second rule to make it — and `foo.o`
# is then an intermediate, made on the way to the archive and deleted after.
AR = ./fake-ar
all: libx.a(foo.o)

%.o: %.src
	@printf '%s\n' 'compile $< -> $@' >> log
	@cp $< $@
