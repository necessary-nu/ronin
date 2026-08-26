# A `\#` is an escape only where the `#` it hides would otherwise open a
# comment. GNU Make removes comments with
# `find_map_unquote (line, MAP_COMMENT|MAP_VARIABLE)` (read.c), and MAP_VARIABLE
# is what makes that scan step over a `$(` or `${` to its close without looking
# inside — so a `#` written in there was never going to start a comment, nothing
# unquotes the backslash in front of it, and the value keeps both characters.
#
#   plain   outside any reference: the backslash comes off and the `#` survives.
#   bare    inside a call, an unescaped `#` is not a comment either — both tools
#           already agreed here, and it is the control that says the two
#           questions are separate.
#   inside  inside a call, an escaped one keeps its backslash.
#   deeper  the same one reference further in.
#   matched both halves are literal `\#`, so they still match each other.
self = $1
plain := x\#y
bare := $(call self,#foo#)#comment
inside := $(call self,\#bar\#)#comment
deeper := $(patsubst %,[%],\#)
matched := $(subst \#,S,a\#b)
all: ; @printf '%s\n' 'plain=[$(plain)]' 'bare=[$(bare)]' 'inside=[$(inside)]' \
	'deeper=[$(deeper)]' 'matched=[$(matched)]' > out
