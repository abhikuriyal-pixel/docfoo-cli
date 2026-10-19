Theorem 2. Let \\(N\\) be the sequence length, \\(d\\) be the head dimension, and \\(M\\) be size of SRAM with \\(d \\le M \\le Nd\\). Standard attention (Algorithm 0) requires \\(\\Theta(Nd + N^2)\\) HBM accesses, while FlashAttention (Algorithm 1) requires \\(\\Theta(N^2 d M^\{-1\})\\) HBM accesses.

For typical values of d (64-128) and M (around 100KB), d² is many times smaller than M, and thus FLASH ATTENTION requires many times fewer HBM accesses than standard implementation. This leads to both faster execution and lower memory footprint, which we validate in Section 4.3.
