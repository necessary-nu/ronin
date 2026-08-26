# GNU Make's `func_foreach` (function.c) takes the FIRST TOKEN of what the name
# argument expanded to and nothing else: `next_token` skips leading whitespace
# and `end_of_token (vp)[0] = '\0'` cuts off everything from the next blank on.
#
# So the surrounding whitespace a comma-separated argument keeps is not part of
# the name, and neither is a second word. A `foreach` written across several
# lines is the same question — a backslash-newline leaves whitespace where it
# was folded, and binding a name that still carries it means `$a` finds nothing.
#
#   spaced      spaces either side of the name
#   folded      the same, written with continuations
#   two-words   a name argument holding two tokens: the first is the name, and
#               the whole of it names nothing
all: ; @printf '%s\n' 'spaced=[$(spaced)]' 'folded=[$(folded)]' \
	'first=[$(first)]' 'whole=[$(whole)]' > out

spaced := $(foreach  a , b c d ,[$a])

folded := $(foreach \
  a \
, b c d \
,[$a])

$(foreach a b, x y, $(eval first += [$a])$(eval whole += [$(a b)]))
