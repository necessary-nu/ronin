# A rule named for a single suffix is a link rule: GNU Make turns `.zig:` into
# the pattern rule `%: %.zig` once the read is over, on the same terms as the
# catalogue's own. Declaring the suffix is what makes the conversion happen.
.SUFFIXES: .zig

.zig: ; @printf 'made %s from %s\n' $@ $< > $@

all: foo
