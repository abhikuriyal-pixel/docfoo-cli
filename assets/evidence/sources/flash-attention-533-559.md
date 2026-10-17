The main challenge in making attention memory-efficient is the softmax that couples the columns of K (and columns of V). Our approach is to compute the softmax normalization constant separately to decouple the columns. This technique \[60\] has been used in the literature \[51, 66\] to show that attention computation does not need quadratic extra memory (though the number of HBM accesses is still quadratic, resulting in slow run-time).

For simplicity, we omit here the max-shifting step during softmax. The full algorithm in Appendix B.3 contains all the steps.

Recall that given input sequences \\(\\mathbf\{Q\}, \\mathbf\{K\}, \\mathbf\{V\} \\in \\mathbb\{R\}^\{N \\times d\}\\), we want to compute the attention output \\(\\mathbf\{O\} \\in \\mathbb\{R\}^\{N \\times d\}\\):

$$
\mathbf{S} = \mathbf{Q}\mathbf{K}^{\top} \in \mathbb{R}^{N \times N}, \quad \mathbf{P} = \operatorname{softmax}(\mathbf{S}) \in \mathbb{R}^{N \times N}, \quad \mathbf{O} = \mathbf{P}\mathbf{V} \in \mathbb{R}^{N \times d}.
$$

We have that \\(S\_\{ij\} = q\_i^T k\_j\\) where \\(q\_i\\) and \\(k\_j\\) are the \\(i\\)-th and \\(j\\)-th columns of \\(\\mathbf\{Q\}\\) and \\(\\mathbf\{K\}\\) respectively. Define the normalization constants of softmax:

$$
\(L_i = \sum_j e^{q_i^T k_j}.\)\tag{1}
$$

Let \\(v\_j\\) be the \\(j\\)-th column of \\(\\mathbf\{V\}\\), then the \\(i\\)-th columns of the output is

$$
\(o_i = P_i \cdot V = \sum_j P_{ij} v_j = \sum_j \frac{e^{q_i^T k_j}}{L_i} v_j.\)\tag{2}
$$

We see that once \\(L\_i\\) is computed, we can compute \\(o\_i\\) without extra memory by repeatedly summing \\(\\frac\{e^\{q\_i^T k\_j\}\}\{L\_i\}v\_j\\). Therefore the forward pass can be computed with \\(O(n)\\) extra memory:

1\. Compute \\(L\_i\\) for all \\(i\\) according to Eq. (1), which takes \\(O(n)\\) extra memory.

2\. Compute \\(o\_i\\) for all \\(i\\) according to Eq. (2), which takes \\(O(d)\\) extra memory.
