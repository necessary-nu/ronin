# A bare export is filtered by whether the name is one a shell could read back:
# a letter or underscore, then letters, digits and underscores. A name that
# fails it is skipped rather than refused, and the ones that pass still go.
A.B = dotted
1AB = leading-digit
A_B = fine
_OK = also-fine
export
all: ; @env | grep -E '^(A_B|_OK|A\.B|1AB)=' | sort > out || true
