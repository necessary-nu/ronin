A = base
A$:= a
B = base
B$+= b
C = base
C$?= c
D = base
D$!= echo shelled
E = base
E$::= e
F = base
F$:::= f
G = base
G$$:= g
X =
H = base
H$(X)+= h
I = base
I$(X):= i
J = base
J${X}?= j
K = base
K$X:= k
L$:M = l
all:
	@printf '%s\n' 'A=$(A)/$(flavor A)' 'B=$(B)/$(flavor B)' \
	  'C=$(C)/$(flavor C)' 'D=$(D)/$(flavor D)' 'E=$(E)/$(flavor E)' \
	  'F=$(F)/$(flavor F)' 'G=$(G)/$(flavor G)' 'Gdollar=$(G$)/$(flavor G$)' \
	  'H=$(H)/$(flavor H)' 'I=$(I)/$(flavor I)' 'J=$(J)/$(flavor J)' \
	  'K=$(K)/$(flavor K)' 'LM=$(LM)/$(flavor LM)' > out
