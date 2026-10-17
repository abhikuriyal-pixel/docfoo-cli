Given the inputs **Q**, **K**, **V** ∈ ℝᴺˣᵈ in HBM, we aim to compute the attention output **O** ∈ ℝᴺˣᵈ and write it to HBM. Our goal is to reduce the amount of HBM accesses (to sub-quadratic in N).

We apply two established techniques (tiling, recomputation) to overcome the technical challenge of computing exact attention in sub-quadratic HBM accesses. We describe this in Algorithm 1. The main idea is that we split the inputs Q, K, V into blocks, load them from slow HBM to fast SRAM, then compute the attention output with respect to those blocks. By scaling the output of each block by the right normalization factor before adding them up, we get the correct result at the end.

Tiling. We compute attention by blocks. Softmax couples columns of **K**, so we decompose the large softmax with scaling \[51, 60, 66\]. For numerical stability, the softmax of vector \\(x \\in \\mathbb\{R\}^B\\) is computed as:

$$
\(m(x) := \max_i x_i,\quad f(x) := [e^{x_1-m(x)} \quad \ldots \quad e^{x_B-m(x)}],\quad \ell(x) := \sum_j f(x)_j,\quad \operatorname{softmax}(x) := \frac{f(x)}{\ell(x)}\)
$$

---

```
For vectors \(x^{(1)}, x^{(2)} \in \mathbb{R}^B\), we can decompose the softmax of the concatenated \(x = [x^{(1)} \ x^{(2)}] \in \mathbb{R}^{2B}\) as:
\[
m(x) = m([x^{(1)} \ x^{(2)}]) = \max(m(x^{(1)}), m(x^{(2)})), \quad
f(x) = \left[e^{m(x^{(1)})-m(x)} f(x^{(1)}) \quad e^{m(x^{(2)})-m(x)} f(x^{(2)})\right],
\]
\[
\ell(x) = \ell([x^{(1)} \ x^{(2)}]) = e^{m(x^{(1)})-m(x)} \ell(x^{(1)}) + e^{m(x^{(2)})-m(x)} \ell(x^{(2)}), \quad
\operatorname{softmax}(x) = \frac{f(x)}{\ell(x)}.
\]
```

Therefore if we keep track of some extra statistics \\((m(x), \\ell(x))\\), we can compute softmax one block at a time.\\(^2\\) We thus split the inputs \\(\\mathbf\{Q\}, \\mathbf\{K\}, \\mathbf\{V\}\\) into blocks (Algorithm 1 line 3), compute the softmax values along with extra statistics (Algorithm 1 line 10), and combine the results (Algorithm 1 line 12).

Recomputation. One of our goals is to not store \\(O(N^2)\\) intermediate values for the backward pass. The backward pass typically requires the matrices S, P ∈ \\(\\mathbb\{R\}^\{N \\times N\}\\) to compute the gradients with respect to Q, K, V. However, by storing the output O and the softmax normalization statistics \\((m, \\ell)\\), we can recompute the attention matrix S and P easily in the backward pass from blocks of Q, K, V in SRAM. This can be seen as a form of selective gradient checkpointing \[10, 34\]. While gradient checkpointing has been suggested to reduce the maximum amount of memory required \[66\], all implementations (that we know of) have to trade speed for memory. In contrast, even with more FLOPs, our recomputation speeds up the backward pass due to reduced HBM accesses (Fig. 2). The full backward pass description is in Appendix B.

Implementation details: Kernel fusion. Tiling enables us to implement our algorithm in one CUDA kernel, loading input from HBM, performing all the computation steps (matrix multiply, softmax, optionally masking and dropout, matrix multiply), then write the result back to HBM (masking and dropout in Appendix B). This avoids repeatedly reading and writing of inputs and outputs from and to HBM.

```
Algorithm 1 FlashAttention

Require: Matrices \(\mathbf{Q}, \mathbf{K}, \mathbf{V} \in \mathbb{R}^{N \times d}\) in HBM, on-chip SRAM of size \(M\).
1: Set block sizes \(B_c = \left\lceil \frac{M}{4d} \right\rceil\), \(B_r = \min\left(\left\lceil \frac{M}{4d} \right\rceil, d\right)\).
2: Initialize \(\mathbf{O} = (\mathbf{0})_{N \times d} \in \mathbb{R}^{N \times d}\), \(\ell = (\mathbf{0})_N \in \mathbb{R}^N\), \(m = (-\infty)_N \in \mathbb{R}^N\) in HBM.
3: Divide \(\mathbf{Q}\) into \(T_r = \left\lceil \frac{N}{B_r} \right\rceil\) blocks \(\mathbf{Q}_1, \ldots, \mathbf{Q}_{T_r}\) of size \(B_r \times d\) each, and divide \(\mathbf{K}, \mathbf{V}\) into \(T_c = \left\lceil \frac{N}{B_c} \right\rceil\) blocks \(\mathbf{K}_1, \ldots, \mathbf{K}_{T_c}\) and \(\mathbf{V}_1, \ldots, \mathbf{V}_{T_c}\), of size \(B_c \times d\) each.
4: Divide \(\mathbf{O}\) into \(T_r\) blocks \(\mathbf{O}_1, \ldots, \mathbf{O}_{T_r}\) of size \(B_r \times d\) each, divide \(\ell\) into \(T_r\) blocks \(\ell_1, \ldots, \ell_{T_r}\) of size \(B_r\) each, divide \(m\) into \(T_r\) blocks \(m_1, \ldots, m_{T_r}\) of size \(B_r\) each.
5: for \(1 \le j \le T_c\) do
6: \(\quad\) Load \(\mathbf{K}_j, \mathbf{V}_j\) from HBM to on-chip SRAM.
7: \(\quad\) for \(1 \le i \le T_r\) do
8: \(\qquad\) Load \(\mathbf{Q}_i, \mathbf{O}_i, \ell_i, m_i\) from HBM to on-chip SRAM.
9: \(\qquad\) On chip, compute \(\mathbf{S}_{ij} = \mathbf{Q}_i \mathbf{K}_j^T \in \mathbb{R}^{B_r \times B_c}\).
10: \(\qquad\) On chip, compute \(\widetilde{m}_{ij} = \operatorname{rowmax}(\mathbf{S}_{ij}) \in \mathbb{R}^{B_r}\), \(\widetilde{\mathbf{P}}_{ij} = \exp(\mathbf{S}_{ij} - \widetilde{m}_{ij}) \in \mathbb{R}^{B_r \times B_c}\) (pointwise), \(\widetilde{\ell}_{ij} = \operatorname{rowsum}(\widetilde{\mathbf{P}}_{ij}) \in \mathbb{R}^{B_r}\).
11: \(\qquad\) On chip, compute \(m_i^{\mathrm{new}} = \max(m_i, \widetilde{m}_{ij}) \in \mathbb{R}^{B_r}\), \(\ell_i^{\mathrm{new}} = e^{m_i - m_i^{\mathrm{new}}} \ell_i + e^{\widetilde{m}_{ij} - m_i^{\mathrm{new}}} \widetilde{\ell}_{ij} \in \mathbb{R}^{B_r}\).
12: \(\qquad\) Write \(\mathbf{O}_i \leftarrow \operatorname{diag}(\ell_i^{\mathrm{new}})^{-1} \left(\operatorname{diag}(\ell_i) e^{m_i - m_i^{\mathrm{new}}} \mathbf{O}_i + e^{\widetilde{m}_{ij} - m_i^{\mathrm{new}}} \widetilde{\mathbf{P}}_{ij} \mathbf{V}_j\right)\) to HBM.
13: \(\qquad\) Write \(\ell_i \leftarrow \ell_i^{\mathrm{new}}, m_i \leftarrow m_i^{\mathrm{new}}\) to HBM.
14: \(\quad\) end for
15: end for
16: Return \(\mathbf{O}\).
```

We show FᴸᴀsʜAᴛᴛᴇɴᴛɪᴏɴ’s correctness, runtime, and memory requirement (proof in Appendix C).

Theorem 1. Algorithm 1 returns \\(\\mathbf\{O\} = \\operatorname\{softmax\}(\\mathbf\{QK\}^\{\\mathsf\{T\}\})\\mathbf\{V\}\\) with \\(O(N^2 d)\\) FLOPs and requires \\(O(N)\\) additional memory beyond inputs and output.
