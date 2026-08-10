# Source scanning unquotes the interior semicolon before partial expansion, but
# the whole ordinary word must still be expanded to discover its rule colon.
COL = :
foo\;bar$(COL) ; @printf '<%s>\n' '$@' > result
