# `.RECIPEPREFIX` decides which character the continuation loses, and a tab is
# then ordinary text: the one `>` goes and the tab after it arrives inside the
# quotes.
.RECIPEPREFIX = >
all:
>@printf '%s' 'a\
>	b' | od -An -c > out
