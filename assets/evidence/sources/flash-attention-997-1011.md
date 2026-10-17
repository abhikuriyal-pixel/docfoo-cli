We compare our method/implementation with Apex FMHA (https://github.com/NVIDIA/apex/tree/ master/apex/contrib/csrc/fmha).

When we started this project, Apex FMHA was the fastest implementation of attention (that we knew of), tailored for short sequences of length at most 512. In fact, almost all MLPerf submissions for BERT training benchmark running on Nvidia GPUs use FMHA for their model code, as of MLPerf 1.1 \[58\]. Since

---

Table 7: Runtime (ms) of FlashAttention compared to FMHA by sequence length, with masking and dropout, measured on an A100-SXM4-40GB GPU. Batch size 64, 16 heads, head dimension 64 (i.e., BERT-large size).

![A three-column numeric table with no visible caption. The left column is headed “Attention Method”; the numeric columns are headed 128, 256, and 512. Horizontal rules separate the header, forward, backward, and combined sections, and a double rule appears along the bottom. - Apex FMHA forward: 0.10, 0.29, 1.14 - FlashAttention forward: 0.08, 0.22, 0.81; all three values are bold. - Apex FMHA backward: 0.17, 0.52, 1.81; all three values are bold. - FlashAttention backward: 0.20, 0.53, 2.00. - Apex FMHA forward + backward: 0.27, 0.81, 2.95. - FlashAttention forward + backward: 0.28, 0.75, 2.81; the values 0.75 and 2.81 are bold.](assets/table_7_p28_07.jpg)

FMHA targets BERT models, it only supports head dimension 64, and only runs on A100 GPUs. FMHA fuses the attention computation dropout(softmax(QKᵀ))V into one CUDA kernel. In the forward pass, it stores the attention matrix softmax(QKᵀ) to HBM to be used in gradient computation. As a result, it does not offer substantial memory saving (though for shorter sequences memory footprint is often not a primary concern).

We use FMHA code as a starting point, and apply two well-established techniques (tiling and recomputation) to deal with long sequences and to save memory as mentioned in Section 3. As a result, we can support much longer sequences (e.g., up to length 64K). We also support more head dimensions (16, 32, 64, 128) and broader GPU types (all Turing and Ampere GPUs at the time of writing).

In Table 7, we compare the performance of FlashAttention and Apex FMHA for short sequences (as FMHA only supports sequence length at most 512). Generally FlashAttention is slightly faster than FMHA in the forward pass and slightly slower than FMHA in the backward pass. This is because we do not store the attention matrix in the forward pass and recompute it in the backward pass. Compared to FMHA, the overall runtime of FlashAttention is about 4% slower for sequence length 128, 8% faster for sequence length 256, and 5% faster for sequence length 512.
