# A file that is not there reads as nothing and does not stop the build:
# `func_file` returns what it has when the open fails with ENOENT, and only
# reports anything else. The kernel relies on it — `read-file` is called for
# `include/config/kernel.release` whether or not a configuration was made.
all:
	@printf 'absent=[%s] present=[%s]\n' '$(file < absent)' '$(file < present)' > out
