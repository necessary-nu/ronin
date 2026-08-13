# `+=` writes a separator between two values, so a variable holding no value
# takes the appended text on its own. What "no value" means is asked of the
# text as written, never of what that text expands to: a recursive variable
# spelled as a reference to nothing still has text, and the separator lands
# ahead of the appended word where it expands to a leading space.
empty =
blank = $(empty)
space := $(empty) $(empty)
spaced := $(space)
grown =

empty += x
blank += x
spaced += x
grown += one
grown += two

all: ; @printf '[%s]\n' '$(empty)' '$(value empty)' '$(blank)' '$(value blank)' '$(spaced)' '$(grown)' '$(value grown)' > out
