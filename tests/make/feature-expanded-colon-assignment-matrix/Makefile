# GNU Make expands only through the source token that produces the first rule
# colon, then classifies a target-specific assignment from that expanded tail
# plus the still-literal suffix. Pin both assignment outcomes and both ways a
# normal rule can acquire its inline recipe.
COLON = :
SEMI = ;
WORDS = A B
EQUAL_TARGET = name=value
WHOLE_WITH_COMMAND = command-assigned: COMMAND_VALUE=kept; @printf '%s\n' wrong > wrong-out
WHOLE_NORMAL = whole-normal: one name=value; @printf '%s\n' whole-normal > whole-normal-out

.PHONY: all normal-literal normal-expanded whole-normal assigned spaced command-assigned one $(EQUAL_TARGET)

all: normal-literal normal-expanded whole-normal assigned spaced command-assigned ; @printf '%s\n' all > all-out

normal-literal$(COLON) one name=value ; @printf '%s\n' normal-literal > normal-literal-out
normal-expanded$(COLON) one name=value $(SEMI) @printf '%s\n' normal-expanded > normal-expanded-out
$(WHOLE_NORMAL)

one: ; @printf '%s\n' one > one-out
$(EQUAL_TARGET): ; @printf '%s\n' equals > equals-out

assigned$(COLON) ASSIGNED=good
assigned: ; @printf '<%s>\n' '$(ASSIGNED)' > assigned-out

spaced$(COLON) $(WORDS)=good
spaced: ; @printf '<%s>\n' '$($(WORDS))' > spaced-out

$(WHOLE_WITH_COMMAND)
command-assigned: ; @printf '<%s>\n' '$(COMMAND_VALUE)' > command-assigned-out
