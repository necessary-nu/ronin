# A `;;` ends a `case` item and is a token of the script, not punctuation
# between recipe lines. POSIX lets the LAST item drop its terminator, so a
# one-arm `case` survives losing it and a two-arm one does not: `y)` then
# follows an unterminated item and the shell refuses the whole script.
.ONESHELL:
all:
	@case x in
	x) touch m1 ;;
	y) touch m2 ;;
	esac
	touch m3
