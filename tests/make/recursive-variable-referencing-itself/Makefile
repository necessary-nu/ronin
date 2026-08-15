# A variable that really does wait on itself is still refused. Nothing here
# shadows the name, so the variable the guard finds already expanding is the
# one being expanded — reached the long way round, through a second variable.
a = $(b)
b = $(a)
all: ; @echo [$(a)] > out
