# One member of a group is present and current; the other is absent. Asking for
# the present one still has to run the recipe, because the group is made whole
# or not at all.
gta gtb &: ; @printf '%s\n' rebuilt >> log; printf 'a\n' > gta; printf 'b\n' > gtb
