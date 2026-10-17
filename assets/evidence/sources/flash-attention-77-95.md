Given input sequences \\(\\mathbf\{Q\}, \\mathbf\{K\}, \\mathbf\{V\} \\in \\mathbb\{R\}^\{N \\times d\}\\) where \\(N\\) is the sequence length and \\(d\\) is the head dimension, we want to compute the attention output \\(\\mathbf\{O\} \\in \\mathbb\{R\}^\{N \\times d\}\\).

$$
\mathbf{S} = \mathbf{Q}\mathbf{K}^{\mathsf{T}} \in \mathbb{R}^{N \times N},\quad \mathbf{P} = \operatorname{softmax}(\mathbf{S}) \in \mathbb{R}^{N \times N},\quad \mathbf{O} = \mathbf{P}\mathbf{V} \in \mathbb{R}^{N \times d},
$$

where softmax is applied row-wise.

Standard attention implementations materialize the matrices S and P to HBM, which takes O(N²) memory. Often N ≫ d (e.g., for GPT2, N = 1024 and d = 64). We describe the standard attention implementation in Algorithm 0. As some or most of the operations are memory-bound (e.g., softmax), the large number of memory accesses translates to slow wall-clock time.

This problem is exacerbated by other elementwise operations applied to the attention matrix, such as masking applied to S or dropout applied to P. As a result, there have been many attempts to fuse several elementwise operations, such as fusing masking with softmax \[77\].

In Section 3.2, we will show that the standard attention implementation performs HBM accesses quadratic in the sequence length \\(N\\). We also compare the number of FLOPs and number of HBM accesses of standard attention and of our method (FLASHATTENTION).

Algorithm 0 Standard Attention Implementation

Require: Matrices Q, K, V ∈ ℝ^\{N×d\} in HBM.

2: Read S from HBM, compute P = softmax(S), write P to HBM.
