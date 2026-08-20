# `decode_env_switches` prepends the missing dash to `argv[1]` and to no other
# word, so a word with neither a dash nor a name is dropped where it stands.
# Counting the words kept so far instead would hand the dash to whatever an
# assignment happened to stand in front of, and `-ran` is three switches — one
# of them `-n`, which builds nothing at all.
MAKEFLAGS += FOO=bar ran
all: ; @printf 'FOO=[%s]\n' '$(FOO)' > out
