# With no goal named, the default comes from the first file that declares one,
# even though later files declare targets of their own.
ONE: ; @printf '%s\n' from-one > out
