# `hello.z' is reached first and makes `hello.w' beside it, but `use' asks for
# `hello.w' by name, so its absence is a reason to run the recipe again even
# though `hello.z' is current.
all: hello.z use
hello.x: ; echo body > $@
%.z %.w: %.x ; echo ran >> log ; cat $< > $*.z ; cat $< > $*.w
use: hello.w ; cat $< > $@
