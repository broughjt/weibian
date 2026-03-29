Consider a node $a$. The following sets of nodes contribute to its backmatter:

- $Contexts(a) = \{ b \mid b \text{transcludes} a \}$
- $Backlinks(a) = \{ b \mid b \text{links to} a \}$
- $TranscludedLinks = \{ c \mid \exists b \in {a} \cup Desc(a), b \text{links to} c \}$

If we prevent inline body rendering in backmatter, I claim that we can know
whether to rerender a node's backmatter using the following condition. We should
rerender the backmatter for a node $a$ if its backmatters sets are different
from the previous call to `process` or if the title or metadata of any of the
nodes in these sets has changed.
