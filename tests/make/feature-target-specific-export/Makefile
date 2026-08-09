# A target-specific assignment can carry the export keyword, and then the name
# reaches the environment of that target's recipe alone.
all: base ; @printf 'BAZ=%s\n' "$$BAZ" > all-out
all: export BAZ = baz

base: ; @printf 'BAZ=%s\n' "$$BAZ" > base-out
