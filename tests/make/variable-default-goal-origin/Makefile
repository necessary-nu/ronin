# .DEFAULT_GOAL is bound before any target is recorded, so it is readable then
# rather than undefined, and the first eligible target is what it names after.
BEFORE := origin=$(origin .DEFAULT_GOAL) flavor=$(flavor .DEFAULT_GOAL) value=[$(.DEFAULT_GOAL)]
foo: ; @printf '%s\n' '$(BEFORE)' '$(AFTER)' > out
bar: ; @printf '%s\n' 'bar ran' > bar.out
AFTER := origin=$(origin .DEFAULT_GOAL) flavor=$(flavor .DEFAULT_GOAL) value=[$(.DEFAULT_GOAL)]
