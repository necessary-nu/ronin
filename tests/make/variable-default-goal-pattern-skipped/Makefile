# A pattern rule says how to make things rather than naming one, so it never
# becomes the default goal and the first named target still does.
%.o: %.c ; @printf '%s\n' 'pattern ran' > $@

real: ; @printf '%s\n' 'real ran' > out
