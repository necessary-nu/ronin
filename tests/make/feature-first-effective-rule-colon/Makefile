# A later colon cannot change the provenance of the first effective one. Cover
# an expanded first colon followed by either spelling of the second, plus the
# all-literal control case. Each second colon belongs to the assigned value.
COLON = :
EXPANDED_LITERAL = expanded-literal:E_L=good
EXPANDED_EXPANDED = expanded-expanded:E_E=good

$(EXPANDED_LITERAL):
$(EXPANDED_EXPANDED)$(COLON)
literal-literal:L_L=good:

.PHONY: all expanded-literal expanded-expanded literal-literal
all: expanded-literal expanded-expanded literal-literal ; @printf '%s\n' all > all-out
expanded-literal: ; @printf '<%s>\n' '$(E_L)' > expanded-literal-out
expanded-expanded: ; @printf '<%s>\n' '$(E_E)' > expanded-expanded-out
literal-literal: ; @printf '<%s>\n' '$(L_L)' > literal-literal-out
