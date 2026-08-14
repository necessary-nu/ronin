# A `$(file >)` in a recipe happens while that recipe is expanded, which is
# before its own first line runs and after every earlier recipe has finished.
# So the recipe that wrote it sees it, and so does the one expanded next.
all: second

first:
	$(file > written,from the first recipe)
	@printf 'first [%s]\n' '$(file < written)' > out

second: first
	@printf 'second [%s]\n' '$(file < written)' >> out
