The previous section assumed we used Transformers that used hardmax operator σ\_H and activations functions belonging to the set Φ. This is without loss of generality as following lemma shows.

Lemma 5 (Lemma 9 \[104\]). For each \\(g \\in \\mathcal\{T\}^\{2,1,1\}\\) and \\(1 \\le p \\le \\infty\\), \\(\\exists g \\in \\mathcal\{T\}^\{2,1,4\}\\) such that \\(d\_p(g,\\bar\{g\}) \\le \\epsilon/3\\)

Combining the above lemma with the Lemma 3, we get our main result:

Theorem 2. Let \\(1 \\leq p \\leq \\infty\\) and \\(\\epsilon \> 0\\), there exists a transformer network \\(g \\in \\mathcal\{T\}\_D^\{2,1,4\}\\) which achieves a ratio of \\(d\_p(f,g) \\leq \\epsilon\\) where \\(D\\) is the sparse graph.

Since the sparsity graph associated with BigBird contains a star network, we know that it can express any continuous function from a compact domain.

Contemporary work on Universal Approximability of Sparse Transformers We would like to note that, contemporary work done by Yun et al. \[105\], also parallelly explored the ability of sparse transformers with linear connections to capture sequence-to-sequence functions on the compact domain.

---
