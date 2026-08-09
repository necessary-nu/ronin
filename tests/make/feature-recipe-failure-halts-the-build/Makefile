# A recipe that fails stops the build: what it had already written stays, and
# the prerequisite after it is not made.
all: first second

first: ; @printf '%s\n' first > first-out; exit 1
second: ; @printf '%s\n' second > second-out
