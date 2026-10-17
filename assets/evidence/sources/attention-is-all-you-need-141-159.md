Since our model contains no recurrence and no convolution, in order for the model to make use of the order of the sequence, we must inject some information about the relative or absolute position of the tokens in the sequence. To this end, we add "positional encodings" to the input embeddings at the

---

Table 1: Maximum path lengths, per-layer complexity and minimum number of sequential operations for different layer types. \\(n\\) is the sequence length, \\(d\\) is the representation dimension, \\(k\\) is the kernel size of convolutions and \\(r\\) the size of the neighborhood in restricted self-attention.

![A four-column table with five rows. The headers are “Layer Type,” “Complexity per Layer,” “Sequential Operations” (wrapped onto two lines), and “Maximum Path Length.” Rows: - Self-Attention \| O(n² · d) \| O(1) \| O(1) - Recurrent \| O(n · d²) \| O(n) \| O(n) - Convolutional \| O(k · n · d²) \| O(1) \| O(logₖ(n)) - Self-Attention (restricted) \| O(r · n · d) \| O(1) \| O(n/r) Thin horizontal rules appear above the header, below the header, and beneath the last row. The table has no visible vertical rules.](assets/table_1_p06_01.jpg)

bottoms of the encoder and decoder stacks. The positional encodings have the same dimension \\(d\_\{\\text\{model\}\}\\) as the embeddings, so that the two can be summed. There are many choices of positional encodings, learned and fixed \[8\].

In this work, we use sine and cosine functions of different frequencies:

$$
\(PE_{(pos,2i+1)} = cos(pos/10000^{2i/d_{model}})\)
$$

where $pos$ is the position and $i$ is the dimension. That is, each dimension of the positional encoding corresponds to a sinusoid. The wavelengths form a geometric progression from $2\pi$ to $10000 \cdot 2\pi$. We chose this function because we hypothesized it would allow the model to easily learn to attend by relative positions, since for any fixed offset $k$, $PE_{pos+k}$ can be represented as a linear function of $PE_{pos}$.

We also experimented with using learned positional embeddings \[8\] instead, and found that the two versions produced nearly identical results (see Table 3 row (E)). We chose the sinusoidal version because it may allow the model to extrapolate to sequence lengths longer than the ones encountered during training.
