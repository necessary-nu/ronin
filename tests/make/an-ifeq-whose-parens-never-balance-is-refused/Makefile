# The `(a)` inside the second string balances itself, so the condition's own
# close never arrives and the directive is refused. Reading the line's last
# byte as the close would take this for a well-formed condition comparing `a`
# against `(a`.
ifeq (a,(a)
inner: ; @echo inner > inner
endif

all: ; @echo built > out
