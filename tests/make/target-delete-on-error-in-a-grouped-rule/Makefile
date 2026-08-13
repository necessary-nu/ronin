# One recipe makes both members, so a failure leaves both half-made -- and
# .PRECIOUS answers for each name on its own rather than for the recipe.
.DELETE_ON_ERROR:
.PRECIOUS: kept
all: gone kept
gone kept &: ; echo G > gone ; echo K > kept ; exit 1
