# Where the condition ends decides what is compared, and it ends at the paren
# that balances the opening one. Each of these compares equal only if the close
# was found by counting forward; reading the line's last byte as the close
# gives the second string extra text and the comparison the other answer.
all: comma late extra

# The comma inside the nested parens is not the one that ends the first string.
ifeq (a(b,c),a(b,c))
comma: ; @echo counted > comma
else
comma: ; @echo uncounted > comma
endif

# The condition closes at its own paren; `,c)` after it is trailing text.
ifeq (a,a),c)
late: ; @echo counted > late
else
late: ; @echo uncounted > late
endif

ifeq (a,a))
extra: ; @echo counted > extra
else
extra: ; @echo uncounted > extra
endif
