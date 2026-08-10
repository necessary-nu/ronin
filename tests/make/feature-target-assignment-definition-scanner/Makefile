# Target-specific assignments use GNU Make's variable-definition scanner after
# the rule colon.  A backslash does not quote its first `=`, while a literal
# semicolon rejoins the value without turning a later `#` back into a comment.
# POSIX continuation spacing applies to that rejoined suffix too.
.POSIX:

escaped-equals: FOO\=BAR=x
escaped-equals: ; @printf '<%s><%s>\n' '$(FOO\)' '$(FOO\=BAR)' > escaped-equals-out

semicolon-comment: COMMENT_VALUE = before ; after # comment
semicolon-comment: ; @printf '<%s>\n' '$(COMMENT_VALUE)' > semicolon-comment-out

posix-spacing: POSIX_VALUE = before  ; after  \
  continued
posix-spacing: ; @printf '<%s>\n' '$(POSIX_VALUE)' > posix-spacing-out

operators: SIMPLE_VALUE:=simple
operators: POSIX_SIMPLE::=posix-simple
operators: IMMEDIATE_RECURSIVE:::=immediate-recursive
operators: ; @printf '<%s><%s><%s>\n' \
  '$(SIMPLE_VALUE)' '$(POSIX_SIMPLE)' '$(IMMEDIATE_RECURSIVE)' > operators-out

question-name: ??=question
question-name: ; @printf '<%s><%s>\n' '$(??)' '$(origin ??)' > question-name-out

escaped-semicolon: ESCAPED_SEMI:=left\;middle ; tail # kept
escaped-semicolon: ESCAPED_HASH:=left\\\#middle ; tail # kept
escaped-semicolon: ; @printf '<%s>\n<%s>\n' '$(ESCAPED_SEMI)' '$(ESCAPED_HASH)' > escaped-semicolon-out

.PHONY: all escaped-equals semicolon-comment posix-spacing operators question-name escaped-semicolon
all: escaped-equals semicolon-comment posix-spacing operators question-name escaped-semicolon
all: ; @printf '%s\n' all > all-out
