# `construct_include_path` (read.c) stats every `-I` directory as the switches
# are decoded: one that is not there never joins .INCLUDE_DIRS, and a trailing
# slash is discarded before the name is recorded.
#
# The list is not compared word for word: GNU's ends with its built-in default
# search path and this evaluator searches no such path, so what is compared is
# what `-I` put there — which is the whole of what this case is about, and is
# the same in both tools whatever else the list holds.
ifeq ($(filter inc,$(.INCLUDE_DIRS)),inc)
all: the-directory-joined
else
all: the-directory-did-not-join
endif

the-directory-joined: ; @printf 'named=[%s] absent=[%s] stripped=[%s] as-written=[%s]\n' '$(filter inc,$(.INCLUDE_DIRS))' '$(filter nosuchdir,$(.INCLUDE_DIRS))' '$(filter inc2,$(.INCLUDE_DIRS))' '$(filter inc2/,$(.INCLUDE_DIRS))' > out
the-directory-did-not-join: ; @printf 'branch=[absent]\n' > out
