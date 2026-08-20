# A conditional's close is found by counting parens forward from the opening
# one, never by reading the line's last byte. So whatever follows the balanced
# close is trailing text: it is warned about and the read carries on, rather
# than the line being taken for a different form of the directive and refused.
all: plain nested parens unequal

ifeq (a,a) junk
plain: ; @echo plain > plain
endif

ifeq ($(subst a,b,aaa),bbb) more junk
nested: ; @echo nested > nested
endif

ifeq ((a),(a)) x
parens: ; @echo parens > parens
endif

ifneq (a,b) junk
unequal: ; @echo unequal > unequal
endif
