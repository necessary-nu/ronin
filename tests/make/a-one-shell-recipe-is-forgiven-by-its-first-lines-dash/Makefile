# `.ONESHELL` leaves the recipe unchopped, so the prefix scan that fills its
# flags runs over the first line alone and what it finds stands for all of it.
# The `-` here forgives the `false` two lines below it.
.ONESHELL:
all: out
out:
	-@touch m1
	false
