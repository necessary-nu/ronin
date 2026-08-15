# The recursion guard is about the variable, not the name it answers to, so a
# binding that shadows the name being expanded is a different variable and
# reading it is not recursion.
y = $(foreach y,z,$ydef)
l = $(let l,,$ldef)
n = $(foreach n,a b,$(foreach n,1 2,$(n)))
s = $(foreach s,1 2,"<$(s)>")
o = outer
i = $(foreach o,inner,$(o))
all: ; @printf '[%s]\n' '$y' '$l' '$n' '$s' '$i' '$o' > out
