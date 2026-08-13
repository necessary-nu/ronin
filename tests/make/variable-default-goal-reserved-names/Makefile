# A leading dot with no directory separator after it is a name Make reserved,
# and one is passed over rather than ending the search: the targets beside it
# are still considered, and a dot inside a path is an ordinary file.
.hidden: ; @printf '%s\n' 'hidden ran' > hidden.out
..dots: ; @printf '%s\n' 'dots ran' > dots.out
sub/.deps: ; @printf '%s\n' 'nested ran' > out
plain: ; @printf '%s\n' 'plain ran' > plain.out
